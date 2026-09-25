//! Turn execution: the tool loop that drives the agent.

use async_trait::async_trait;
use serde_json::Value;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::bus::{
    EndpointName, NoticeEvent, NotifyName, Publisher, SYSTEM_CHANNEL, SessionAddress,
    SessionEventKind, SessionResponseEvent, ToolActivityEvent, ToolCallEvent, ToolResultEvent,
    TurnUsageEvent, topics,
};
use crate::inference::{
    CompletionOptions, InferenceProvider, InferenceResponse, Message, ToolCall,
};
use crate::mcp::SharedMcpRegistry;
use crate::tools::{
    CANCELLED_BEFORE_START, CANCELLED_WHILE_RUNNING, ToolError, ToolRegistry, ToolResult,
};
use crate::workspace::identity::IdentityFiles;
use anyhow::Context;

use super::context::{MemoryContext, PromptContext, StatusLine, assemble_system_prompt};
use super::hop::HopCounter;
use super::interrupt::{Interrupt, InterruptSource};
use super::recent_messages::RecentMessages;
use super::usage::{SessionUsageTotals, TurnUsage, UsageSink};

/// Context for publishing streaming events during a turn.
pub(crate) struct EventContext<'a> {
    pub publisher: &'a Publisher,
    pub target: EventTarget<'a>,
    /// When this turn belongs to a conversation session, its own address and
    /// the conversation it replies to on its interface. Intermediate
    /// (pre-tool-call) text is then *additionally* published as a
    /// [`SessionResponseEvent`] to that conversation — the same event, and
    /// the same "never falls back to the owner's DM" delivery, its turn
    /// responses use — alongside the ordinary session-stream event every
    /// [`EventTarget::Session`] turn publishes. `None` for the main agent's
    /// own turns and for a session with no conversation of its own to reply
    /// to.
    pub session_conversation: Option<SessionConversationTarget<'a>>,
}

/// Where a turn's streaming events (tool activity, intermediate text) go.
pub(crate) enum EventTarget<'a> {
    /// The main agent: interactive endpoint topics, correlated to the
    /// message that started the turn. Either endpoint may be absent (e.g. a
    /// background turn with nowhere to show its output).
    Endpoint {
        output_endpoint: Option<&'a EndpointName>,
        tool_activity_endpoint: Option<&'a EndpointName>,
        correlation_id: &'a str,
    },
    /// An agent session: the sessions topic, tagged with the session's
    /// address and run id.
    Session {
        address: &'a SessionAddress,
        run_id: &'a str,
    },
}

/// Where a conversation session's intermediate turn text is *additionally*
/// delivered, alongside the ordinary session-stream event — the interface
/// endpoint its conversation lives on, and the conversation id itself. See
/// [`EventContext::session_conversation`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionConversationTarget<'a> {
    pub(crate) session_address: &'a SessionAddress,
    pub(crate) endpoint: &'a str,
    pub(crate) conversation_id: &'a str,
}

impl EventContext<'_> {
    /// Correlation id stamped on tool activity events. Sessions have no
    /// inbound message to correlate to; their events are identified by the
    /// session address and run id instead.
    fn correlation_id(&self) -> &str {
        match self.target {
            EventTarget::Endpoint { correlation_id, .. } => correlation_id,
            EventTarget::Session { .. } => "",
        }
    }

    async fn publish_tool_activity(&self, event: ToolActivityEvent, tool_name: &str) {
        match self.target {
            EventTarget::Endpoint {
                tool_activity_endpoint: Some(ep),
                ..
            } => {
                if let Err(e) = self
                    .publisher
                    .publish(topics::Endpoint(ep.clone()), event)
                    .await
                {
                    tracing::debug!(error = %e, tool_name = %tool_name, "failed to publish tool activity event");
                }
            }
            EventTarget::Endpoint {
                tool_activity_endpoint: None,
                ..
            } => {}
            EventTarget::Session { address, run_id } => {
                let kind = match event {
                    ToolActivityEvent::Call(call) => SessionEventKind::ToolCall(call),
                    ToolActivityEvent::Result(result) => SessionEventKind::ToolResult(result),
                };
                crate::background::events::publish_session_event(
                    self.publisher,
                    address,
                    run_id,
                    kind,
                )
                .await;
            }
        }
    }

    /// Publish this turn's intermediate (pre-tool-call) text: to the
    /// session-stream topic for [`EventTarget::Session`] (or main's own
    /// endpoint topic for [`EventTarget::Endpoint`]), and — when
    /// [`Self::session_conversation`] is set — additionally as a
    /// [`SessionResponseEvent`] to the conversation session's own
    /// conversation, never falling back to the owner's DM, the same rule its
    /// turn responses follow.
    async fn publish_intermediate(&self, content: &str) {
        match self.target {
            EventTarget::Endpoint {
                output_endpoint: Some(ep),
                correlation_id,
                ..
            } => {
                if let Err(e) = self
                    .publisher
                    .publish(
                        topics::Endpoint(ep.clone()),
                        crate::bus::IntermediateEvent {
                            correlation_id: correlation_id.to_owned(),
                            content: content.to_owned(),
                        },
                    )
                    .await
                {
                    tracing::debug!(error = %e, "failed to publish intermediate text event");
                }
            }
            EventTarget::Endpoint {
                output_endpoint: None,
                ..
            } => {}
            EventTarget::Session { address, run_id } => {
                crate::background::events::publish_session_event(
                    self.publisher,
                    address,
                    run_id,
                    SessionEventKind::Intermediate {
                        content: content.to_owned(),
                    },
                )
                .await;
            }
        }

        if let Some(target) = self.session_conversation
            && let Err(e) = self
                .publisher
                .publish(
                    topics::Endpoint(EndpointName::from(target.endpoint)),
                    SessionResponseEvent {
                        session_address: target.session_address.clone(),
                        conversation_id: target.conversation_id.to_string(),
                        content: content.to_string(),
                        attachment: None,
                        timestamp: chrono::Utc::now().naive_utc(),
                        is_final: false,
                    },
                )
                .await
        {
            tracing::debug!(error = %e, "failed to publish intermediate text to conversation");
        }
    }

    /// Publish this turn's live usage progress after a model call: this
    /// turn's own output tokens so far (for the running-turn indicator)
    /// and, when the caller tracks cumulative session totals, the updated
    /// totals (for the chat footer). Never reaches the agent itself — see
    /// `docs/systems-usage/turn-control.md`.
    async fn publish_usage(&self, turn: TurnUsage, session_totals: Option<SessionUsageTotals>) {
        match self.target {
            EventTarget::Endpoint {
                output_endpoint: Some(ep),
                correlation_id,
                ..
            } => {
                if let Err(e) = self
                    .publisher
                    .publish(
                        topics::Endpoint(ep.clone()),
                        TurnUsageEvent {
                            correlation_id: correlation_id.to_owned(),
                            output_tokens: turn.output_tokens,
                            has_usage: turn.has_usage,
                            session_totals,
                        },
                    )
                    .await
                {
                    tracing::debug!(error = %e, "failed to publish turn usage event");
                }
            }
            EventTarget::Endpoint {
                output_endpoint: None,
                ..
            } => {}
            EventTarget::Session { address, run_id } => {
                crate::background::events::publish_session_event(
                    self.publisher,
                    address,
                    run_id,
                    SessionEventKind::TurnUsage {
                        output_tokens: turn.output_tokens,
                        has_usage: turn.has_usage,
                        session_totals,
                    },
                )
                .await;
            }
        }
    }
}

/// Maximum retries for empty responses (transient API glitches).
const MAX_EMPTY_RESPONSE_RETRIES: u32 = 2;

/// Sink for incremental transcript persistence.
///
/// Called after every model response and every tool result is appended to
/// `recent_messages`, so a crash mid-turn loses at most the message
/// currently in flight rather than the whole turn. The main agent's own
/// persistence path is separate (`recent_messages.json`, written after the
/// whole turn), so its turns pass `None`; a session's turn passes a sink
/// bound to its run so the session store's transcript stays durable as the
/// run progresses.
#[async_trait]
pub(crate) trait TranscriptSink: Send + Sync {
    /// Append newly-produced messages to the durable transcript.
    async fn append(&self, messages: &[Message]);
}

/// Shared subsystem references needed for each turn iteration.
pub(crate) struct TurnResources<'a> {
    pub provider: &'a dyn InferenceProvider,
    pub tools: &'a ToolRegistry,
    pub mcp_registry: &'a SharedMcpRegistry,
    pub identity: &'a IdentityFiles,
    pub options: &'a CompletionOptions,
    /// Maximum tool-call iterations before the turn stops itself
    /// gracefully. `None` (the default) means unlimited — the user's own
    /// Cancel / `stop_agent` is the intended safety valve for a runaway
    /// turn. See [`crate::config::AgentAbilitiesConfig::max_tool_iterations`].
    pub max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call over and
    /// over. See [`crate::config::AgentAbilitiesConfig::repeat_call_guard`].
    pub repeat_call_guard: crate::config::RepeatCallGuardConfig,
    /// Cancelled when the user asks to stop this turn. Raced directly
    /// against the in-flight model call so generation aborts immediately;
    /// turns that don't support being stopped (system/wake turns) pass a
    /// token nobody ever cancels.
    pub stop_token: &'a CancellationToken,
    /// Incremental transcript persistence, or `None` when the caller
    /// persists the transcript some other way (the main agent).
    pub transcript_sink: Option<&'a dyn TranscriptSink>,
    /// Durable session-level usage totals to accumulate this turn's model
    /// calls into, for the web UI's chat footer. `None` for a turn that
    /// doesn't track them (tests, and any turn kind that never surfaces to
    /// a web client). The per-turn running-turn indicator publishes
    /// regardless of whether this is set.
    pub usage_sink: Option<&'a dyn UsageSink>,
    /// This turn's current hop count: set by the caller to the kickoff
    /// input's hop count before the turn starts, and raised here whenever an
    /// `Interrupt::AgentMessage` is drained mid-turn — that message becomes
    /// one more input driving the turn, alongside the kickoff.
    pub hop_counter: &'a HopCounter,
}

/// System note injected into the conversation when a turn is stopped
/// mid-flight, so the next turn knows the work above was cut short rather
/// than completed or abandoned.
const STOP_NOTE: &str = "[Stopped] the user stopped this turn before it finished; review what's already been done above before continuing or repeating any of it.";

/// A model response was cut off by the configured output-token limit: tell
/// the user (a transient notice, not an error — the turn otherwise
/// completed normally) and leave a system note in the transcript so the
/// agent knows its own last response was truncated. There is no automatic
/// continuation — whether to pick up where it left off is the agent's own
/// call, made the same way any other turn decision is.
async fn handle_output_truncated(
    response: &InferenceResponse,
    resources: &TurnResources<'_>,
    events: &EventContext<'_>,
    recent_messages: &mut RecentMessages,
) {
    let limit_desc = resources.options.max_tokens.map_or_else(
        || "its configured output-token limit".to_string(),
        |n| format!("the {n}-token output limit"),
    );
    tracing::warn!(
        max_tokens = ?resources.options.max_tokens,
        stop_reason = ?response.stop_reason,
        "model response truncated at the output-token limit"
    );

    let notice = format!("The response was cut off at {limit_desc}.");
    if let Err(e) = events
        .publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent { message: notice },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish truncation notice");
    }

    let note = Message::system(format!(
        "[Truncated] your previous response was cut off at {limit_desc}; it may be \
         incomplete. Continue from where it left off if that's still useful, or start fresh."
    ));
    push_and_record(recent_messages, resources.transcript_sink, note).await;
}

/// Push a message onto the turn's history and, when a sink is configured,
/// durably record it in the same step — the one place every message that
/// enters `recent_messages` during a turn also reaches the transcript sink.
async fn push_and_record(
    recent_messages: &mut RecentMessages,
    sink: Option<&dyn TranscriptSink>,
    message: Message,
) {
    recent_messages.push(message.clone());
    if let Some(sink) = sink {
        sink.append(&[message]).await;
    }
}

/// Same as [`push_and_record`], for a batch of messages produced together
/// (e.g. the history messages an injected mid-turn user message expands
/// into).
async fn push_and_record_many(
    recent_messages: &mut RecentMessages,
    sink: Option<&dyn TranscriptSink>,
    messages: Vec<Message>,
) {
    if let Some(sink) = sink {
        sink.append(&messages).await;
    }
    recent_messages.extend(messages);
}

/// Push `notice` as the turn's one final assistant message and return it as
/// the turn's whole result. Shared by every "end the turn here with a
/// message the user sees" path in [`execute_turn`]'s loop (the
/// `max_tool_iterations` cutoff, the repeat-call guard's stop threshold) so
/// the push/record/return sequence lives in one place.
async fn finish_turn_with_notice(
    notice: String,
    recent_messages: &mut RecentMessages,
    resources: &TurnResources<'_>,
) -> anyhow::Result<Vec<String>> {
    let final_message = Message::assistant(notice.clone(), None);
    push_and_record(recent_messages, resources.transcript_sink, final_message).await;
    Ok(vec![notice])
}

/// Execute the tool loop against the given message buffer.
///
/// Calls the provider repeatedly until it returns a text response (no tool calls),
/// executing any requested tools in between. Updates `recent_messages` in place.
///
/// MCP tool definitions are merged into the built-in tool list, and tool calls
/// fall back to MCP servers when no built-in tool matches. Name collisions are
/// resolved by the MCP registry before this merge: a built-in always wins and
/// the colliding MCP tool is dropped from the union, so the model is never
/// offered two definitions under one name. See `src/mcp/CLAUDE.md`.
///
/// Returns a vec containing the final text-only response. Intermediate texts
/// emitted alongside tool calls are sent via `reply` in real-time but not
/// included in the return value.
#[tracing::instrument(skip_all, fields(operation = "execute_turn"))]
#[expect(
    clippy::too_many_arguments,
    reason = "turn execution context spans resources, contexts, and watchers"
)]
pub(crate) async fn execute_turn(
    resources: &TurnResources<'_>,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    status_line: Option<&StatusLine>,
    interrupt_rx: &mut dyn InterruptSource,
    subconscious: Option<&crate::subconscious::SubconsciousWatch>,
) -> anyhow::Result<Vec<String>> {
    let mut texts: Vec<String> = Vec::new();
    let mut empty_retries: u32 = 0;
    // Includes the triggering user message pushed just before this call.
    let turn_start = recent_messages.len().saturating_sub(1);
    // This turn's own running totals for the web UI's turn-in-progress
    // indicator. Never exposed to the agent — see `docs/systems-usage/turn-control.md`.
    let mut turn_usage = TurnUsage::default();
    let mut repeat_guard = RepeatCallGuard::new();

    let mut iteration: usize = 0;
    loop {
        if check_tool_iteration_limit(resources, recent_messages, iteration, &mut texts).await {
            return Ok(texts);
        }

        if check_interrupts_and_stop(interrupt_rx, recent_messages, resources, iteration).await {
            return Ok(texts);
        }

        let mut tool_definitions = resources.tools.definitions();

        // Merge MCP tool definitions from all connected servers. The registry
        // has already dropped any MCP tool whose name collides with a built-in
        // or with an earlier server, so this union never carries a duplicate
        // name (collision policy: src/mcp/CLAUDE.md).
        let mcp_guard = resources.mcp_registry.read().await;
        tool_definitions.extend(mcp_guard.tool_definitions());
        drop(mcp_guard);

        // System prompt is reassembled each iteration to pick up MCP changes.
        // Identity is a snapshot taken at turn entry (reloaded from disk each
        // turn), so mid-turn identity-file edits apply on the next turn.
        let messages = assemble_system_prompt(
            resources.identity,
            recent_messages,
            memory_ctx,
            prompt_ctx,
            status_line,
        );

        let mut response = tokio::select! {
            biased;
            () = resources.stop_token.cancelled() => {
                tracing::info!(iterations = iteration, "turn stopped by user during model call");
                push_and_record(recent_messages, resources.transcript_sink, Message::system(STOP_NOTE)).await;
                return Ok(texts);
            }
            result = resources.provider.complete(&messages, &tool_definitions, resources.options) => {
                result.context("model completion failed")?
            }
        };

        if let Some(ref thinking) = response.thinking {
            tracing::debug!(
                thinking_len = thinking.len(),
                "structured thinking received"
            );
        }
        response.content = super::think_tags::strip_think_tags(&response.content);

        if response.was_truncated() {
            handle_output_truncated(&response, resources, events, recent_messages).await;
        }

        if response.tool_calls.is_empty() {
            log_usage(&response);
            update_and_publish_usage(&response, &mut turn_usage, resources.usage_sink, events)
                .await;
            if response.content.is_empty() {
                if empty_retries < MAX_EMPTY_RESPONSE_RETRIES {
                    empty_retries += 1;
                    tracing::warn!(
                        attempt = empty_retries,
                        max = MAX_EMPTY_RESPONSE_RETRIES,
                        "model returned empty response, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                anyhow::bail!("model returned empty response with no tool calls");
            }
            tracing::debug!(iterations = iteration, "turn complete");
            let final_message = Message::assistant(response.content.clone(), None);
            push_and_record(recent_messages, resources.transcript_sink, final_message).await;
            texts.push(response.content);
            return Ok(texts);
        }

        tracing::debug!(
            iteration,
            tool_count = response.tool_calls.len(),
            "processing tool calls"
        );

        if let Some(notice) = handle_tool_call_response(
            &response,
            recent_messages,
            &mut repeat_guard,
            ToolCallResponseContext {
                resources,
                events,
                subconscious,
                iteration,
                turn_start,
            },
        )
        .await
        {
            return finish_turn_with_notice(notice, recent_messages, resources).await;
        }

        log_usage(&response);
        update_and_publish_usage(&response, &mut turn_usage, resources.usage_sink, events).await;
        iteration += 1;
    }
}

/// Check whether this iteration has reached the configured
/// `max_tool_iterations` limit and, if so, end the turn gracefully with an
/// explanatory final message. Returns `true` when the turn should stop
/// (its result is already pushed onto `texts`). Split out of
/// [`execute_turn`] purely to keep that function's line count down.
async fn check_tool_iteration_limit(
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    iteration: usize,
    texts: &mut Vec<String>,
) -> bool {
    let Some(limit) = resources.max_tool_iterations else {
        return false;
    };
    if iteration < limit {
        return false;
    }
    tracing::warn!(
        tool_calls = limit,
        "turn stopped: reached the configured max_tool_iterations limit"
    );
    let notice = format!(
        "I stopped after {limit} tool calls — the limit set by `max_tool_iterations` \
         in your Residuum config. Raise or remove that setting (under `[agent]` in \
         config.toml, or in Settings) to allow longer turns."
    );
    let final_message = Message::assistant(notice.clone(), None);
    push_and_record(recent_messages, resources.transcript_sink, final_message).await;
    texts.push(notice);
    true
}

/// Per-iteration locals [`handle_tool_call_response`] needs from
/// [`execute_turn`]'s loop, bundled so the function itself stays within a
/// normal argument count.
struct ToolCallResponseContext<'a> {
    resources: &'a TurnResources<'a>,
    events: &'a EventContext<'a>,
    subconscious: Option<&'a crate::subconscious::SubconsciousWatch>,
    iteration: usize,
    turn_start: usize,
}

/// Handle a model response that carries tool calls: publish the
/// intermediate text, record the assistant message, kick off a mid-turn
/// subconscious evaluation, and run the tool-call batch. Split out of
/// [`execute_turn`] purely to keep that function's line count down.
///
/// Returns `Some(notice)` when the repeat-call guard's stop threshold ended
/// the turn on this batch — the caller turns that into the turn's final
/// user-visible message. `None` means the loop should continue as normal.
async fn handle_tool_call_response(
    response: &InferenceResponse,
    recent_messages: &mut RecentMessages,
    repeat_guard: &mut RepeatCallGuard,
    ctx: ToolCallResponseContext<'_>,
) -> Option<String> {
    if !response.content.is_empty() {
        ctx.events.publish_intermediate(&response.content).await;
    }

    let msg = Message::assistant(response.content.clone(), Some(response.tool_calls.clone()));
    push_and_record(recent_messages, ctx.resources.transcript_sink, msg).await;

    // Classification runs concurrently with tool execution; a correction
    // lands at a later drain_interrupts poll.
    if let Some(watch) = ctx.subconscious {
        watch.maybe_spawn(
            ctx.iteration,
            recent_messages.messages_since(ctx.turn_start).to_vec(),
        );
    }

    let (tool_name, count) = run_tool_call_batch(
        &response.tool_calls,
        ctx.resources,
        recent_messages,
        ctx.events,
        repeat_guard,
    )
    .await?;

    tracing::warn!(
        tool_name = %tool_name,
        consecutive = count,
        "turn stopped: model repeated the same tool call too many times in a row"
    );
    Some(format!(
        "I stopped this turn because I called `{tool_name}` with the exact same arguments \
         {count} times in a row — the result can't change by calling it again. Adjust \
         `repeat_call_stop_after` under `[agent]` in your Residuum config (or in Settings) if \
         this is expected, or let me know what you'd like me to try instead."
    ))
}

/// Drain interrupts at a tool-loop checkpoint and report whether the turn
/// should stop, logging the reason. Split out of [`execute_turn`] purely to
/// keep that function's line count down.
async fn check_interrupts_and_stop(
    interrupt_rx: &mut dyn InterruptSource,
    recent_messages: &mut RecentMessages,
    resources: &TurnResources<'_>,
    iteration: usize,
) -> bool {
    let mut stopped = drain_interrupts(
        interrupt_rx,
        recent_messages,
        resources.transcript_sink,
        resources.hop_counter,
    )
    .await;

    // A stop that has no matching `Interrupt::Stopped` marker still needs to
    // end the turn here — e.g. daemon shutdown cancels the turn's own
    // `stop_token` directly (see `run_agent_turn_with_interrupts`) without
    // routing anything through this interrupt channel.
    if !stopped && resources.stop_token.is_cancelled() {
        push_and_record(
            recent_messages,
            resources.transcript_sink,
            Message::system(STOP_NOTE),
        )
        .await;
        stopped = true;
    }

    if stopped {
        tracing::info!(
            iterations = iteration,
            "turn stopped before next model call"
        );
    }
    stopped
}

/// Drain any interrupt messages that arrived while tools were executing.
///
/// Returns `true` if a stop was observed among the drained interrupts, so
/// the caller can end the turn gracefully at this checkpoint. Draining
/// continues to the end of the buffered batch even after a stop is seen, so
/// any interrupts queued just before it are still folded into history.
async fn drain_interrupts(
    interrupt_rx: &mut dyn InterruptSource,
    recent_messages: &mut RecentMessages,
    sink: Option<&dyn TranscriptSink>,
    hop_counter: &HopCounter,
) -> bool {
    let mut stopped = false;
    while let Ok(interrupt) = interrupt_rx.try_recv() {
        match interrupt {
            Interrupt::UserMessage(msg) => {
                tracing::info!(msg_id = %msg.id, "injecting mid-turn user message");
                push_and_record_many(recent_messages, sink, msg.into_history_messages()).await;
            }
            Interrupt::AgentMessage(msg) => {
                tracing::info!(
                    from = %msg.from,
                    category = %msg.from_category,
                    hop_count = msg.hop_count,
                    "injecting agent message mid-turn"
                );
                hop_counter.bump(msg.hop_count);
                push_and_record(recent_messages, sink, msg.to_history_message()).await;
            }
            Interrupt::Subconscious(content) => {
                tracing::info!("injecting subconscious correction mid-turn");
                push_and_record(recent_messages, sink, Message::system(content)).await;
            }
            Interrupt::Stopped => {
                tracing::info!("turn stopped by user, recording note for next turn");
                push_and_record(recent_messages, sink, Message::system(STOP_NOTE)).await;
                stopped = true;
            }
        }
    }
    stopped
}

/// Tracks the current streak of consecutive identical tool calls within a
/// turn, for [`run_tool_call_batch`]'s repeat-call guard.
///
/// Observed failure this guards against: GLM 5.3 Flash has been seen calling
/// a tool with byte-identical arguments hundreds of times in a row — the
/// result cannot change, so repeating the call again never makes progress.
/// A call "repeats" the previous one when it names the same tool and its
/// raw argument JSON (as received from the model) is identical; any other
/// call — a different tool, or different arguments — resets the streak back
/// to a single call. Calls within a single model response's batch count in
/// order, exactly like calls from separate model responses.
struct RepeatCallGuard {
    last: Option<(String, Value)>,
    consecutive: u32,
}

impl RepeatCallGuard {
    fn new() -> Self {
        Self {
            last: None,
            consecutive: 0,
        }
    }

    /// Record the next call about to be considered and return the updated
    /// streak length (1 for a call that doesn't repeat the previous one).
    fn record(&mut self, name: &str, arguments: &Value) -> u32 {
        let repeats_last = self
            .last
            .as_ref()
            .is_some_and(|(last_name, last_args)| last_name == name && last_args == arguments);
        if repeats_last {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last = Some((name.to_string(), arguments.clone()));
        }
        self.consecutive
    }
}

/// Run every tool call a model response carries, in order.
///
/// Once the turn is stopped — whether that happens during one of these
/// calls or was already true going in — every remaining call in the batch
/// is skipped rather than run, but still gets a recorded result (see
/// `skip_tool_call`) so the transcript stays valid for the next provider
/// call. The same happens once the repeat-call guard's stop threshold is
/// reached; when that happens this returns `Some((tool_name, consecutive))`
/// so the caller can end the turn with a user-visible notice.
async fn run_tool_call_batch(
    tool_calls: &[ToolCall],
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    repeat_guard: &mut RepeatCallGuard,
) -> Option<(String, u32)> {
    let guard_cfg = resources.repeat_call_guard;
    let mut stopped_by_repeat_guard = None;
    let mut skipped = 0_usize;
    for tool_call in tool_calls {
        if resources.stop_token.is_cancelled() || stopped_by_repeat_guard.is_some() {
            skip_tool_call(tool_call, resources, recent_messages, events).await;
            skipped += 1;
            continue;
        }

        if guard_cfg.enabled {
            let consecutive = repeat_guard.record(&tool_call.name, &tool_call.arguments);
            if consecutive >= guard_cfg.stop_after {
                stop_repeated_tool_call(tool_call, consecutive, resources, recent_messages, events)
                    .await;
                stopped_by_repeat_guard = Some((tool_call.name.clone(), consecutive));
                continue;
            }
            if consecutive >= guard_cfg.steer_after {
                tracing::debug!(
                    tool_name = %tool_call.name,
                    consecutive,
                    "steering a repeated tool call"
                );
                let note = format!(
                    "You've made this exact call {consecutive} times in a row with the same \
                     arguments; the result won't change. Try something different or finish."
                );
                execute_tool(tool_call, resources, recent_messages, events, Some(note)).await;
                continue;
            }
        }

        execute_tool(tool_call, resources, recent_messages, events, None).await;
    }
    if skipped > 0 {
        tracing::info!(
            skipped,
            total = tool_calls.len(),
            "skipped remaining tool calls after the turn was stopped"
        );
    }
    stopped_by_repeat_guard
}

/// Dispatch a tool call to the built-in registry, falling back to MCP.
///
/// Returns the result alongside whether the MCP fallback was used, so the
/// caller's error-source logging can distinguish a built-in failure from an
/// MCP one. A malformed name means the inference provider failed to parse
/// the model's tool-call syntax into structured JSON (e.g. GLM's
/// `<arg_key>`/`<arg_value>` template leaking through unparsed) — dispatch
/// to the built-in or MCP registry would only ever produce a confusing
/// "unknown tool" lookup failure, so this short-circuits with a clear error
/// instead of trying both registries first.
async fn dispatch_tool_call(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
) -> (Result<ToolResult, ToolError>, bool) {
    if !crate::tools::is_plausible_tool_name(&tool_call.name) {
        return (
            Err(ToolError::MalformedName(
                crate::tools::truncate_tool_name_for_display(&tool_call.name),
            )),
            false,
        );
    }

    match resources
        .tools
        .execute_cancellable(
            &tool_call.name,
            tool_call.arguments.clone(),
            resources.stop_token,
        )
        .await
    {
        // Try built-in tools first, fall back to MCP servers. This ordering
        // is the dispatch half of the collision policy: a built-in always
        // wins its name, and the registry has already hidden any shadowed
        // MCP tool from the model (src/mcp/CLAUDE.md), so the fallback only
        // ever reaches genuinely MCP-owned names.
        Err(ToolError::NotFound(_)) => {
            tracing::debug!(tool_name = %tool_call.name, "tool not found in built-in registry, falling back to MCP");
            (execute_mcp_tool(tool_call, resources).await, true)
        }
        other => (other, false),
    }
}

/// Dispatch a tool call to whichever MCP server owns it.
///
/// The MCP registry has no cancellation awareness of its own (unlike
/// `execute_cancellable`'s built-in path), so this races the call directly:
/// on a stop, the call is dropped and a cancellation result reported
/// instead of waiting for an MCP server that may never answer.
///
/// The registry's read lock is held only long enough to resolve the tool to
/// a cheap-to-clone client handle, then dropped before the call is awaited
/// — otherwise a slow or hung MCP server would hold the lock for the call's
/// whole duration, blocking a config reload's write lock and every other
/// agent's next iteration behind it.
async fn execute_mcp_tool(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
) -> Result<ToolResult, ToolError> {
    let resolved = resources
        .mcp_registry
        .read()
        .await
        .resolve_tool(&tool_call.name);
    let handle = resolved?;
    let call = handle.call_tool(&tool_call.name, tool_call.arguments.clone());
    tokio::select! {
        biased;
        () = resources.stop_token.cancelled() => Ok(ToolResult::cancelled(CANCELLED_WHILE_RUNNING)),
        r = call => r,
    }
}

/// Execute a single tool call, falling back to MCP servers.
///
/// `steering_note`, when set, is appended to the tool's own result — used by
/// the repeat-call guard to nudge a model that keeps calling this tool with
/// the same arguments, without altering whether the call itself succeeded.
#[tracing::instrument(skip_all, fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
async fn execute_tool(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    steering_note: Option<String>,
) {
    events
        .publish_tool_activity(
            ToolActivityEvent::Call(ToolCallEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            }),
            &tool_call.name,
        )
        .await;

    let (result, used_mcp) = dispatch_tool_call(tool_call, resources).await;

    let (mut output, is_error, images) = match result {
        Ok(r) => (r.output, r.is_error, r.images),
        Err(e) => {
            let source = match (&e, used_mcp) {
                (ToolError::MalformedName(_), _) => "validation",
                (_, true) => "mcp",
                (_, false) => "built-in",
            };
            tracing::warn!(
                error = %e,
                tool_name = %crate::tools::truncate_tool_name_for_display(&tool_call.name),
                tool_call_id = %tool_call.id,
                source,
                "tool execution failed"
            );
            (e.to_string(), true, vec![])
        }
    };

    // Reached only when the dispatch above actually raced against the stop
    // (built-in default `execute_cancellable`, `ExecTool`'s own override, or
    // the MCP race just above) and lost — i.e. this call was running when
    // the turn was stopped, not merely one that happened to run after.
    if resources.stop_token.is_cancelled() {
        tracing::info!(
            tool_name = %tool_call.name,
            tool_call_id = %tool_call.id,
            "tool call interrupted: the turn was stopped while it was running"
        );
    }

    // The one redaction point for tool output: everything downstream (the
    // web UI activity feed, recent_messages, transcripts, episodes, the
    // search index, and the next provider call) reads this string.
    if resources
        .tools
        .redactor()
        .await
        .redact_in_place(&mut output)
    {
        tracing::debug!(
            tool_name = %tool_call.name,
            tool_call_id = %tool_call.id,
            "redacted agent key values from tool output"
        );
    }

    // Repeat-call guard steering note, appended after redaction so it never
    // gets mistaken for agent-key-bearing tool output.
    if let Some(note) = steering_note {
        output.push_str("\n\n");
        output.push_str(&note);
    }

    events
        .publish_tool_activity(
            ToolActivityEvent::Result(ToolResultEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                output: output.clone(),
                is_error,
            }),
            &tool_call.name,
        )
        .await;

    let tool_message = if images.is_empty() {
        Message::tool(output, tool_call.id.clone())
    } else {
        Message::tool_with_images(output, tool_call.id.clone(), images)
    };
    push_and_record(recent_messages, resources.transcript_sink, tool_message).await;
}

/// Record a tool call that never ran, publishing the same call/result event
/// pair a real execution would, with a cancellation notice standing in for
/// the result. Shared by [`skip_tool_call`] and [`stop_repeated_tool_call`]:
/// every tool call a response carries still needs a matching tool result —
/// providers reject a transcript with a call left unanswered.
async fn record_cancelled_tool_call(
    tool_call: &ToolCall,
    output: String,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
) {
    events
        .publish_tool_activity(
            ToolActivityEvent::Call(ToolCallEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            }),
            &tool_call.name,
        )
        .await;

    let result = ToolResult::cancelled(output);

    events
        .publish_tool_activity(
            ToolActivityEvent::Result(ToolResultEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                output: result.output.clone(),
                is_error: result.is_error,
            }),
            &tool_call.name,
        )
        .await;

    let tool_message = Message::tool(result.output, tool_call.id.clone());
    push_and_record(recent_messages, resources.transcript_sink, tool_message).await;
}

/// Record a tool call that never ran because the turn had already been
/// stopped (or the repeat-call guard had already fired) by the time its
/// turn came up in this response's batch.
async fn skip_tool_call(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
) {
    tracing::info!(
        tool_name = %tool_call.name,
        tool_call_id = %tool_call.id,
        "tool call skipped: the turn was stopped before it started"
    );
    record_cancelled_tool_call(
        tool_call,
        CANCELLED_BEFORE_START.to_string(),
        resources,
        recent_messages,
        events,
    )
    .await;
}

/// Record a tool call that the repeat-call guard did not run because its
/// stop threshold was reached: the model has called this tool with
/// byte-identical arguments `consecutive` times in a row, and repeating it
/// again cannot produce a different result.
async fn stop_repeated_tool_call(
    tool_call: &ToolCall,
    consecutive: u32,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
) {
    let output = format!(
        "cancelled: this exact call (same tool, same arguments) was made {consecutive} times \
         in a row; the turn was stopped instead of running it again because the result cannot \
         change — try a different approach or report back instead"
    );
    record_cancelled_tool_call(tool_call, output, resources, recent_messages, events).await;
}

/// Log token usage from a model response at debug level.
fn log_usage(response: &InferenceResponse) {
    if let Some(usage) = response.usage {
        tracing::debug!(
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_creation_tokens = usage.cache_creation_tokens,
            cache_read_tokens = usage.cache_read_tokens,
            "token usage"
        );
    } else {
        tracing::debug!("token usage not available in response");
    }
}

/// Fold a model call's usage into this turn's running totals, into the
/// durable session totals when the turn tracks them, and publish the
/// result for the web UI's running-turn indicator and chat footer. Never
/// delivered to the agent itself — see `docs/systems-usage/turn-control.md`.
async fn update_and_publish_usage(
    response: &InferenceResponse,
    turn_usage: &mut TurnUsage,
    usage_sink: Option<&dyn UsageSink>,
    events: &EventContext<'_>,
) {
    turn_usage.accumulate(response.usage);
    let session_totals = match usage_sink {
        Some(sink) => Some(sink.accumulate(response.usage).await),
        None => None,
    };
    events.publish_usage(*turn_usage, session_totals).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Role;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    /// Collects every message it's asked to record, so a test can assert the
    /// incremental transcript sink saw exactly what `recent_messages` did.
    #[derive(Default)]
    struct MockSink {
        recorded: StdMutex<Vec<Message>>,
    }

    #[async_trait]
    impl TranscriptSink for MockSink {
        async fn append(&self, messages: &[Message]) {
            self.recorded.lock().unwrap().extend_from_slice(messages);
        }
    }

    #[tokio::test]
    async fn drain_injects_subconscious_correction_as_system_message() {
        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        tx.send(Interrupt::Subconscious(
            "[Subconscious] Save the preference.".to_string(),
        ))
        .ok();
        let stopped =
            drain_interrupts(&mut rx, &mut recent, Some(&sink), &HopCounter::new(0)).await;

        assert!(!stopped, "a subconscious correction is not a stop");
        assert_eq!(recent.len(), 1, "one message should be injected");
        let msg = recent.messages().first();
        assert_eq!(msg.map(|m| m.role), Some(Role::System));
        assert_eq!(
            msg.map(|m| m.content.as_str()),
            Some("[Subconscious] Save the preference."),
            "correction content should be preserved"
        );
        assert_eq!(
            contents(&sink.recorded.lock().unwrap()),
            contents(recent.messages()),
            "the transcript sink should have recorded the same message"
        );
    }

    #[tokio::test]
    async fn drain_reports_stop_and_injects_note() {
        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        tx.send(Interrupt::Stopped).ok();
        let stopped =
            drain_interrupts(&mut rx, &mut recent, Some(&sink), &HopCounter::new(0)).await;

        assert!(stopped, "a queued Stopped interrupt should report true");
        assert_eq!(recent.len(), 1, "the stop note should be injected");
        let msg = recent.messages().first();
        assert_eq!(msg.map(|m| m.role), Some(Role::System));
        assert_eq!(msg.map(|m| m.content.as_str()), Some(STOP_NOTE));

        let recorded = sink.recorded.lock().unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "the stop note must reach the transcript sink, not just recent_messages"
        );
        assert_eq!(
            recorded.first().map(|m| m.content.as_str()),
            Some(STOP_NOTE)
        );
    }

    #[tokio::test]
    async fn drain_with_no_interrupts_reports_no_stop() {
        let (_tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();

        let stopped = drain_interrupts(&mut rx, &mut recent, None, &HopCounter::new(0)).await;

        assert!(!stopped, "an empty channel should never report a stop");
        assert_eq!(recent.len(), 0, "nothing should be injected");
    }

    #[tokio::test]
    async fn drain_injects_user_message_history_through_the_sink() {
        use crate::interfaces::types::{InboundMessage, MessageOrigin};

        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        let inbound = InboundMessage {
            id: "msg-1".to_string(),
            content: "hello mid-turn".to_string(),
            origin: MessageOrigin {
                endpoint: "test".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: None,
        };
        tx.send(Interrupt::UserMessage(inbound)).ok();
        let stopped =
            drain_interrupts(&mut rx, &mut recent, Some(&sink), &HopCounter::new(0)).await;

        assert!(!stopped);
        assert!(!recent.messages().is_empty());
        assert_eq!(
            contents(&sink.recorded.lock().unwrap()),
            contents(recent.messages()),
            "every history message an injected user message expands into must reach the sink"
        );
    }

    /// Text content of each message, for comparing two message lists without
    /// requiring `Message: PartialEq`.
    fn contents(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.content.as_str()).collect()
    }

    #[tokio::test]
    async fn publish_intermediate_for_a_conversation_session_also_uses_session_response_event() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("discord");
        let mut conv_sub: crate::bus::Subscriber<SessionResponseEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let mut intermediate_sub: crate::bus::Subscriber<crate::bus::IntermediateEvent> =
            bus_handle
                .subscribe(topics::Endpoint(ep.clone()))
                .await
                .unwrap();
        let mut session_sub: crate::bus::Subscriber<crate::bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();

        let address = SessionAddress::from("external-discord-chan-1");
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Session {
                address: &address,
                run_id: "run-1",
            },
            session_conversation: Some(SessionConversationTarget {
                session_address: &address,
                endpoint: "discord",
                conversation_id: "chan-1",
            }),
        };

        events.publish_intermediate("checking the build now").await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), conv_sub.recv())
            .await
            .expect("a SessionResponseEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.content, "checking the build now");
        assert_eq!(event.session_address, address);
        assert_eq!(event.conversation_id, "chan-1");
        assert!(event.attachment.is_none());
        assert!(
            !event.is_final,
            "intermediate turn text must not be marked as the run's final output"
        );

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                intermediate_sub.recv()
            )
            .await
            .is_err(),
            "a conversation session's intermediate text must never publish IntermediateEvent, \
             which would route through main's correlation-id target lookup and log the \
             misleading \"owner has not messaged the bot yet\" line"
        );

        let session_event =
            tokio::time::timeout(std::time::Duration::from_secs(1), session_sub.recv())
                .await
                .expect(
                    "the session-stream event should still be published, same as any other session",
                )
                .unwrap()
                .unwrap();
        assert_eq!(session_event.address, address);
        assert_eq!(session_event.run_id, "run-1");
        assert!(
            matches!(
                session_event.kind,
                crate::bus::SessionEventKind::Intermediate { ref content } if content == "checking the build now"
            ),
            "a conversation session's intermediate text must reach the web sessions stream too"
        );
    }

    #[tokio::test]
    async fn publish_intermediate_without_a_session_conversation_uses_intermediate_event() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<crate::bus::IntermediateEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: Some(&ep),
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        events.publish_intermediate("thinking...").await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("an IntermediateEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.content, "thinking...");
        assert_eq!(event.correlation_id, "corr-1");
    }

    #[tokio::test]
    async fn publish_intermediate_with_no_output_endpoint_is_a_noop() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<crate::bus::IntermediateEvent> =
            bus_handle.subscribe(topics::Endpoint(ep)).await.unwrap();

        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        events.publish_intermediate("nothing to see").await;

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv())
                .await
                .is_err(),
            "with no output endpoint there is nowhere to publish intermediate text"
        );
    }

    #[tokio::test]
    async fn execute_tool_rejects_malformed_name_without_registry_lookup() {
        // Mirrors a Fireworks/GLM response whose native `<arg_key>`/`<arg_value>`
        // tool-call template leaked through unparsed into the structured
        // `tool_calls` name field, instead of a clean `write_file`.
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "write_file\tcontent</arg_key><arg_value># Hello".to_string(),
            arguments: serde_json::json!({}),
        };

        let provider = crate::inference::providers::null::NullProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };

        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, None).await;

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool, "should be a tool-result message");
        assert!(
            msg.content.contains("provider likely failed to parse"),
            "the model should be told why its call was rejected: {}",
            msg.content
        );
        assert!(
            !msg.content.starts_with("unknown tool:"),
            "should not surface the confusing 'unknown tool' registry-lookup message: {}",
            msg.content
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_tool_redacts_agent_key_values_before_recording() {
        let dir = tempfile::tempdir().unwrap();
        let keys = crate::agent_keys::AgentKeys::new_shared(dir.path());
        keys.set(
            "api_key",
            "sk-live-redactme42",
            None,
            crate::agent_keys::KeyCreator::User,
        )
        .await
        .unwrap();

        let mut tools = ToolRegistry::new();
        tools.set_agent_keys(keys);
        tools.register_defaults(
            crate::tools::FileTracker::new_shared(),
            crate::tools::PathPolicy::new_shared(),
            crate::diagnostics::DiagnosticsPaths {
                config_dir: std::path::PathBuf::from("/tmp/residuum-test-config-unused"),
                workspace_dir: std::path::PathBuf::from("/tmp/residuum-test-workspace-unused"),
            },
        );

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "exec".to_string(),
            arguments: serde_json::json!({
                "command": "echo \"Authorization: Bearer $API_KEY\"",
                "keys": ["api_key"]
            }),
        };

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, None).await;

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert!(
            !msg.content.contains("sk-live-redactme42"),
            "the key value must never reach history: {}",
            msg.content
        );
        assert!(
            msg.content
                .contains("Authorization: Bearer [agent-key:api_key]"),
            "the value should be replaced by its marker: {}",
            msg.content
        );
    }

    /// A tool whose `execute()` never resolves, so the default
    /// `execute_cancellable()` can only return via its cancellation branch —
    /// proving dispatch actually races against the stop token instead of
    /// waiting for the tool to finish.
    struct BlockingTool;

    #[async_trait]
    impl crate::tools::Tool for BlockingTool {
        fn name(&self) -> &'static str {
            "blocking_tool"
        }

        fn definition(&self) -> crate::inference::ToolDefinition {
            crate::inference::ToolDefinition {
                name: self.name().to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            }
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn execute_tool_reports_cancellation_when_stopped_mid_execution() {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(BlockingTool));

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "blocking_tool".to_string(),
            arguments: serde_json::json!({}),
        };

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let cancel_after_a_moment = {
            let stop_token = stop_token.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                stop_token.cancel();
            }
        };

        let mut recent = RecentMessages::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                execute_tool(&tool_call, &resources, &mut recent, &events, None),
                cancel_after_a_moment,
            )
        })
        .await
        .expect("a stop should let execute_tool return well within 2s of a hung tool");

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool, "should be a tool-result message");
        assert_eq!(
            msg.content,
            crate::tools::CANCELLED_WHILE_RUNNING,
            "the transcript should say the tool was cancelled, not that it failed"
        );
    }

    #[tokio::test]
    async fn skip_tool_call_records_a_cancelled_result_without_running_anything() {
        let tools = ToolRegistry::new();
        let tool_call = ToolCall {
            id: "call-2".to_string(),
            name: "whatever".to_string(),
            arguments: serde_json::json!({}),
        };

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        skip_tool_call(&tool_call, &resources, &mut recent, &events).await;

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool, "should be a tool-result message");
        assert_eq!(msg.content, crate::tools::CANCELLED_BEFORE_START);
    }

    /// Returns a two-tool-call response exactly once; a second call means
    /// the turn looped back for another model call instead of stopping,
    /// which this test must catch.
    struct TwoToolCallsProvider {
        served: AtomicBool,
    }

    #[async_trait]
    impl InferenceProvider for TwoToolCallsProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, crate::inference::InferenceError> {
            assert!(
                !self.served.swap(true, Ordering::SeqCst),
                "the turn should have stopped after the first batch, not called complete() again"
            );
            Ok(InferenceResponse::new(
                String::new(),
                vec![
                    ToolCall {
                        id: "call-1".to_string(),
                        name: "blocking_tool".to_string(),
                        arguments: serde_json::json!({}),
                    },
                    ToolCall {
                        id: "call-2".to_string(),
                        name: "blocking_tool".to_string(),
                        arguments: serde_json::json!({}),
                    },
                ],
            ))
        }

        fn model_name(&self) -> &'static str {
            "two-tool-calls"
        }
    }

    #[tokio::test]
    async fn execute_turn_skips_remaining_tool_calls_after_a_mid_batch_stop() {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(BlockingTool));

        let provider = TwoToolCallsProvider {
            served: AtomicBool::new(false),
        };
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let memory_ctx = MemoryContext {
            observations: None,
            recent_context: None,
        };
        let prompt_ctx = PromptContext::default();
        let (_interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("go"));

        let cancel_after_a_moment = {
            let stop_token = stop_token.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                stop_token.cancel();
            }
        };

        let (turn_result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                execute_turn(
                    &resources,
                    &memory_ctx,
                    &prompt_ctx,
                    &mut recent,
                    &events,
                    None,
                    &mut interrupt_rx,
                    None,
                ),
                cancel_after_a_moment,
            )
        })
        .await
        .expect("the turn should end well within 2s of the stop");

        let texts = turn_result.expect("a stopped turn is not a turn error");
        assert!(texts.is_empty(), "a stopped turn produces no final text");

        let messages = recent.messages();
        let tool_results: Vec<_> = messages.iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(
            tool_results.len(),
            2,
            "every tool call in the batch must get a matching result"
        );
        assert_eq!(
            tool_results
                .first()
                .expect("checked above: exactly 2 results")
                .content,
            crate::tools::CANCELLED_WHILE_RUNNING,
            "the call that was running when the stop landed was interrupted"
        );
        assert_eq!(
            tool_results
                .get(1)
                .expect("checked above: exactly 2 results")
                .content,
            crate::tools::CANCELLED_BEFORE_START,
            "the second call in the batch must never have started"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.role == Role::System && m.content == STOP_NOTE),
            "the stop must be recorded in history for the next turn"
        );
    }

    struct TruncatedProvider;

    #[async_trait]
    impl InferenceProvider for TruncatedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, crate::inference::InferenceError> {
            let mut resp = InferenceResponse::new("cut off mid-sen".to_string(), vec![]);
            resp.stop_reason = Some(crate::inference::StopReason::MaxTokens);
            Ok(resp)
        }

        fn model_name(&self) -> &'static str {
            "truncated"
        }
    }

    /// A [`UsageSink`] test double that records every call it's asked to
    /// accumulate and returns pre-set cumulative totals in sequence.
    #[derive(Default)]
    struct MockUsageSink {
        calls: StdMutex<Vec<Option<crate::inference::Usage>>>,
        totals_to_return: StdMutex<Vec<SessionUsageTotals>>,
    }

    impl MockUsageSink {
        fn new(totals_to_return: Vec<SessionUsageTotals>) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                totals_to_return: StdMutex::new(totals_to_return),
            }
        }
    }

    #[async_trait]
    impl UsageSink for MockUsageSink {
        async fn accumulate(&self, usage: Option<crate::inference::Usage>) -> SessionUsageTotals {
            self.calls.lock().unwrap().push(usage);
            let mut totals = self.totals_to_return.lock().unwrap();
            if totals.len() > 1 {
                totals.remove(0)
            } else {
                totals.first().copied().unwrap_or_default()
            }
        }
    }

    fn usage(input: u32, output: u32) -> crate::inference::Usage {
        crate::inference::Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }
    }

    // ── RepeatCallGuard ──────────────────────────────────────────────────────

    #[test]
    fn repeat_call_guard_counts_consecutive_identical_calls() {
        let mut guard = RepeatCallGuard::new();
        let args = serde_json::json!({"x": 1});
        assert_eq!(guard.record("a", &args), 1);
        assert_eq!(guard.record("a", &args), 2);
        assert_eq!(guard.record("a", &args), 3);
    }

    #[test]
    fn repeat_call_guard_resets_on_different_arguments() {
        let mut guard = RepeatCallGuard::new();
        assert_eq!(guard.record("a", &serde_json::json!({"x": 1})), 1);
        assert_eq!(guard.record("a", &serde_json::json!({"x": 1})), 2);
        assert_eq!(
            guard.record("a", &serde_json::json!({"x": 2})),
            1,
            "different arguments should reset the streak"
        );
        assert_eq!(guard.record("a", &serde_json::json!({"x": 2})), 2);
    }

    #[test]
    fn repeat_call_guard_resets_on_different_tool_name() {
        let mut guard = RepeatCallGuard::new();
        let args = serde_json::json!({});
        assert_eq!(guard.record("a", &args), 1);
        assert_eq!(guard.record("a", &args), 2);
        assert_eq!(
            guard.record("b", &args),
            1,
            "a different tool name should reset the streak"
        );
    }

    /// Records how many times it actually ran, so a test can prove the
    /// repeat-call guard's stop threshold prevented a call from executing
    /// rather than merely producing a cancelled-looking result some other way.
    struct CountingTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl crate::tools::Tool for CountingTool {
        fn name(&self) -> &'static str {
            "counting_tool"
        }

        fn definition(&self) -> crate::inference::ToolDefinition {
            crate::inference::ToolDefinition {
                name: self.name().to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            }
        }

        async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("ok"))
        }
    }

    /// Build a `counting_tool` call with the given id, always carrying the
    /// same arguments — used to drive the repeat-call guard's streak.
    fn counting_tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "counting_tool".to_string(),
            arguments: serde_json::json!({"x": 1}),
        }
    }

    #[tokio::test]
    async fn publish_usage_for_endpoint_target_maps_to_turn_usage_event() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<TurnUsageEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: Some(&ep),
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(usage(100, 20)));
        events
            .publish_usage(
                TurnUsage {
                    output_tokens: 20,
                    has_usage: true,
                },
                Some(totals),
            )
            .await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a TurnUsageEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.correlation_id, "corr-1");
        assert_eq!(event.output_tokens, 20);
        assert!(event.has_usage);
        assert_eq!(event.session_totals, Some(totals));
    }

    #[tokio::test]
    async fn publish_usage_with_no_output_endpoint_is_a_noop() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<TurnUsageEvent> =
            bus_handle.subscribe(topics::Endpoint(ep)).await.unwrap();

        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        events.publish_usage(TurnUsage::default(), None).await;

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv())
                .await
                .is_err(),
            "with no output endpoint there is nowhere to publish turn usage"
        );
    }

    #[tokio::test]
    async fn publish_usage_for_a_session_target_uses_session_event_kind_turn_usage() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut sub: crate::bus::Subscriber<crate::bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();

        let address = SessionAddress::from("spawned-x-0001");
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Session {
                address: &address,
                run_id: "run-1",
            },
            session_conversation: None,
        };

        events
            .publish_usage(
                TurnUsage {
                    output_tokens: 7,
                    has_usage: true,
                },
                None,
            )
            .await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a SessionEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.address, address);
        assert_eq!(event.run_id, "run-1");
        assert!(matches!(
            event.kind,
            crate::bus::SessionEventKind::TurnUsage {
                output_tokens: 7,
                has_usage: true,
                session_totals: None,
            }
        ));
    }

    #[tokio::test]
    async fn update_and_publish_usage_feeds_the_sink_and_accumulates_the_turn_total() {
        let sink = MockUsageSink::new(vec![{
            let mut t = SessionUsageTotals::default();
            t.accumulate(Some(usage(100, 20)));
            t
        }]);
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<TurnUsageEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: Some(&ep),
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(100, 20));
        update_and_publish_usage(&response, &mut turn_usage, Some(&sink), &events).await;

        assert_eq!(
            turn_usage.output_tokens, 20,
            "the turn's own running total should accumulate"
        );
        assert_eq!(
            sink.calls.lock().unwrap().as_slice(),
            &[Some(usage(100, 20))],
            "the sink should receive exactly the response's usage"
        );

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a TurnUsageEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.output_tokens, 20);
        assert_eq!(event.session_totals.map(|t| t.input_tokens), Some(100));
    }

    #[tokio::test]
    async fn update_and_publish_usage_with_no_sink_still_publishes_the_turn_progress() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<TurnUsageEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: Some(&ep),
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(50, 5));
        update_and_publish_usage(&response, &mut turn_usage, None, &events).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a TurnUsageEvent should be published promptly")
            .unwrap()
            .unwrap();
        assert_eq!(event.output_tokens, 5);
        assert_eq!(
            event.session_totals, None,
            "with no usage sink there are no session totals to report"
        );
    }

    #[tokio::test]
    async fn update_and_publish_usage_a_provider_with_no_usage_leaves_totals_blank() {
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: crate::bus::Subscriber<TurnUsageEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: Some(&ep),
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let response = InferenceResponse::new("hi".to_string(), vec![]);
        update_and_publish_usage(&response, &mut turn_usage, Some(&sink), &events).await;

        assert!(
            !turn_usage.has_usage,
            "a provider reporting no usage must not flip has_usage"
        );
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a TurnUsageEvent should still be published so the indicator keeps ticking")
            .unwrap()
            .unwrap();
        assert!(!event.has_usage);
        assert_eq!(event.output_tokens, 0);
    }

    #[tokio::test]
    async fn execute_turn_notices_and_notes_a_truncated_response() {
        let provider = TruncatedProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions {
            max_tokens: Some(64),
            ..CompletionOptions::default()
        };
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };

        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: crate::bus::Subscriber<crate::bus::NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let memory_ctx = MemoryContext {
            observations: None,
            recent_context: None,
        };
        let prompt_ctx = PromptContext::default();
        let (_interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("go"));

        let texts = execute_turn(
            &resources,
            &memory_ctx,
            &prompt_ctx,
            &mut recent,
            &events,
            None,
            &mut interrupt_rx,
            None,
        )
        .await
        .expect("a truncated response is still a completed turn, not an error");
        assert_eq!(texts, vec!["cut off mid-sen".to_string()]);

        let notice = tokio::time::timeout(std::time::Duration::from_secs(1), notices.recv())
            .await
            .expect("a truncation notice should be published")
            .unwrap()
            .unwrap();
        assert!(
            notice.message.contains("64-token output limit"),
            "the notice should name the configured limit: {}",
            notice.message
        );

        assert!(
            recent
                .messages()
                .iter()
                .any(|m| m.role == Role::System && m.content.contains("[Truncated]")),
            "a system note about the truncation must be added to the transcript"
        );
    }
    #[tokio::test]
    async fn repeat_guard_steers_at_the_configured_threshold_without_blocking_the_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let batch = vec![
            counting_tool_call("c1"),
            counting_tool_call("c2"),
            counting_tool_call("c3"),
        ];

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig {
                enabled: true,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let stopped =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard).await;

        assert!(stopped.is_none(), "3 repeats is below the stop threshold");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "all three calls should have actually run"
        );

        let tool_messages: Vec<_> = recent
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.clone())
            .collect();
        let [first, second, third] = tool_messages.as_slice() else {
            panic!(
                "expected exactly 3 tool-result messages, got {}",
                tool_messages.len()
            );
        };
        assert!(
            !first.contains("Try something different"),
            "the first call should carry no steering note: {first}"
        );
        assert!(
            !second.contains("Try something different"),
            "the second call should carry no steering note: {second}"
        );
        assert!(
            third.contains("3 times in a row"),
            "the third (threshold-reaching) call should carry the steering note: {third}"
        );
    }

    #[tokio::test]
    async fn repeat_guard_stops_at_the_configured_threshold_without_running_that_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let batch: Vec<ToolCall> = (0..7)
            .map(|i| counting_tool_call(&format!("c{i}")))
            .collect();

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig {
                enabled: true,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let stopped =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard).await;

        let (name, count) = stopped.expect("the 6th identical call should stop the turn");
        assert_eq!(name, "counting_tool");
        assert_eq!(count, 6);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            5,
            "only the first 5 calls should actually run; the 6th is refused and the 7th skipped"
        );

        let tool_messages: Vec<_> = recent
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.clone())
            .collect();
        let [_, _, _, _, _, sixth, seventh] = tool_messages.as_slice() else {
            panic!(
                "expected exactly 7 tool-result messages (one per call in the batch), got {}",
                tool_messages.len()
            );
        };
        assert!(
            sixth.contains("cancelled"),
            "the 6th call's result should explain the stop, not run: {sixth}"
        );
        assert_eq!(
            seventh.as_str(),
            crate::tools::CANCELLED_BEFORE_START,
            "the 7th call should be skipped outright, not attempted"
        );
    }

    #[tokio::test]
    async fn repeat_guard_disabled_never_steers_or_stops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let batch: Vec<ToolCall> = (0..10)
            .map(|i| counting_tool_call(&format!("c{i}")))
            .collect();

        let provider = crate::inference::providers::null::NullProvider;
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig {
                enabled: false,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let stopped =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard).await;

        assert!(stopped.is_none(), "a disabled guard never stops the turn");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            10,
            "every call should run when the guard is disabled"
        );
        assert!(
            recent
                .messages()
                .iter()
                .filter(|m| m.role == Role::Tool)
                .all(|m| !m.content.contains("Try something different")),
            "no steering note should appear when the guard is disabled"
        );
    }

    /// Always returns the same single `counting_tool` call, counting how
    /// many times `complete()` was invoked — a test asserts this stays at
    /// the configured stop threshold instead of growing without bound.
    struct RepeatingCallProvider {
        completions: AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for RepeatingCallProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, crate::inference::InferenceError> {
            let n = self.completions.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(InferenceResponse::new(
                String::new(),
                vec![counting_tool_call(&format!("call-{n}"))],
            ))
        }

        fn model_name(&self) -> &'static str {
            "repeating-call"
        }
    }

    #[tokio::test]
    async fn execute_turn_stops_when_the_model_repeats_the_same_call_past_the_stop_threshold() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool {
            calls: Arc::clone(&calls),
        }));

        let provider = RepeatingCallProvider {
            completions: AtomicUsize::new(0),
        };
        let mcp_registry = crate::mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            target: EventTarget::Endpoint {
                output_endpoint: None,
                tool_activity_endpoint: None,
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let memory_ctx = MemoryContext {
            observations: None,
            recent_context: None,
        };
        let prompt_ctx = PromptContext::default();
        let (_interrupt_tx, mut interrupt_rx) = mpsc::channel(4);
        let mut recent = RecentMessages::new();
        recent.push(Message::user("loop forever"));

        let texts = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            execute_turn(
                &resources,
                &memory_ctx,
                &prompt_ctx,
                &mut recent,
                &events,
                None,
                &mut interrupt_rx,
                None,
            ),
        )
        .await
        .expect("the repeat-call guard should end the turn well within 2s")
        .expect("a guard-stopped turn is not a turn error");

        assert_eq!(texts.len(), 1);
        let notice = texts.first().unwrap();
        assert!(
            notice.contains("times in a row"),
            "the notice should explain why the turn stopped: {notice}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            5,
            "the 6th identical call must never actually run"
        );
    }
}

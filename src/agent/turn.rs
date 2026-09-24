//! Turn execution: the tool loop that drives the agent.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::bus::{
    EndpointName, Publisher, SessionAddress, SessionEventKind, SessionResponseEvent,
    ToolActivityEvent, ToolCallEvent, ToolResultEvent, topics,
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
use super::interrupt::Interrupt;
use super::recent_messages::RecentMessages;

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
    /// Cancelled when the user asks to stop this turn. Raced directly
    /// against the in-flight model call so generation aborts immediately;
    /// turns that don't support being stopped (system/wake turns) pass a
    /// token nobody ever cancels.
    pub stop_token: &'a CancellationToken,
    /// Incremental transcript persistence, or `None` when the caller
    /// persists the transcript some other way (the main agent).
    pub transcript_sink: Option<&'a dyn TranscriptSink>,
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
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
    subconscious: Option<&crate::subconscious::SubconsciousWatch>,
) -> anyhow::Result<Vec<String>> {
    let mut texts: Vec<String> = Vec::new();
    let mut empty_retries: u32 = 0;
    // Includes the triggering user message pushed just before this call.
    let turn_start = recent_messages.len().saturating_sub(1);

    let mut iteration: usize = 0;
    loop {
        if let Some(limit) = resources.max_tool_iterations
            && iteration >= limit
        {
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

        if response.tool_calls.is_empty() {
            log_usage(&response);
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

        if !response.content.is_empty() {
            events.publish_intermediate(&response.content).await;
        }

        let msg = Message::assistant(response.content.clone(), Some(response.tool_calls.clone()));
        push_and_record(recent_messages, resources.transcript_sink, msg).await;

        // Classification runs concurrently with tool execution; a correction
        // lands at a later drain_interrupts poll.
        if let Some(watch) = subconscious {
            watch.maybe_spawn(
                iteration,
                recent_messages.messages_since(turn_start).to_vec(),
            );
        }

        run_tool_call_batch(&response.tool_calls, resources, recent_messages, events).await;

        log_usage(&response);
        iteration += 1;
    }
}

/// Drain interrupts at a tool-loop checkpoint and report whether the turn
/// should stop, logging the reason. Split out of [`execute_turn`] purely to
/// keep that function's line count down.
async fn check_interrupts_and_stop(
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
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
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
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

/// Run every tool call a model response carries, in order.
///
/// Once the turn is stopped — whether that happens during one of these
/// calls or was already true going in — every remaining call in the batch
/// is skipped rather than run, but still gets a recorded result (see
/// `skip_tool_call`) so the transcript stays valid for the next provider
/// call.
async fn run_tool_call_batch(
    tool_calls: &[ToolCall],
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
) {
    let mut skipped = 0_usize;
    for tool_call in tool_calls {
        if resources.stop_token.is_cancelled() {
            skip_tool_call(tool_call, resources, recent_messages, events).await;
            skipped += 1;
            continue;
        }
        execute_tool(tool_call, resources, recent_messages, events).await;
    }
    if skipped > 0 {
        tracing::info!(
            skipped,
            total = tool_calls.len(),
            "skipped remaining tool calls after the turn was stopped"
        );
    }
}

/// Execute a single tool call, falling back to MCP servers.
#[tracing::instrument(skip_all, fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
async fn execute_tool(
    tool_call: &ToolCall,
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

    // A malformed name means the inference provider failed to parse the
    // model's tool-call syntax into structured JSON (e.g. GLM's
    // `<arg_key>`/`<arg_value>` template leaking through unparsed) — dispatch
    // to the built-in or MCP registry would only ever produce a confusing
    // "unknown tool" lookup failure, so short-circuit with a clear error
    // instead of trying both registries first.
    let mut used_mcp = false;
    let result = if crate::tools::is_plausible_tool_name(&tool_call.name) {
        match resources
            .tools
            .execute_cancellable(
                &tool_call.name,
                tool_call.arguments.clone(),
                resources.stop_token,
            )
            .await
        {
            // Try built-in tools first, fall back to MCP servers. This
            // ordering is the dispatch half of the collision policy: a
            // built-in always wins its name, and the registry has already
            // hidden any shadowed MCP tool from the model
            // (src/mcp/CLAUDE.md), so the fallback only ever reaches
            // genuinely MCP-owned names.
            Err(ToolError::NotFound(_)) => {
                tracing::debug!(tool_name = %tool_call.name, "tool not found in built-in registry, falling back to MCP");
                used_mcp = true;
                // The MCP registry has no cancellation awareness of its own
                // (unlike `execute_cancellable`'s built-in path), so this
                // races the call directly: on a stop, the call is dropped
                // and a cancellation result reported instead of waiting for
                // an MCP server that may never answer.
                let mcp_guard = resources.mcp_registry.read().await;
                let call = mcp_guard.call_tool(&tool_call.name, tool_call.arguments.clone());
                tokio::select! {
                    biased;
                    () = resources.stop_token.cancelled() => Ok(ToolResult::cancelled(CANCELLED_WHILE_RUNNING)),
                    r = call => r,
                }
            }
            other => other,
        }
    } else {
        Err(ToolError::MalformedName(
            crate::tools::truncate_tool_name_for_display(&tool_call.name),
        ))
    };

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

/// Record a tool call that never ran because the turn had already been
/// stopped by the time its turn came up in this response's batch.
///
/// Every tool call a response carries still needs a matching tool result —
/// providers reject a transcript with a call left unanswered — so this
/// reports the same call/result event pair a real execution would, with a
/// cancellation notice standing in for the result.
async fn skip_tool_call(
    tool_call: &ToolCall,
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

    tracing::info!(
        tool_name = %tool_call.name,
        tool_call_id = %tool_call.id,
        "tool call skipped: the turn was stopped before it started"
    );

    let result = ToolResult::cancelled(CANCELLED_BEFORE_START);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Role;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};

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
        let (tx, mut rx) = mpsc::channel::<Interrupt>(4);
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        tx.try_send(Interrupt::Subconscious(
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
        let (tx, mut rx) = mpsc::channel::<Interrupt>(4);
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        tx.try_send(Interrupt::Stopped).ok();
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
        let (_tx, mut rx) = mpsc::channel::<Interrupt>(4);
        let mut recent = RecentMessages::new();

        let stopped = drain_interrupts(&mut rx, &mut recent, None, &HopCounter::new(0)).await;

        assert!(!stopped, "an empty channel should never report a stop");
        assert_eq!(recent.len(), 0, "nothing should be injected");
    }

    #[tokio::test]
    async fn drain_injects_user_message_history_through_the_sink() {
        use crate::interfaces::types::{InboundMessage, MessageOrigin};

        let (tx, mut rx) = mpsc::channel::<Interrupt>(4);
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
        tx.try_send(Interrupt::UserMessage(inbound)).ok();
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
            stop_token: &stop_token,
            transcript_sink: None,
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
        execute_tool(&tool_call, &resources, &mut recent, &events).await;

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
            stop_token: &stop_token,
            transcript_sink: None,
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
        execute_tool(&tool_call, &resources, &mut recent, &events).await;

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
            stop_token: &stop_token,
            transcript_sink: None,
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
                execute_tool(&tool_call, &resources, &mut recent, &events),
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
            stop_token: &stop_token,
            transcript_sink: None,
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
            stop_token: &stop_token,
            transcript_sink: None,
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
}

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
use crate::tools::{ToolError, ToolRegistry};
use crate::workspace::identity::IdentityFiles;
use anyhow::Context;

use super::context::{MemoryContext, PromptContext, StatusLine, assemble_system_prompt};
use super::hop::HopCounter;
use super::interrupt::Interrupt;
use super::recent_messages::RecentMessages;

/// Maximum number of tool-call iterations before the agent stops.
pub(crate) const MAX_TOOL_ITERATIONS: usize = 50;

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

    for iteration in 0..MAX_TOOL_ITERATIONS {
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

        for tool_call in &response.tool_calls {
            execute_tool(tool_call, resources, recent_messages, events).await;
        }

        log_usage(&response);
    }

    anyhow::bail!("agent exceeded maximum tool iterations ({MAX_TOOL_ITERATIONS})")
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
    let stopped = drain_interrupts(
        interrupt_rx,
        recent_messages,
        resources.transcript_sink,
        resources.hop_counter,
    )
    .await;
    if stopped {
        tracing::info!(
            iterations = iteration,
            "turn stopped by user before next model call"
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
                push_and_record(recent_messages, sink, Message::user(msg.format_for_agent())).await;
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

    // Try built-in tools first, fall back to MCP servers. This ordering is the
    // dispatch half of the collision policy: a built-in always wins its name,
    // and the registry has already hidden any shadowed MCP tool from the model
    // (src/mcp/CLAUDE.md), so the fallback only ever reaches genuinely
    // MCP-owned names.
    let mut used_mcp = false;
    let result = match resources
        .tools
        .execute(&tool_call.name, tool_call.arguments.clone())
        .await
    {
        Err(ToolError::NotFound(_)) => {
            tracing::debug!(tool_name = %tool_call.name, "tool not found in built-in registry, falling back to MCP");
            used_mcp = true;
            resources
                .mcp_registry
                .read()
                .await
                .call_tool(&tool_call.name, tool_call.arguments.clone())
                .await
        }
        other => other,
    };

    let (output, is_error, images) = match result {
        Ok(r) => (r.output, r.is_error, r.images),
        Err(e) => {
            let source = if used_mcp { "mcp" } else { "built-in" };
            tracing::warn!(
                error = %e,
                tool_name = %tool_call.name,
                tool_call_id = %tool_call.id,
                source,
                "tool execution failed"
            );
            (e.to_string(), true, vec![])
        }
    };

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
}

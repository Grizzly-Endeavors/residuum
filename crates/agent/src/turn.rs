use async_trait::async_trait;
use serde_json::Value;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use anyhow::Context;
use residuum_bus::origin::chat_interface_endpoint;
use residuum_bus::{
    AgentMessageEvent, EndpointName, MainConversationEvent, MessageEvent, NoticeEvent, NotifyName,
    Publisher, SYSTEM_CHANNEL, SessionAddress, SessionEventKind, SessionResponseEvent,
    ToolCallEvent, ToolResultEvent, TurnUsageEvent, topics,
};
use residuum_inference::{
    CompletionOptions, InferenceProvider, InferenceResponse, Message, StreamSink, ToolCall,
    ToolDefinition, joined_thinking_text,
};
use residuum_inference::{ToolError, ToolResult};
use residuum_mcp::SharedMcpRegistry;
use residuum_workspace::identity::IdentityFiles;

use super::context::{
    MemoryContext, PromptContext, RequestLayout, StatusClock, assemble_system_prompt,
};
use super::hop::HopCounter;
use super::recent_messages::RecentMessages;
use super::stream::{ChannelSink, DeltaCoalescer, DiscardSink, StreamPiece};
use super::tool::{
    CANCELLED_BEFORE_START, CANCELLED_WHILE_RUNNING, ToolRegistry, is_plausible_tool_name,
    truncate_tool_name_for_display,
};
use super::usage::{TurnUsage, UsageSink};
use residuum_bus::interrupt::{Interrupt, InterruptSource};
use residuum_bus::usage::SessionUsageTotals;

pub trait ReplyAudience: Send + Sync {
    fn reply_published(&self);
}

#[derive(Debug, Default)]
pub struct LiveOutputEndpoint(std::sync::Mutex<Option<EndpointName>>);

impl LiveOutputEndpoint {
    #[must_use]
    pub fn new(endpoint: Option<EndpointName>) -> Self {
        Self(std::sync::Mutex::new(endpoint))
    }

    #[must_use]
    pub fn get(&self) -> Option<EndpointName> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn replace(&self, endpoint: EndpointName) -> Option<EndpointName> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(endpoint)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TurnOutput<'a> {
    Fixed(Option<&'a EndpointName>),
    Live(&'a LiveOutputEndpoint),
}

impl TurnOutput<'_> {
    #[must_use]
    pub fn current(&self) -> Option<EndpointName> {
        match self {
            Self::Fixed(endpoint) => endpoint.cloned(),
            Self::Live(live) => live.get(),
        }
    }
}

impl<'a> From<Option<&'a EndpointName>> for TurnOutput<'a> {
    fn from(endpoint: Option<&'a EndpointName>) -> Self {
        Self::Fixed(endpoint)
    }
}

impl<'a> From<&'a LiveOutputEndpoint> for TurnOutput<'a> {
    fn from(live: &'a LiveOutputEndpoint) -> Self {
        Self::Live(live)
    }
}

pub struct EventContext<'a> {
    pub publisher: &'a Publisher,
    pub reply_audience: Option<&'a dyn ReplyAudience>,
    pub target: EventTarget<'a>,
    pub session_conversation: Option<SessionConversationTarget<'a>>,
}

enum ToolActivity {
    Call(ToolCallEvent),
    Result(ToolResultEvent),
}

pub enum EventTarget<'a> {
    Endpoint {
        output_endpoint: TurnOutput<'a>,
        correlation_id: &'a str,
    },
    Session {
        address: &'a SessionAddress,
        run_id: &'a str,
        turn_id: &'a str,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SessionConversationTarget<'a> {
    pub session_address: &'a SessionAddress,
    pub endpoint: &'a str,
    pub conversation_id: &'a str,
}

impl EventContext<'_> {
    fn correlation_id(&self) -> &str {
        match self.target {
            EventTarget::Endpoint { correlation_id, .. } => correlation_id,
            EventTarget::Session { .. } => "",
        }
    }

    fn streams_to_main(&self) -> bool {
        matches!(self.target, EventTarget::Endpoint { .. })
    }

    async fn publish_main(&self, build: impl FnOnce(&str) -> MainConversationEvent) {
        let EventTarget::Endpoint { correlation_id, .. } = self.target else {
            return;
        };
        if let Err(e) = self
            .publisher
            .publish(topics::MainConversation, build(correlation_id))
            .await
        {
            tracing::debug!(error = %e, "failed to publish main conversation event");
        }
    }

    async fn publish_tool_activity(&self, event: ToolActivity, call: u32) {
        match self.target {
            EventTarget::Endpoint { .. } => {
                self.publish_main(|_| match event {
                    ToolActivity::Call(tool_call) => MainConversationEvent::ToolCall {
                        call,
                        event: tool_call,
                    },
                    ToolActivity::Result(result) => MainConversationEvent::ToolResult(result),
                })
                .await;
            }
            EventTarget::Session {
                address, run_id, ..
            } => {
                let kind = match event {
                    ToolActivity::Call(tool_call) => SessionEventKind::ToolCall(tool_call),
                    ToolActivity::Result(result) => SessionEventKind::ToolResult(result),
                };
                residuum_bus::session::publish_session_event(self.publisher, address, run_id, kind)
                    .await;
            }
        }
    }

    async fn publish_intermediate(&self, content: &str, call: u32) {
        match self.target {
            EventTarget::Endpoint {
                output_endpoint,
                correlation_id,
                ..
            } => {
                self.publish_main(|turn_id| MainConversationEvent::Intermediate {
                    turn_id: turn_id.to_owned(),
                    call,
                    content: content.to_owned(),
                })
                .await;
                let output_endpoint = output_endpoint.current();
                if let Some(ep) = chat_interface_endpoint(output_endpoint.as_ref())
                    && let Err(e) = self
                        .publisher
                        .publish(
                            topics::Endpoint(ep.clone()),
                            residuum_bus::IntermediateEvent {
                                correlation_id: correlation_id.to_owned(),
                                content: content.to_owned(),
                            },
                        )
                        .await
                {
                    tracing::debug!(error = %e, "failed to publish intermediate text event");
                }
            }
            EventTarget::Session {
                address, run_id, ..
            } => {
                residuum_bus::session::publish_session_event(
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

    async fn publish_reply(&self, call: Option<u32>, content: &str) {
        let endpoint = match self.target {
            EventTarget::Endpoint {
                output_endpoint, ..
            } => output_endpoint
                .current()
                .map(|ep| ep.to_string())
                .unwrap_or_default(),
            EventTarget::Session { .. } => String::new(),
        };
        let for_endpoint = !endpoint.is_empty();
        self.publish_main(|turn_id| MainConversationEvent::Response {
            turn_id: turn_id.to_owned(),
            call,
            endpoint,
            content: content.to_owned(),
        })
        .await;
        if for_endpoint && let Some(audience) = self.reply_audience {
            audience.reply_published();
        }
    }

    async fn publish_thinking(&self, call: u32, content: String) {
        self.publish_main(|turn_id| MainConversationEvent::Thinking {
            turn_id: turn_id.to_owned(),
            call,
            content,
        })
        .await;
    }

    async fn publish_stream_piece(&self, call: u32, piece: StreamPiece) {
        self.publish_main(|turn_id| {
            let turn_id = turn_id.to_owned();
            match piece {
                StreamPiece::Text(text) => MainConversationEvent::TextDelta {
                    turn_id,
                    call,
                    text,
                },
                StreamPiece::Thinking(text) => MainConversationEvent::ThinkingDelta {
                    turn_id,
                    call,
                    text,
                },
                StreamPiece::Restart => MainConversationEvent::StreamRestart { turn_id, call },
            }
        })
        .await;
    }

    async fn publish_user_message(&self, message: &MessageEvent) {
        let EventTarget::Endpoint { correlation_id, .. } = self.target else {
            return;
        };
        let Some(event) = MainConversationEvent::user_message(message, correlation_id) else {
            return;
        };
        self.publish_main(|_| event).await;
    }

    pub async fn publish_message_read(&self, message: &AgentMessageEvent) {
        let EventTarget::Session {
            address,
            run_id,
            turn_id,
        } = self.target
        else {
            return;
        };
        residuum_bus::session::publish_session_event(
            self.publisher,
            address,
            run_id,
            message.read_event(turn_id),
        )
        .await;
    }

    async fn publish_usage(&self, turn: TurnUsage, session_totals: Option<SessionUsageTotals>) {
        match self.target {
            EventTarget::Endpoint { correlation_id, .. } => {
                let event = TurnUsageEvent {
                    correlation_id: correlation_id.to_owned(),
                    output_tokens: turn.output_tokens,
                    has_usage: turn.has_usage,
                    tool_calls: turn.tool_calls,
                    session_totals,
                };
                self.publish_main(|_| MainConversationEvent::TurnUsage(event))
                    .await;
            }
            EventTarget::Session {
                address, run_id, ..
            } => {
                residuum_bus::session::publish_session_event(
                    self.publisher,
                    address,
                    run_id,
                    SessionEventKind::TurnUsage {
                        output_tokens: turn.output_tokens,
                        has_usage: turn.has_usage,
                        tool_calls: turn.tool_calls,
                        session_totals,
                    },
                )
                .await;
            }
        }
    }
}

const MAX_EMPTY_RESPONSE_RETRIES: u32 = 2;

#[async_trait]
pub trait TranscriptSink: Send + Sync {
    async fn append(&self, messages: &[Message]);
}

pub struct TurnResources<'a> {
    pub provider: &'a dyn InferenceProvider,
    pub tools: &'a ToolRegistry,
    pub mcp_registry: &'a SharedMcpRegistry,
    pub identity: &'a IdentityFiles,
    pub options: &'a CompletionOptions,
    pub max_tool_iterations: Option<usize>,
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    pub stop_token: &'a CancellationToken,
    pub transcript_sink: Option<&'a dyn TranscriptSink>,
    pub usage_sink: Option<&'a dyn UsageSink>,
    pub hop_counter: &'a HopCounter,
}

const STOP_NOTE: &str = "[Stopped] the user stopped this turn before it finished; review what's already been done above before continuing or repeating any of it.";

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
            NoticeEvent::new(notice),
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

async fn notice_images_omitted(
    response: &InferenceResponse,
    turn_messages: &[Message],
    events: &EventContext<'_>,
    noticed: &mut bool,
) {
    let Some(omitted) = &response.images_omitted else {
        return;
    };
    let new_images: usize = turn_messages
        .iter()
        .map(|message| message.images.len())
        .sum();
    if new_images == 0 {
        tracing::debug!(
            model = %omitted.model,
            images = omitted.count,
            "left earlier turns' images out of a request to a model that can't take them"
        );
        return;
    }
    if std::mem::replace(noticed, true) {
        return;
    }
    tracing::warn!(
        model = %omitted.model,
        images = new_images,
        learned = omitted.learned,
        "left this turn's images out of a request to a model that can't take them"
    );
    if let Err(e) = events
        .publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent::new(images_omitted_notice(omitted, new_images)),
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish images-omitted notice");
    }
}

const THINKING_DROPPED_NOTICE: &str =
    "Some of the model's earlier reasoning couldn't be reused, so it continued without it.";

async fn notice_thinking_dropped(
    response: &InferenceResponse,
    events: &EventContext<'_>,
    noticed: &mut bool,
) {
    if response.thinking_dropped == 0 || std::mem::replace(noticed, true) {
        return;
    }
    tracing::warn!(
        blocks = response.thinking_dropped,
        "the provider dropped replayed reasoning because what preceded it in the request changed; the model continued without it"
    );
    if let Err(e) = events
        .publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent::new(THINKING_DROPPED_NOTICE),
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish dropped-reasoning notice");
    }
}

fn images_omitted_notice(omitted: &residuum_inference::ImagesOmitted, count: usize) -> String {
    let count = match count {
        1 => "1 image was".to_string(),
        n => format!("{n} images were"),
    };
    let notice = format!("This model can't see images, so {count} left out of the request.");
    if !omitted.learned {
        return notice;
    }
    format!(
        "{notice} Its provider turned images down, so they're left out until Residuum \
         restarts. To skip that failed first try, mark {} as text only under its provider in \
         Settings → Model.",
        omitted.model
    )
}

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

enum ModelCall {
    Completed {
        response: Box<InferenceResponse>,
        streamed: bool,
    },
    Stopped,
}

async fn sleep_until_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn publish_stream_pieces(
    events: &EventContext<'_>,
    call: u32,
    pieces: impl IntoIterator<Item = StreamPiece>,
    streamed: &mut bool,
) {
    for piece in pieces {
        *streamed = !matches!(piece, StreamPiece::Restart);
        events.publish_stream_piece(call, piece).await;
    }
}

#[derive(Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub options: CompletionOptions,
    pub(crate) layout: RequestLayout,
}

pub(crate) async fn build_model_request(
    identity: &IdentityFiles,
    recent_messages: &RecentMessages,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    tools: &ToolRegistry,
    mcp_registry: &SharedMcpRegistry,
    options: &CompletionOptions,
) -> ModelRequest {
    let mut tool_definitions = tools.definitions();
    let builtin_tools = tool_definitions.len();

    let mcp_guard = mcp_registry.read().await;
    tool_definitions.extend(mcp_guard.tool_definitions());
    drop(mcp_guard);

    let (messages, layout) =
        assemble_system_prompt(identity, recent_messages, memory_ctx, prompt_ctx);

    ModelRequest {
        messages,
        tools: tool_definitions,
        options: options.clone(),
        layout: RequestLayout {
            builtin_tools,
            ..layout
        },
    }
}

async fn call_model(
    resources: &TurnResources<'_>,
    events: &EventContext<'_>,
    request: &ModelRequest,
    call: u32,
) -> anyhow::Result<ModelCall> {
    let (channel_sink, mut deltas) = ChannelSink::channel();
    let sink: &dyn StreamSink = if events.streams_to_main() {
        &channel_sink
    } else {
        &DiscardSink
    };
    let completion = resources.provider.complete_streaming(
        &request.messages,
        &request.tools,
        &request.options,
        sink,
    );
    tokio::pin!(completion);

    let mut coalescer = DeltaCoalescer::default();
    let mut streamed = false;
    let outcome = loop {
        let flush_at = coalescer.flush_at();
        tokio::select! {
            biased;
            () = resources.stop_token.cancelled() => break None,
            Some(delta) = deltas.recv() => {
                let ready = coalescer.push(delta, tokio::time::Instant::now());
                publish_stream_pieces(events, call, ready, &mut streamed).await;
            }
            () = sleep_until_deadline(flush_at) => {
                publish_stream_pieces(events, call, coalescer.take_pending(), &mut streamed).await;
            }
            result = &mut completion => break Some(result),
        }
    };

    while let Ok(delta) = deltas.try_recv() {
        let ready = coalescer.push(delta, tokio::time::Instant::now());
        publish_stream_pieces(events, call, ready, &mut streamed).await;
    }
    publish_stream_pieces(events, call, coalescer.take_pending(), &mut streamed).await;

    match outcome {
        None => Ok(ModelCall::Stopped),
        Some(result) => {
            if let Some(usage_sink) = resources.usage_sink {
                usage_sink.record_call(resources.provider, &result).await;
            }
            Ok(ModelCall::Completed {
                response: Box::new(result.context("model completion failed")?),
                streamed,
            })
        }
    }
}

async fn retry_empty_response(
    empty_retries: &mut u32,
    streamed: bool,
    events: &EventContext<'_>,
    call: u32,
) -> anyhow::Result<()> {
    if streamed {
        events
            .publish_stream_piece(call, StreamPiece::Restart)
            .await;
    }
    if *empty_retries >= MAX_EMPTY_RESPONSE_RETRIES {
        anyhow::bail!("model returned empty response with no tool calls");
    }
    *empty_retries += 1;
    tracing::warn!(
        attempt = *empty_retries,
        max = MAX_EMPTY_RESPONSE_RETRIES,
        "model returned empty response, retrying"
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(())
}

async fn record_stop(recent_messages: &mut RecentMessages, resources: &TurnResources<'_>) {
    push_and_record(
        recent_messages,
        resources.transcript_sink,
        Message::system(STOP_NOTE),
    )
    .await;
}

async fn settle_response(response: &mut InferenceResponse, events: &EventContext<'_>, call: u32) {
    if !response.thinking.is_empty() {
        tracing::debug!(
            thinking_len = response
                .thinking
                .iter()
                .map(|block| block.text.len())
                .sum::<usize>(),
            "structured thinking received"
        );
    }
    response.content = super::think_tags::strip_think_tags(&response.content);

    let empty = response.tool_calls.is_empty() && response.content.is_empty();
    if !empty && let Some(reasoning) = joined_thinking_text(&response.thinking) {
        events.publish_thinking(call, reasoning).await;
    }
}

async fn finish_turn_with_notice(
    notice: String,
    recent_messages: &mut RecentMessages,
    resources: &TurnResources<'_>,
    events: &EventContext<'_>,
) -> anyhow::Result<Vec<String>> {
    let final_message = Message::assistant(notice.clone(), None);
    push_and_record(recent_messages, resources.transcript_sink, final_message).await;
    events.publish_reply(None, &notice).await;
    Ok(vec![notice])
}

async fn request_model_response(
    resources: &TurnResources<'_>,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    call: u32,
) -> anyhow::Result<Option<(InferenceResponse, bool)>> {
    let request = build_model_request(
        resources.identity,
        recent_messages,
        memory_ctx,
        prompt_ctx,
        resources.tools,
        resources.mcp_registry,
        resources.options,
    )
    .await;
    recent_messages.watch_request(&request, call > 0);

    match call_model(resources, events, &request, call).await? {
        ModelCall::Completed { response, streamed } => Ok(Some((*response, streamed))),
        ModelCall::Stopped => {
            tracing::info!(iterations = call, "turn stopped by user during model call");
            record_stop(recent_messages, resources).await;
            Ok(None)
        }
    }
}

#[tracing::instrument(skip_all, fields(operation = "execute_turn"))]
#[expect(
    clippy::too_many_arguments,
    reason = "turn execution context spans resources, contexts, and watchers"
)]
pub async fn execute_turn(
    turn_start: usize,
    resources: &TurnResources<'_>,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    mut status_clock: Option<&mut StatusClock>,
    interrupt_rx: &mut dyn InterruptSource,
    subconscious: Option<&crate::subconscious::SubconsciousWatch>,
) -> anyhow::Result<Vec<String>> {
    let mut texts: Vec<String> = Vec::new();
    let mut empty_retries: u32 = 0;
    let mut turn_usage = TurnUsage::default();
    let mut repeat_guard = RepeatCallGuard::new();
    let mut images_noticed = false;
    let mut thinking_drop_noticed = false;

    let mut iteration: usize = 0;
    loop {
        if check_tool_iteration_limit(resources, recent_messages, events, iteration, &mut texts)
            .await
        {
            return Ok(texts);
        }

        if check_interrupts_and_stop(
            interrupt_rx,
            recent_messages,
            status_clock.as_deref_mut(),
            resources,
            events,
            iteration,
        )
        .await
        {
            return Ok(texts);
        }

        let call = u32::try_from(iteration).unwrap_or(u32::MAX);

        let Some((mut response, streamed)) = request_model_response(
            resources,
            memory_ctx,
            prompt_ctx,
            recent_messages,
            events,
            call,
        )
        .await?
        else {
            return Ok(texts);
        };
        settle_response(&mut response, events, call).await;
        notice_images_omitted(
            &response,
            recent_messages.messages_since(turn_start),
            events,
            &mut images_noticed,
        )
        .await;
        notice_thinking_dropped(&response, events, &mut thinking_drop_noticed).await;

        if response.was_truncated() {
            handle_output_truncated(&response, resources, events, recent_messages).await;
        }

        if response.tool_calls.is_empty() {
            log_usage(&response);
            update_and_publish_usage(&response, 0, &mut turn_usage, resources.usage_sink, events)
                .await;
            if response.content.is_empty() {
                retry_empty_response(&mut empty_retries, streamed, events, call).await?;
                continue;
            }
            tracing::debug!(iterations = iteration, "turn complete");
            let final_message =
                Message::assistant(response.content.clone(), None).with_thinking(response.thinking);
            push_and_record(recent_messages, resources.transcript_sink, final_message).await;
            events.publish_reply(Some(call), &response.content).await;
            texts.push(response.content);
            return Ok(texts);
        }

        tracing::debug!(
            iteration,
            tool_count = response.tool_calls.len(),
            "processing tool calls"
        );

        let notice = process_tool_call_batch(
            &response,
            recent_messages,
            &mut repeat_guard,
            ToolCallResponseContext {
                resources,
                events,
                subconscious,
                iteration,
                call,
                turn_start,
            },
            &mut turn_usage,
        )
        .await;

        if let Some(notice) = notice {
            return finish_turn_with_notice(notice, recent_messages, resources, events).await;
        }
        iteration += 1;
    }
}

async fn process_tool_call_batch(
    response: &InferenceResponse,
    recent_messages: &mut RecentMessages,
    repeat_guard: &mut RepeatCallGuard,
    ctx: ToolCallResponseContext<'_>,
    turn_usage: &mut TurnUsage,
) -> Option<String> {
    let usage_sink = ctx.resources.usage_sink;
    let events = ctx.events;
    let (tool_calls_executed, notice) =
        handle_tool_call_response(response, recent_messages, repeat_guard, ctx).await;

    log_usage(response);
    update_and_publish_usage(
        response,
        tool_calls_executed,
        turn_usage,
        usage_sink,
        events,
    )
    .await;

    notice
}

async fn check_tool_iteration_limit(
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
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
         config.toml, or in Settings → Advanced → Runtime) to allow longer turns."
    );
    let final_message = Message::assistant(notice.clone(), None);
    push_and_record(recent_messages, resources.transcript_sink, final_message).await;
    events.publish_reply(None, &notice).await;
    texts.push(notice);
    true
}

struct ToolCallResponseContext<'a> {
    resources: &'a TurnResources<'a>,
    events: &'a EventContext<'a>,
    subconscious: Option<&'a crate::subconscious::SubconsciousWatch>,
    iteration: usize,
    call: u32,
    turn_start: usize,
}

async fn handle_tool_call_response(
    response: &InferenceResponse,
    recent_messages: &mut RecentMessages,
    repeat_guard: &mut RepeatCallGuard,
    ctx: ToolCallResponseContext<'_>,
) -> (u32, Option<String>) {
    if !response.content.is_empty() {
        ctx.events
            .publish_intermediate(&response.content, ctx.call)
            .await;
    }

    let tool_calls =
        annotate_tool_call_servers(&response.tool_calls, ctx.resources.mcp_registry).await;

    let msg = Message::assistant(response.content.clone(), Some(tool_calls.clone()))
        .with_thinking(response.thinking.clone());
    push_and_record(recent_messages, ctx.resources.transcript_sink, msg).await;

    if let Some(watch) = ctx.subconscious {
        watch.maybe_spawn(
            ctx.iteration,
            recent_messages.messages_since(ctx.turn_start).to_vec(),
        );
    }

    let (executed, stopped) = run_tool_call_batch(
        &tool_calls,
        ctx.resources,
        recent_messages,
        ctx.events,
        repeat_guard,
        ctx.call,
    )
    .await;

    let Some((tool_name, count)) = stopped else {
        return (executed, None);
    };

    tracing::warn!(
        tool_name = %tool_name,
        consecutive = count,
        "turn stopped: model repeated the same tool call too many times in a row"
    );
    (
        executed,
        Some(format!(
            "I stopped this turn because I called `{tool_name}` with the exact same arguments \
             {count} times in a row — the result can't change by calling it again. Adjust \
             `repeat_call_stop_after` under `[agent]` in your Residuum config (or in Settings → Advanced → Runtime) if \
             this is expected, or let me know what you'd like me to try instead."
        )),
    )
}

async fn check_interrupts_and_stop(
    interrupt_rx: &mut dyn InterruptSource,
    recent_messages: &mut RecentMessages,
    status_clock: Option<&mut StatusClock>,
    resources: &TurnResources<'_>,
    events: &EventContext<'_>,
    iteration: usize,
) -> bool {
    let mut stopped = drain_interrupts(
        interrupt_rx,
        recent_messages,
        status_clock,
        resources.transcript_sink,
        resources.hop_counter,
        events,
    )
    .await;

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

async fn drain_interrupts(
    interrupt_rx: &mut dyn InterruptSource,
    recent_messages: &mut RecentMessages,
    mut status_clock: Option<&mut StatusClock>,
    sink: Option<&dyn TranscriptSink>,
    hop_counter: &HopCounter,
    events: &EventContext<'_>,
) -> bool {
    let mut stopped = false;
    while let Ok(interrupt) = interrupt_rx.try_recv() {
        match interrupt {
            Interrupt::UserMessage(msg) => {
                tracing::info!(msg_id = %msg.id, "injecting mid-turn user message");
                events.publish_user_message(&msg).await;
                let entries = match status_clock.as_deref_mut() {
                    Some(clock) => clock.admit(msg),
                    None => msg.into_history_messages(),
                };
                push_and_record_many(recent_messages, sink, entries).await;
            }
            Interrupt::AgentMessage(msg) => {
                tracing::info!(
                    from = %msg.from,
                    category = %msg.from_category,
                    hop_count = msg.hop_count,
                    "injecting agent message mid-turn"
                );
                hop_counter.bump(msg.hop_count);
                let mut entry = msg.to_history_message();
                if let Some(clock) = status_clock.as_deref_mut() {
                    clock.stamp(&mut entry, None);
                }
                push_and_record(recent_messages, sink, entry).await;
                events.publish_message_read(&msg).await;
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

async fn annotate_tool_call_servers(
    tool_calls: &[ToolCall],
    mcp_registry: &SharedMcpRegistry,
) -> Vec<ToolCall> {
    if tool_calls.is_empty() {
        return Vec::new();
    }
    let registry = mcp_registry.read().await;
    tool_calls
        .iter()
        .cloned()
        .map(|tc| ToolCall {
            server: registry.server_name_for_tool(&tc.name),
            ..tc
        })
        .collect()
}

async fn run_tool_call_batch(
    tool_calls: &[ToolCall],
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    repeat_guard: &mut RepeatCallGuard,
    call: u32,
) -> (u32, Option<(String, u32)>) {
    let guard_cfg = resources.repeat_call_guard;
    let mut stopped_by_repeat_guard = None;
    let mut skipped = 0_usize;
    let mut executed: u32 = 0;
    for tool_call in tool_calls {
        if resources.stop_token.is_cancelled() || stopped_by_repeat_guard.is_some() {
            skip_tool_call(tool_call, resources, recent_messages, events, call).await;
            skipped += 1;
            continue;
        }

        if guard_cfg.enabled {
            let consecutive = repeat_guard.record(&tool_call.name, &tool_call.arguments);
            if consecutive >= guard_cfg.stop_after {
                stop_repeated_tool_call(
                    tool_call,
                    consecutive,
                    resources,
                    recent_messages,
                    events,
                    call,
                )
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
                execute_tool(
                    tool_call,
                    resources,
                    recent_messages,
                    events,
                    call,
                    Some(note),
                )
                .await;
                executed += 1;
                continue;
            }
        }

        execute_tool(tool_call, resources, recent_messages, events, call, None).await;
        executed += 1;
    }
    if skipped > 0 {
        tracing::info!(
            skipped,
            total = tool_calls.len(),
            "skipped remaining tool calls after the turn was stopped"
        );
    }
    (executed, stopped_by_repeat_guard)
}

async fn dispatch_tool_call(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
) -> (Result<ToolResult, ToolError>, bool) {
    if !is_plausible_tool_name(&tool_call.name) {
        return (
            Err(ToolError::MalformedName(truncate_tool_name_for_display(
                &tool_call.name,
            ))),
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
        Err(ToolError::NotFound(_)) => {
            tracing::debug!(tool_name = %tool_call.name, "tool not found in built-in registry, falling back to MCP");
            (execute_mcp_tool(tool_call, resources).await, true)
        }
        other => (other, false),
    }
}

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

async fn run_dispatched_tool_call(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
) -> (String, bool, Vec<residuum_inference::ImageData>) {
    let (result, used_mcp) = dispatch_tool_call(tool_call, resources).await;
    match result {
        Ok(r) => (r.output, r.is_error, r.images),
        Err(e) => {
            let source = match (&e, used_mcp) {
                (ToolError::MalformedName(_), _) => "validation",
                (_, true) => "mcp",
                (_, false) => "built-in",
            };
            tracing::warn!(
                error = %e,
                tool_name = %truncate_tool_name_for_display(&tool_call.name),
                tool_call_id = %tool_call.id,
                source,
                "tool execution failed"
            );
            (e.to_string(), true, vec![])
        }
    }
}

#[tracing::instrument(skip_all, fields(tool.name = %tool_call.name, tool.id = %tool_call.id))]
async fn execute_tool(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    call: u32,
    steering_note: Option<String>,
) {
    events
        .publish_tool_activity(
            ToolActivity::Call(ToolCallEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
                server: tool_call.server.clone(),
            }),
            call,
        )
        .await;

    let auto_mode = match resources.tools.auto_mode() {
        Some(gate) => {
            let request = crate::auto_mode::latest_user_request(recent_messages.messages());
            gate.check(tool_call, request).await
        }
        None => None,
    };
    let blocked = auto_mode
        .as_ref()
        .filter(|v| v.decision == residuum_bus::auto_mode::AutoModeDecision::Blocked);

    let (mut output, is_error, images) = if let Some(verdict) = blocked {
        (verdict.blocked_message(), true, vec![])
    } else {
        run_dispatched_tool_call(tool_call, resources).await
    };

    if resources.stop_token.is_cancelled() {
        tracing::info!(
            tool_name = %tool_call.name,
            tool_call_id = %tool_call.id,
            "tool call interrupted: the turn was stopped while it was running"
        );
    }

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

    if let Some(store) = resources.tools.result_store() {
        output = store
            .keep_for_history(output, &tool_call.name, &tool_call.id)
            .await;
    }

    if let Some(note) = steering_note {
        output.push_str("\n\n");
        output.push_str(&note);
    }

    events
        .publish_tool_activity(
            ToolActivity::Result(ToolResultEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                output: output.clone(),
                is_error,
                auto_mode,
            }),
            call,
        )
        .await;

    let tool_message = if images.is_empty() {
        Message::tool(output, tool_call.id.clone())
    } else {
        Message::tool_with_images(output, tool_call.id.clone(), images)
    };
    push_and_record(recent_messages, resources.transcript_sink, tool_message).await;
}

async fn record_cancelled_tool_call(
    tool_call: &ToolCall,
    output: String,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    call: u32,
) {
    events
        .publish_tool_activity(
            ToolActivity::Call(ToolCallEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
                server: tool_call.server.clone(),
            }),
            call,
        )
        .await;

    let result = ToolResult::cancelled(output);

    events
        .publish_tool_activity(
            ToolActivity::Result(ToolResultEvent {
                correlation_id: events.correlation_id().to_owned(),
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                output: result.output.clone(),
                is_error: result.is_error,
                auto_mode: None,
            }),
            call,
        )
        .await;

    let tool_message = Message::tool(result.output, tool_call.id.clone());
    push_and_record(recent_messages, resources.transcript_sink, tool_message).await;
}

async fn skip_tool_call(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    call: u32,
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
        call,
    )
    .await;
}

async fn stop_repeated_tool_call(
    tool_call: &ToolCall,
    consecutive: u32,
    resources: &TurnResources<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    call: u32,
) {
    let output = format!(
        "cancelled: this exact call (same tool, same arguments) was made {consecutive} times \
         in a row; the turn was stopped instead of running it again because the result cannot \
         change — try a different approach or report back instead"
    );
    record_cancelled_tool_call(tool_call, output, resources, recent_messages, events, call).await;
}

fn log_usage(response: &InferenceResponse) {
    if let Some(usage) = response.usage {
        tracing::debug!(
            uncached_input_tokens = usage.uncached_input_tokens,
            cache_read_tokens = usage.cache_read_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            cache_write_long_tokens = usage.cache_write_long_tokens,
            output_tokens = usage.output_tokens,
            reasoning_tokens = usage.reasoning_tokens,
            "token usage"
        );
    } else {
        tracing::debug!("token usage not available in response");
    }
}

async fn update_and_publish_usage(
    response: &InferenceResponse,
    tool_calls_executed: u32,
    turn_usage: &mut TurnUsage,
    usage_sink: Option<&dyn UsageSink>,
    events: &EventContext<'_>,
) {
    turn_usage.accumulate(response.usage);
    turn_usage.record_tool_calls(tool_calls_executed);
    let mut session_totals = match usage_sink {
        Some(sink) => Some(sink.accumulate(response.usage).await),
        None => None,
    };
    if tool_calls_executed > 0
        && let Some(sink) = usage_sink
    {
        session_totals = Some(sink.record_tool_calls(tool_calls_executed).await);
    }
    events.publish_usage(*turn_usage, session_totals).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{BlockingTool, CountingTool, FixedOutputTool};
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;
    use residuum_inference::Role;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    #[tokio::test]
    async fn the_built_request_carries_everything_a_provider_caches_on() {
        let mut recent_messages = RecentMessages::new();
        let mut stamped = Message::user("hi");
        stamped.status_line = Some("[Received: Mon 9:00]".to_string());
        recent_messages.push(stamped);
        let options = CompletionOptions {
            cache: residuum_inference::CacheHints {
                conversation_key: Some("main-conversation".to_string()),
                retention: residuum_inference::CacheRetention::Long,
            },
            ..CompletionOptions::default()
        };
        let inputs = crate::testing::RequestInputs {
            recent_messages,
            options,
            ..crate::testing::RequestInputs::default()
        };

        let request = crate::testing::render_model_request(&inputs, &ToolRegistry::new()).await;

        assert_eq!(
            request.options.cache, inputs.options.cache,
            "the call is sent the turn's cache hints"
        );
        let system = request.messages.first().unwrap();
        assert_eq!(system.role, Role::System);
        assert!(
            system.cache_boundary.is_some(),
            "the system prompt carries its cache boundary"
        );
        let rest: Vec<(Role, &str)> = request
            .messages
            .iter()
            .skip(1)
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        assert_eq!(
            rest,
            [(Role::System, "[Received: Mon 9:00]"), (Role::User, "hi")],
            "the stored status line goes just ahead of its message"
        );
    }

    async fn request_with_mcp_servers(registry: &SharedMcpRegistry) -> ModelRequest {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(crate::testing::EchoTool));
        let inputs = crate::testing::RequestInputs {
            mcp: Some(Arc::clone(registry)),
            ..crate::testing::RequestInputs::default()
        };
        crate::testing::render_model_request(&inputs, &tools).await
    }

    fn cached_bytes(request: &ModelRequest) -> (String, String) {
        (
            serde_json::to_string(&request.tools).unwrap(),
            serde_json::to_string(request.messages.first().unwrap()).unwrap(),
        )
    }

    fn mcp_tools(server: &str, names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| ToolDefinition {
                name: (*name).to_string(),
                description: format!("{name}, from {server}"),
                parameters: serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            })
            .collect()
    }

    async fn connect_in_order(servers: &[(&str, &[&str])]) -> SharedMcpRegistry {
        let registry = residuum_mcp::McpRegistry::new_shared();
        for (server, names) in servers {
            registry
                .write()
                .await
                .insert_running_server(server, mcp_tools(server, names));
        }
        registry
    }

    #[tokio::test]
    async fn mcp_tools_reach_the_request_grouped_by_server_in_name_order() {
        let zeta_first = connect_in_order(&[
            ("zeta", &["page", "annotate"]),
            ("bravo", &["ping", "count"]),
            ("mike", &["list"]),
            ("alpha", &["search", "fetch"]),
            ("delta", &["lookup"]),
        ])
        .await;
        let mike_first = connect_in_order(&[
            ("mike", &["list"]),
            ("delta", &["lookup"]),
            ("zeta", &["page", "annotate"]),
            ("alpha", &["search", "fetch"]),
            ("bravo", &["ping", "count"]),
        ])
        .await;

        let request = request_with_mcp_servers(&zeta_first).await;
        let names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "echo", "fetch", "search", "count", "ping", "lookup", "list", "annotate", "page",
            ],
            "the built-in tool first, then each server's tools in name order, servers in name order"
        );
        assert_eq!(
            cached_bytes(&request),
            cached_bytes(&request_with_mcp_servers(&mike_first).await),
            "the tools and system prompt don't depend on the order servers connected in"
        );
    }

    #[tokio::test]
    async fn an_mcp_server_reconnecting_unchanged_leaves_the_request_prefix_alone() {
        let registry = connect_in_order(&[("alpha", &["a_search"]), ("zeta", &["z_lookup"])]).await;
        let before = cached_bytes(&request_with_mcp_servers(&registry).await);

        registry
            .write()
            .await
            .insert_running_server("alpha", mcp_tools("alpha", &["a_search"]));

        assert_eq!(
            cached_bytes(&request_with_mcp_servers(&registry).await),
            before,
            "an unchanged server keeps its tools' bytes and position"
        );
    }

    #[derive(Default)]
    struct CountingAudience(AtomicUsize);

    impl ReplyAudience for CountingAudience {
        fn reply_published(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn the_reply_audience_hears_each_endpoint_reply_as_it_is_published() {
        let bus = residuum_bus::spawn_broker();
        let publisher = bus.publisher();
        let mut main = bus.subscribe(topics::MainConversation).await.unwrap();
        let audience = CountingAudience::default();
        let ws = EndpointName::from("ws");
        let to_endpoint = EventContext {
            publisher: &publisher,
            reply_audience: Some(&audience),
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ws)),
                correlation_id: "t1",
            },
            session_conversation: None,
        };
        to_endpoint.publish_reply(Some(0), "hello").await;
        assert_eq!(audience.0.load(Ordering::SeqCst), 1);

        let nowhere = EventContext {
            publisher: &publisher,
            reply_audience: Some(&audience),
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "t2",
            },
            session_conversation: None,
        };
        nowhere.publish_reply(None, "a note").await;
        assert_eq!(
            audience.0.load(Ordering::SeqCst),
            1,
            "a reply bound for no endpoint is not one to count"
        );

        for want in ["hello", "a note"] {
            let event = next_event("a reply in the main conversation", &mut main).await;
            assert!(
                matches!(&event, MainConversationEvent::Response { content, .. } if content == want),
                "{event:?}"
            );
        }
    }

    fn silent_events(publisher: &Publisher) -> EventContext<'_> {
        EventContext {
            publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "turn-1",
            },
            session_conversation: None,
        }
    }

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
        let stopped = drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            Some(&sink),
            &HopCounter::new(0),
            &silent_events(&Publisher::noop()),
        )
        .await;

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
        let stopped = drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            Some(&sink),
            &HopCounter::new(0),
            &silent_events(&Publisher::noop()),
        )
        .await;

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

        let stopped = drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            None,
            &HopCounter::new(0),
            &silent_events(&Publisher::noop()),
        )
        .await;

        assert!(!stopped, "an empty channel should never report a stop");
        assert_eq!(recent.len(), 0, "nothing should be injected");
    }

    #[tokio::test]
    async fn drain_injects_user_message_history_through_the_sink() {
        use residuum_bus::MessageEvent;
        use residuum_bus::origin::MessageOrigin;

        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();

        let inbound = MessageEvent {
            id: "msg-1".to_string(),
            content: "hello mid-turn".to_string(),
            origin: MessageOrigin {
                endpoint: "test".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        };
        tx.send(Interrupt::UserMessage(inbound)).ok();
        let stopped = drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            Some(&sink),
            &HopCounter::new(0),
            &silent_events(&Publisher::noop()),
        )
        .await;

        assert!(!stopped);
        assert!(!recent.messages().is_empty());
        assert_eq!(
            contents(&sink.recorded.lock().unwrap()),
            contents(recent.messages()),
            "every history message an injected user message expands into must reach the sink"
        );
    }

    #[tokio::test]
    async fn drain_announces_an_agent_message_a_session_reads_mid_turn() {
        let bus = residuum_bus::spawn_broker();
        let publisher = bus.publisher();
        let mut sessions: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        let sink = MockSink::default();
        let address = SessionAddress::from("spawned-x-0001");
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Session {
                address: &address,
                run_id: "run-1",
                turn_id: "run-1-t2",
            },
            session_conversation: None,
        };

        let owner_message = AgentMessageEvent::new(
            SessionAddress::from(residuum_bus::session::OWNER_ADDRESS),
            "owner",
            "check the logs too",
            0,
        )
        .with_id("m-owner-1");
        tx.send(Interrupt::AgentMessage(owner_message)).ok();
        let stopped = drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            Some(&sink),
            &HopCounter::new(0),
            &events,
        )
        .await;
        assert!(!stopped);

        let recorded = sink.recorded.lock().unwrap().clone();
        assert_eq!(
            recorded.first().and_then(|m| m.id.as_deref()),
            Some("m-owner-1"),
            "the transcript entry carries the message's id"
        );

        let event = next_event("the session's read of the message", &mut sessions).await;
        assert_eq!(event.address, address);
        assert_eq!(event.run_id, "run-1");
        let SessionEventKind::MessageRead {
            id,
            turn_id,
            content,
            from,
            from_category,
        } = event.kind
        else {
            panic!("expected a MessageRead, got {:?}", event.kind);
        };
        assert_eq!(id, "m-owner-1");
        assert_eq!(turn_id, "run-1-t2", "the read names the turn in flight");
        assert_eq!(content, "check the logs too", "the body as sent, no header");
        assert_eq!(from, residuum_bus::session::OWNER_ADDRESS);
        assert_eq!(from_category, "owner");
    }

    #[tokio::test]
    async fn drain_announces_no_read_for_a_main_turn() {
        let bus = residuum_bus::spawn_broker();
        let publisher = bus.publisher();
        let mut sessions: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();

        tx.send(Interrupt::AgentMessage(AgentMessageEvent::new(
            SessionAddress::from("spawned-y-0001"),
            "spawned",
            "status?",
            0,
        )))
        .ok();
        drain_interrupts(
            &mut rx,
            &mut recent,
            None,
            None,
            &HopCounter::new(0),
            &silent_events(&publisher),
        )
        .await;

        assert_eq!(recent.len(), 1, "the message still joins the turn");
        bus_barrier(&bus).await;
        assert!(
            sessions.drain().is_empty(),
            "a main turn has no session stream to announce a read on"
        );
    }

    fn contents(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.content.as_str()).collect()
    }

    #[tokio::test]
    async fn publish_intermediate_for_a_conversation_session_also_uses_session_response_event() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("discord");
        let mut conv_sub: residuum_bus::Subscriber<SessionResponseEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let mut intermediate_sub: residuum_bus::Subscriber<residuum_bus::IntermediateEvent> =
            bus_handle
                .subscribe(topics::Endpoint(ep.clone()))
                .await
                .unwrap();
        let mut session_sub: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();

        let address = SessionAddress::from("external-discord-chan-1");
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Session {
                address: &address,
                run_id: "run-1",
                turn_id: "run-1-t1",
            },
            session_conversation: Some(SessionConversationTarget {
                session_address: &address,
                endpoint: "discord",
                conversation_id: "chan-1",
            }),
        };

        events
            .publish_intermediate("checking the build now", 0)
            .await;

        let event = next_event("a SessionResponseEvent to be published", &mut conv_sub).await;
        assert_eq!(event.content, "checking the build now");
        assert_eq!(event.session_address, address);
        assert_eq!(event.conversation_id, "chan-1");
        assert!(event.attachment.is_none());
        assert!(
            !event.is_final,
            "intermediate turn text must not be marked as the run's final output"
        );

        bus_barrier(&bus_handle).await;
        assert!(
            intermediate_sub.drain().is_empty(),
            "a conversation session's intermediate text must never publish IntermediateEvent, \
             which would route through main's correlation-id target lookup and log the \
             misleading \"owner has not messaged the bot yet\" line"
        );

        let session_event =
            next_event("the session-stream event to be published", &mut session_sub).await;
        assert_eq!(session_event.address, address);
        assert_eq!(session_event.run_id, "run-1");
        assert!(
            matches!(
                session_event.kind,
                residuum_bus::SessionEventKind::Intermediate { ref content } if content == "checking the build now"
            ),
            "a conversation session's intermediate text must reach the web sessions stream too"
        );
    }

    #[tokio::test]
    async fn publish_intermediate_without_a_session_conversation_uses_intermediate_event() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("telegram");
        let mut sub: residuum_bus::Subscriber<residuum_bus::IntermediateEvent> = bus_handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        events.publish_intermediate("thinking...", 0).await;

        let event = next_event("an IntermediateEvent to be published", &mut sub).await;
        assert_eq!(event.content, "thinking...");
        assert_eq!(event.correlation_id, "corr-1");
    }

    #[tokio::test]
    async fn publish_intermediate_for_the_web_ui_reaches_only_the_main_conversation() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from(residuum_config::WEB_UI_ENDPOINT);
        let mut endpoint_sub: residuum_bus::Subscriber<residuum_bus::IntermediateEvent> =
            bus_handle
                .subscribe(topics::Endpoint(ep.clone()))
                .await
                .unwrap();
        let mut main_sub: residuum_bus::Subscriber<MainConversationEvent> = bus_handle
            .subscribe(topics::MainConversation)
            .await
            .unwrap();

        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        events.publish_intermediate("thinking...", 0).await;

        let event = next_event("the main conversation to get the text", &mut main_sub).await;
        assert!(
            matches!(
                &event,
                MainConversationEvent::Intermediate { turn_id, call: 0, content }
                    if turn_id == "corr-1" && content == "thinking..."
            ),
            "{event:?}"
        );
        bus_barrier(&bus_handle).await;
        assert!(
            endpoint_sub.drain().is_empty(),
            "the web UI's endpoint topic has no subscriber for a turn's intermediate text"
        );
    }

    #[tokio::test]
    async fn publish_intermediate_with_no_output_endpoint_is_a_noop() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: residuum_bus::Subscriber<residuum_bus::IntermediateEvent> =
            bus_handle.subscribe(topics::Endpoint(ep)).await.unwrap();

        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        events.publish_intermediate("nothing to see", 0).await;

        bus_barrier(&bus_handle).await;
        assert!(
            sub.drain().is_empty(),
            "with no output endpoint there is nowhere to publish intermediate text"
        );
    }

    #[tokio::test]
    async fn execute_tool_rejects_malformed_name_without_registry_lookup() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "write_file\tcontent</arg_key><arg_value># Hello".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };

        let provider = residuum_inference::NullProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };

        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, 0, None).await;

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

    #[tokio::test]
    async fn execute_tool_redacts_agent_key_values_before_recording() {
        let dir = tempfile::tempdir().unwrap();
        let keys = residuum_config::agent_keys::AgentKeys::new_shared(dir.path());
        keys.set(
            "api_key",
            "sk-live-redactme42",
            None,
            residuum_config::agent_keys::KeyCreator::User,
        )
        .await
        .unwrap();

        let mut tools = ToolRegistry::new();
        tools.set_agent_keys(keys);
        tools.register(Box::new(FixedOutputTool::new(
            "Authorization: Bearer sk-live-redactme42",
        )));

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "fixed_output".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, 0, None).await;

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

    async fn record_one_tool_call(
        tools: &ToolRegistry,
        mcp_registry: &SharedMcpRegistry,
        name: &str,
    ) -> String {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };
        let provider = residuum_inference::NullProvider;
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let resources = TurnResources {
            provider: &provider,
            tools,
            mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, 0, None).await;
        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool);
        msg.content.clone()
    }

    fn only_saved_result(store: &crate::ToolResultStore) -> std::path::PathBuf {
        let files: Vec<_> = std::fs::read_dir(store.dir())
            .expect("the store's directory should exist once a result is saved")
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "exactly one saved result: {files:?}");
        files.into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn execute_tool_saves_an_oversized_built_in_result_and_keeps_an_excerpt() {
        let dir = tempfile::tempdir().unwrap();
        let keys = residuum_config::agent_keys::AgentKeys::new_shared(dir.path().join("keys"));
        keys.set(
            "api_key",
            "sk-live-redactme42",
            None,
            residuum_config::agent_keys::KeyCreator::User,
        )
        .await
        .unwrap();
        let body = {
            use std::fmt::Write as _;
            (1..=5_000).fold(String::new(), |mut text, n| {
                _ = writeln!(text, "row {n} of the dump");
                text
            })
        };
        let raw = format!("HEAD-MARK sk-live-redactme42\n{body}TAIL-MARK sk-live-redactme42");
        let store = crate::ToolResultStore::new(dir.path().join("tool-results"), "scout");

        let mut tools = ToolRegistry::new();
        tools.set_agent_keys(keys);
        tools.set_result_store(store.clone());
        tools.register(Box::new(FixedOutputTool::new(raw)));
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();

        let content = record_one_tool_call(&tools, &mcp_registry, "fixed_output").await;

        let saved = only_saved_result(&store);
        let full = std::fs::read_to_string(&saved).unwrap();
        let redacted =
            format!("HEAD-MARK [agent-key:api_key]\n{body}TAIL-MARK [agent-key:api_key]");
        assert_eq!(full, redacted, "the file holds the full redacted result");

        assert!(
            content.chars().count() < crate::HISTORY_RESULT_MAX_CHARS,
            "history keeps an excerpt, not the whole result ({} chars)",
            content.chars().count()
        );
        assert!(
            content.starts_with("HEAD-MARK [agent-key:api_key]"),
            "{content}"
        );
        assert!(
            content.ends_with("TAIL-MARK [agent-key:api_key]"),
            "{content}"
        );
        assert!(!content.contains("sk-live-redactme42"));
        assert!(
            content.contains(&format!("saved at {}", saved.display())),
            "the excerpt names the saved file: {content}"
        );
        assert!(
            content.contains("read_file starting at offset"),
            "{content}"
        );
    }

    #[tokio::test]
    async fn execute_tool_keeps_a_result_under_the_threshold_whole() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::ToolResultStore::new(dir.path(), "scout");
        let output = "a modest result\n".repeat(100);

        let mut tools = ToolRegistry::new();
        tools.set_result_store(store.clone());
        tools.register(Box::new(FixedOutputTool::new(output.clone())));
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();

        let content = record_one_tool_call(&tools, &mcp_registry, "fixed_output").await;

        assert_eq!(content, output, "a short result is recorded byte for byte");
        assert!(
            !store.dir().exists(),
            "nothing is saved for a result under the threshold"
        );
    }

    #[cfg(unix)]
    const BIG_OUTPUT_MCP_SERVER: &str = r#"
big=$(head -c 40000 /dev/zero | tr '\000' x)
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | grep -o '"id":[0-9]*' | head -n 1 | cut -d: -f2)
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"big","version":"1.0.0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"big_output","description":"returns a large result","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"MCP-HEAD %s MCP-TAIL"}],"isError":false}}\n' "$id" "$big" ;;
  esac
done
"#;

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_tool_saves_an_oversized_mcp_result_and_keeps_an_excerpt() {
        let entry = residuum_mcp::McpServerEntry {
            name: "big".to_string(),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), BIG_OUTPUT_MCP_SERVER.to_string()],
            env: std::collections::BTreeMap::new(),
            transport: residuum_mcp::McpTransport::Stdio,
            headers: std::collections::BTreeMap::new(),
            timeout_secs: None,
        };
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
        let report = wait::guarded("the fake MCP server to connect", async {
            mcp_registry
                .write()
                .await
                .reconcile_and_connect(&[entry])
                .await
        })
        .await;
        assert!(
            report.failures.is_empty(),
            "the fake MCP server should connect: {:?}",
            report.failures
        );

        let dir = tempfile::tempdir().unwrap();
        let store = crate::ToolResultStore::new(dir.path(), "scout");
        let mut tools = ToolRegistry::new();
        tools.set_result_store(store.clone());

        let content = wait::guarded(
            "the MCP tool call to be recorded",
            record_one_tool_call(&tools, &mcp_registry, "big_output"),
        )
        .await;

        let full = std::fs::read_to_string(only_saved_result(&store)).unwrap();
        assert_eq!(
            full,
            format!("MCP-HEAD {} MCP-TAIL", "x".repeat(40_000)),
            "the file holds the MCP server's whole result"
        );
        assert!(content.starts_with("MCP-HEAD"), "{content}");
        assert!(content.ends_with("MCP-TAIL"), "{content}");
        assert!(
            content.chars().count() < crate::HISTORY_RESULT_MAX_CHARS,
            "history keeps an excerpt of the MCP result"
        );

        mcp_registry.write().await.disconnect_all().await;
    }

    async fn run_counting_tool_under_auto_mode(probability: Option<f64>) -> (usize, String) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let usage_dir = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        if let Some(p) = probability {
            Mock::given(method("POST"))
                .and(path("/v1/systemone"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "model": "nimble",
                    "answers": { "deny_0": { "type": "noul", "noul": p } },
                    "usage": { "input_tokens": 30, "output_tokens": 1 }
                })))
                .mount(&server)
                .await;
        }
        let url = if probability.is_some() {
            server.uri()
        } else {
            "http://127.0.0.1:9".to_string()
        };
        let service = residuum_inference::system_one::SystemOneService::new(Some(
            &residuum_config::SystemOneConfig {
                provider: residuum_config::SystemOneProvider::Ollama,
                url,
                model: "nimble".to_string(),
                api_key: None,
                keep_alive: None,
            },
        ));
        let gate = crate::auto_mode::AutoModeGate::new_shared(
            "scout",
            residuum_config::AutoModeConfig {
                enabled: true,
                deny: vec!["Running the counting tool".to_string()],
                allow: Vec::new(),
                threshold: 0.5,
            },
            service,
            residuum_inference::UsageRecorder::new(&residuum_config::WorkspaceLayout::new(
                usage_dir.path(),
            )),
        );

        let runs = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool::new(Arc::clone(&runs))));
        tools.set_auto_mode(gate);

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "counting_tool".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };
        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let mut recent = RecentMessages::new();
        execute_tool(&tool_call, &resources, &mut recent, &events, 0, None).await;
        let output = recent
            .messages()
            .first()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        (runs.load(Ordering::SeqCst), output)
    }

    #[tokio::test]
    async fn a_call_auto_mode_blocks_never_reaches_the_tool() {
        let (runs, output) = run_counting_tool_under_auto_mode(Some(0.95)).await;
        assert_eq!(runs, 0, "a blocked call must not run");
        assert!(
            output.contains("Auto Mode blocked") && output.contains("Running the counting tool"),
            "the agent is told which rule blocked it: {output}"
        );
    }

    #[tokio::test]
    async fn a_call_auto_mode_allows_runs_normally() {
        let (runs, output) = run_counting_tool_under_auto_mode(Some(0.05)).await;
        assert_eq!(runs, 1);
        assert_eq!(output, "ok");
    }

    #[tokio::test]
    async fn an_unreachable_decision_model_lets_the_call_run_unchecked() {
        let (runs, output) = run_counting_tool_under_auto_mode(None).await;
        assert_eq!(runs, 1, "an outage degrades to running unchecked");
        assert_eq!(output, "ok");
    }

    #[tokio::test]
    async fn execute_tool_reports_cancellation_when_stopped_mid_execution() {
        let tool_gate = Gate::closed("the blocking tool");
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(BlockingTool::new(&tool_gate)));

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "blocking_tool".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let cancel_once_the_tool_hangs = {
            let stop_token = stop_token.clone();
            let tool_gate = &tool_gate;
            async move {
                tool_gate.until_held(1).await;
                stop_token.cancel();
            }
        };

        let mut recent = RecentMessages::new();
        wait::guarded(
            "execute_tool to return once stopped",
            Box::pin(async {
                tokio::join!(
                    execute_tool(&tool_call, &resources, &mut recent, &events, 0, None),
                    cancel_once_the_tool_hangs,
                )
            }),
        )
        .await;

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool, "should be a tool-result message");
        assert_eq!(
            msg.content,
            crate::tool::CANCELLED_WHILE_RUNNING,
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
            server: None,
        };

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        skip_tool_call(&tool_call, &resources, &mut recent, &events, 0).await;

        let msg = recent
            .messages()
            .first()
            .expect("a tool-result message should have been recorded");
        assert_eq!(msg.role, Role::Tool, "should be a tool-result message");
        assert_eq!(msg.content, crate::tool::CANCELLED_BEFORE_START);
    }

    struct TwoToolCallsProvider {
        served: AtomicBool,
    }

    #[async_trait]
    impl InferenceProvider for TwoToolCallsProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
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
                        server: None,
                    },
                    ToolCall {
                        id: "call-2".to_string(),
                        name: "blocking_tool".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
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
        let tool_gate = Gate::closed("the blocking tool");
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(BlockingTool::new(&tool_gate)));

        let provider = TwoToolCallsProvider {
            served: AtomicBool::new(false),
        };
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            hop_counter: &hop_counter,
            usage_sink: None,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = silent_events(&publisher);
        let memory_ctx = MemoryContext {
            observations: None,
            recent_context: None,
        };
        let prompt_ctx = PromptContext::default();
        let (_interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();
        recent.push(Message::user("go"));

        let cancel_once_the_tool_hangs = {
            let stop_token = stop_token.clone();
            let tool_gate = &tool_gate;
            async move {
                tool_gate.until_held(1).await;
                stop_token.cancel();
            }
        };

        let (turn_result, ()) = wait::guarded(
            "the turn to end once stopped",
            Box::pin(async {
                tokio::join!(
                    execute_turn(
                        recent.len() - 1,
                        &resources,
                        &memory_ctx,
                        &prompt_ctx,
                        &mut recent,
                        &events,
                        None,
                        &mut interrupt_rx,
                        None,
                    ),
                    cancel_once_the_tool_hangs,
                )
            }),
        )
        .await;

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
            crate::tool::CANCELLED_WHILE_RUNNING,
            "the call that was running when the stop landed was interrupted"
        );
        assert_eq!(
            tool_results
                .get(1)
                .expect("checked above: exactly 2 results")
                .content,
            crate::tool::CANCELLED_BEFORE_START,
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
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            let mut resp = InferenceResponse::new("cut off mid-sen".to_string(), vec![]);
            resp.stop_reason = Some(residuum_inference::StopReason::MaxTokens);
            Ok(resp)
        }

        fn model_name(&self) -> &'static str {
            "truncated"
        }
    }

    #[derive(Default)]
    struct MockUsageSink {
        calls: StdMutex<Vec<Option<residuum_inference::Usage>>>,
        tool_call_batches: StdMutex<Vec<u32>>,
        totals_to_return: StdMutex<Vec<SessionUsageTotals>>,
        recorded_calls: StdMutex<Vec<(String, bool)>>,
    }

    impl MockUsageSink {
        fn new(totals_to_return: Vec<SessionUsageTotals>) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                tool_call_batches: StdMutex::new(Vec::new()),
                totals_to_return: StdMutex::new(totals_to_return),
                recorded_calls: StdMutex::new(Vec::new()),
            }
        }

        fn next_totals(&self) -> SessionUsageTotals {
            let mut totals = self.totals_to_return.lock().unwrap();
            if totals.len() > 1 {
                totals.remove(0)
            } else {
                totals.first().copied().unwrap_or_default()
            }
        }
    }

    #[async_trait]
    impl UsageSink for MockUsageSink {
        async fn record_call(
            &self,
            provider: &dyn InferenceProvider,
            result: &Result<InferenceResponse, residuum_inference::InferenceError>,
        ) {
            self.recorded_calls
                .lock()
                .unwrap()
                .push((provider.model_name().to_string(), result.is_ok()));
        }

        async fn accumulate(&self, usage: Option<residuum_inference::Usage>) -> SessionUsageTotals {
            self.calls.lock().unwrap().push(usage);
            self.next_totals()
        }

        async fn record_tool_calls(&self, n: u32) -> SessionUsageTotals {
            self.tool_call_batches.lock().unwrap().push(n);
            self.next_totals()
        }
    }

    fn usage(input: u32, output: u32) -> residuum_inference::Usage {
        residuum_inference::Usage {
            uncached_input_tokens: Some(input),
            output_tokens: Some(output),
            ..Default::default()
        }
    }

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

    fn counting_tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "counting_tool".to_string(),
            arguments: serde_json::json!({"x": 1}),
            server: None,
        }
    }

    struct UsageFeed(residuum_bus::Subscriber<MainConversationEvent>);

    impl UsageFeed {
        async fn follow(bus_handle: &residuum_bus::BusHandle) -> Self {
            Self(
                bus_handle
                    .subscribe(topics::MainConversation)
                    .await
                    .unwrap(),
            )
        }

        async fn next(&mut self) -> TurnUsageEvent {
            loop {
                let event = next_event("a usage event to be published", &mut self.0).await;
                if let MainConversationEvent::TurnUsage(usage) = event {
                    return usage;
                }
            }
        }
    }

    #[tokio::test]
    async fn publish_usage_for_endpoint_target_maps_to_turn_usage_event() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;

        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
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
                    tool_calls: 3,
                },
                Some(totals),
            )
            .await;

        let event = sub.next().await;
        assert_eq!(event.correlation_id, "corr-1");
        assert_eq!(event.output_tokens, 20);
        assert!(event.has_usage);
        assert_eq!(event.tool_calls, 3);
        assert_eq!(event.session_totals, Some(totals));
    }

    #[tokio::test]
    async fn publish_usage_with_no_output_endpoint_still_reports_to_the_main_conversation() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut sub = UsageFeed::follow(&bus_handle).await;

        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        events.publish_usage(TurnUsage::default(), None).await;

        let event = sub.next().await;
        assert_eq!(event.correlation_id, "corr-1");
        assert!(!event.has_usage);
    }

    #[tokio::test]
    async fn publish_usage_for_a_session_target_uses_session_event_kind_turn_usage() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut sub: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();

        let address = SessionAddress::from("spawned-x-0001");
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Session {
                address: &address,
                run_id: "run-1",
                turn_id: "run-1-t1",
            },
            session_conversation: None,
        };

        events
            .publish_usage(
                TurnUsage {
                    output_tokens: 7,
                    has_usage: true,
                    tool_calls: 2,
                },
                None,
            )
            .await;

        let event = next_event("a SessionEvent to be published", &mut sub).await;
        assert_eq!(event.address, address);
        assert_eq!(event.run_id, "run-1");
        assert!(matches!(
            event.kind,
            residuum_bus::SessionEventKind::TurnUsage {
                output_tokens: 7,
                has_usage: true,
                tool_calls: 2,
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
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(100, 20));
        update_and_publish_usage(&response, 2, &mut turn_usage, Some(&sink), &events).await;

        assert_eq!(
            turn_usage.output_tokens, 20,
            "the turn's own running total should accumulate"
        );
        assert_eq!(
            turn_usage.tool_calls, 2,
            "the turn's own running total should count this batch's executed tool calls"
        );
        assert_eq!(
            sink.calls.lock().unwrap().as_slice(),
            &[Some(usage(100, 20))],
            "the sink should receive exactly the response's usage"
        );
        assert_eq!(
            sink.tool_call_batches.lock().unwrap().as_slice(),
            &[2],
            "the sink should receive exactly this batch's executed tool-call count"
        );

        let event = sub.next().await;
        assert_eq!(event.output_tokens, 20);
        assert_eq!(event.tool_calls, 2);
        assert_eq!(event.session_totals.map(|t| t.input_tokens), Some(100));
    }

    #[tokio::test]
    async fn update_and_publish_usage_with_zero_tool_calls_does_not_call_the_sinks_tool_call_path()
    {
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(100, 20));
        update_and_publish_usage(&response, 0, &mut turn_usage, Some(&sink), &events).await;

        assert_eq!(
            turn_usage.tool_calls, 0,
            "a text-only response carries no executed tool calls"
        );
        assert!(
            sink.tool_call_batches.lock().unwrap().is_empty(),
            "a batch of zero tool calls should not reach the sink's tool-call path"
        );
    }

    #[tokio::test]
    async fn update_and_publish_usage_with_no_sink_still_publishes_the_turn_progress() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(50, 5));
        update_and_publish_usage(&response, 0, &mut turn_usage, None, &events).await;

        let event = sub.next().await;
        assert_eq!(event.output_tokens, 5);
        assert_eq!(
            event.session_totals, None,
            "with no usage sink there are no session totals to report"
        );
    }

    #[tokio::test]
    async fn update_and_publish_usage_a_provider_with_no_usage_leaves_totals_blank() {
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut turn_usage = TurnUsage::default();
        let response = InferenceResponse::new("hi".to_string(), vec![]);
        update_and_publish_usage(&response, 0, &mut turn_usage, Some(&sink), &events).await;

        assert!(
            !turn_usage.has_usage,
            "a provider reporting no usage must not flip has_usage"
        );
        let event = sub.next().await;
        assert!(!event.has_usage);
        assert_eq!(event.output_tokens, 0);
    }

    struct TextOnlyProvider;

    #[async_trait]
    impl InferenceProvider for TextOnlyProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            let mut resp = InferenceResponse::new("hello".to_string(), vec![]);
            resp.usage = Some(usage(10, 5));
            Ok(resp)
        }

        fn model_name(&self) -> &'static str {
            "text-only"
        }
    }

    #[tokio::test]
    async fn execute_turn_with_no_tool_calls_reports_zero_tool_calls() {
        let provider = TextOnlyProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: Some(&sink),
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
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
            recent.len() - 1,
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
        .unwrap();
        assert_eq!(texts, vec!["hello".to_string()]);

        let event = sub.next().await;
        assert_eq!(
            event.tool_calls, 0,
            "a turn with no tool calls should report a zero tool-call count, not omit it"
        );
        assert!(
            sink.tool_call_batches.lock().unwrap().is_empty(),
            "the sink's tool-call path should never be reached when no tools ran"
        );
        assert_eq!(
            *sink.recorded_calls.lock().unwrap(),
            vec![(provider.model_name().to_string(), true)],
            "the turn's one model call is recorded"
        );
    }

    struct FailingProvider;

    #[async_trait]
    impl InferenceProvider for FailingProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            Err(residuum_inference::InferenceError::Api(
                "503 overloaded".to_string(),
            ))
        }

        fn model_name(&self) -> &'static str {
            "failing"
        }
    }

    #[tokio::test]
    async fn a_failed_model_call_is_recorded_before_the_turn_fails() {
        let provider = FailingProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: Some(&sink),
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
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

        let result = execute_turn(
            recent.len() - 1,
            &resources,
            &memory_ctx,
            &prompt_ctx,
            &mut recent,
            &events,
            None,
            &mut interrupt_rx,
            None,
        )
        .await;
        assert!(result.is_err(), "the turn fails with its model call");
        assert_eq!(
            *sink.recorded_calls.lock().unwrap(),
            vec![("failing".to_string(), false)],
            "the failed call is still recorded"
        );
    }

    struct ToolsThenTextProvider {
        served: AtomicBool,
    }

    #[async_trait]
    impl InferenceProvider for ToolsThenTextProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            if self.served.swap(true, Ordering::SeqCst) {
                let mut resp = InferenceResponse::new("done".to_string(), vec![]);
                resp.usage = Some(usage(20, 5));
                return Ok(resp);
            }
            let mut resp = InferenceResponse::new(
                String::new(),
                vec![
                    ToolCall {
                        id: "call-1".to_string(),
                        name: "counting_tool".to_string(),
                        arguments: serde_json::json!({"x": 1}),
                        server: None,
                    },
                    ToolCall {
                        id: "call-2".to_string(),
                        name: "counting_tool".to_string(),
                        arguments: serde_json::json!({"x": 2}),
                        server: None,
                    },
                    ToolCall {
                        id: "call-3".to_string(),
                        name: "counting_tool".to_string(),
                        arguments: serde_json::json!({"x": 3}),
                        server: None,
                    },
                ],
            );
            resp.usage = Some(usage(30, 10));
            Ok(resp)
        }

        fn model_name(&self) -> &'static str {
            "tools-then-text"
        }
    }

    #[tokio::test]
    async fn execute_turn_counts_every_parallel_tool_call_in_a_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool::new(Arc::clone(&calls))));

        let provider = ToolsThenTextProvider {
            served: AtomicBool::new(false),
        };
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
        let identity = IdentityFiles::default();
        let options = CompletionOptions::default();
        let stop_token = CancellationToken::new();
        let hop_counter = HopCounter::new(0);
        let sink = MockUsageSink::new(vec![SessionUsageTotals::default()]);
        let resources = TurnResources {
            provider: &provider,
            tools: &tools,
            mcp_registry: &mcp_registry,
            identity: &identity,
            options: &options,
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: Some(&sink),
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = UsageFeed::follow(&bus_handle).await;
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(Some(&ep)),
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
            recent.len() - 1,
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
        .unwrap();
        assert_eq!(texts, vec!["done".to_string()]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "all three calls should have run"
        );

        let after_batch = sub.next().await;
        assert_eq!(
            after_batch.tool_calls, 3,
            "every parallel call in the batch should count individually"
        );

        let after_final = sub.next().await;
        assert_eq!(
            after_final.tool_calls, 3,
            "the tool-call count must carry over unchanged into the turn's final response"
        );

        assert_eq!(
            sink.tool_call_batches.lock().unwrap().as_slice(),
            &[3],
            "the durable session sink should receive exactly one batch of 3"
        );
    }

    #[tokio::test]
    async fn execute_turn_notices_and_notes_a_truncated_response() {
        let provider = TruncatedProvider;
        let tools = ToolRegistry::new();
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };

        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: residuum_bus::Subscriber<residuum_bus::NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
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
            recent.len() - 1,
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

        let notice = next_event("a truncation notice to be published", &mut notices).await;
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
        tools.register(Box::new(CountingTool::new(Arc::clone(&calls))));
        let batch = vec![
            counting_tool_call("c1"),
            counting_tool_call("c2"),
            counting_tool_call("c3"),
        ];

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig {
                enabled: true,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let (executed, stopped) =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard, 0).await;

        assert!(stopped.is_none(), "3 repeats is below the stop threshold");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "all three calls should have actually run"
        );
        assert_eq!(
            executed, 3,
            "every call in a parallel batch should count individually toward the executed total"
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
        tools.register(Box::new(CountingTool::new(Arc::clone(&calls))));
        let batch: Vec<ToolCall> = (0..7)
            .map(|i| counting_tool_call(&format!("c{i}")))
            .collect();

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig {
                enabled: true,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let (executed, stopped) =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard, 0).await;

        let (name, count) = stopped.expect("the 6th identical call should stop the turn");
        assert_eq!(name, "counting_tool");
        assert_eq!(count, 6);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            5,
            "only the first 5 calls should actually run; the 6th is refused and the 7th skipped"
        );
        assert_eq!(
            executed, 5,
            "the refused and skipped calls must not count toward the executed total"
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
            crate::tool::CANCELLED_BEFORE_START,
            "the 7th call should be skipped outright, not attempted"
        );
    }

    #[tokio::test]
    async fn repeat_guard_disabled_never_steers_or_stops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool::new(Arc::clone(&calls))));
        let batch: Vec<ToolCall> = (0..10)
            .map(|i| counting_tool_call(&format!("c{i}")))
            .collect();

        let provider = residuum_inference::NullProvider;
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig {
                enabled: false,
                steer_after: 3,
                stop_after: 6,
            },
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };

        let mut recent = RecentMessages::new();
        let mut guard = RepeatCallGuard::new();
        let (executed, stopped) =
            run_tool_call_batch(&batch, &resources, &mut recent, &events, &mut guard, 0).await;

        assert!(stopped.is_none(), "a disabled guard never stops the turn");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            10,
            "every call should run when the guard is disabled"
        );
        assert_eq!(
            executed, 10,
            "every parallel call should count individually toward the executed total"
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

    struct RepeatingCallProvider {
        completions: AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for RepeatingCallProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
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
        tools.register(Box::new(CountingTool::new(Arc::clone(&calls))));

        let provider = RepeatingCallProvider {
            completions: AtomicUsize::new(0),
        };
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
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

        let texts = wait::guarded(
            "the repeat-call guard to end the turn",
            Box::pin(execute_turn(
                recent.len() - 1,
                &resources,
                &memory_ctx,
                &prompt_ctx,
                &mut recent,
                &events,
                None,
                &mut interrupt_rx,
                None,
            )),
        )
        .await
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

    struct ImagesOmittedProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for ImagesOmittedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut response = if call == 0 {
                InferenceResponse::new(String::new(), vec![counting_tool_call("c1")])
            } else {
                InferenceResponse::new("done".to_string(), vec![])
            };
            response.images_omitted = Some(residuum_inference::ImagesOmitted {
                model: "glm-5.3".to_string(),
                count: 9,
                learned: call >= 3,
            });
            Ok(response)
        }

        fn model_name(&self) -> &'static str {
            "glm-5.3"
        }
    }

    fn png() -> residuum_inference::ImageData {
        residuum_inference::ImageData {
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        }
    }

    #[tokio::test]
    async fn only_images_a_turn_brings_in_are_noticed_once_per_turn() {
        let provider = ImagesOmittedProvider {
            calls: AtomicUsize::new(0),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool::new(Arc::new(AtomicUsize::new(0)))));
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };

        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: residuum_bus::Subscriber<residuum_bus::NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let (_interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();

        let turns = [
            Message::user_with_images("look at this", vec![png()]),
            Message::user("and again"),
            Message::user_with_images("two more", vec![png(), png()]),
        ];
        for (n, turn) in turns.into_iter().enumerate() {
            recent.push(turn);
            execute_turn(
                recent.len() - 1,
                &resources,
                &MemoryContext {
                    observations: None,
                    recent_context: None,
                },
                &PromptContext::default(),
                &mut recent,
                &events,
                None,
                &mut interrupt_rx,
                None,
            )
            .await
            .unwrap();
            let message = format!("end of turn {n}");
            let end = topics::Notification(NotifyName::from(SYSTEM_CHANNEL));
            publisher
                .publish(end, NoticeEvent::new(message))
                .await
                .unwrap();
        }

        let mut next = async || next_event("a notice", &mut notices).await.message;
        assert_eq!(
            next().await,
            "This model can't see images, so 1 image was left out of the request.",
            "the turn's own image is counted, not the history's"
        );
        assert_eq!(next().await, "end of turn 0", "one notice for two requests");
        assert_eq!(
            next().await,
            "end of turn 1",
            "an image from an earlier turn is left out quietly"
        );
        let third = next().await;
        assert!(
            third.starts_with("This model can't see images, so 2 images were left out"),
            "{third}"
        );
        assert!(
            third.contains("mark glm-5.3 as text only"),
            "a learned verdict suggests declaring it: {third}"
        );
        assert_eq!(next().await, "end of turn 2");
    }

    struct ThinkingDroppedProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for ThinkingDroppedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, residuum_inference::InferenceError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut response = if call.is_multiple_of(2) {
                InferenceResponse::new(String::new(), vec![counting_tool_call("c1")])
            } else {
                InferenceResponse::new("done".to_string(), vec![])
            };
            response.thinking_dropped = 1;
            Ok(response)
        }

        fn model_name(&self) -> &'static str {
            "claude-opus-5-5"
        }
    }

    #[tokio::test]
    async fn dropped_reasoning_is_noticed_once_per_turn() {
        let provider = ThinkingDroppedProvider {
            calls: AtomicUsize::new(0),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CountingTool::new(Arc::new(AtomicUsize::new(0)))));
        let mcp_registry = residuum_mcp::McpRegistry::new_shared();
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
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            stop_token: &stop_token,
            transcript_sink: None,
            usage_sink: None,
            hop_counter: &hop_counter,
        };

        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: residuum_bus::Subscriber<residuum_bus::NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let events = EventContext {
            publisher: &publisher,
            reply_audience: None,
            target: EventTarget::Endpoint {
                output_endpoint: TurnOutput::Fixed(None),
                correlation_id: "corr-1",
            },
            session_conversation: None,
        };
        let (_interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
        let mut recent = RecentMessages::new();

        for n in 0..2 {
            recent.push(Message::user("go"));
            execute_turn(
                recent.len() - 1,
                &resources,
                &MemoryContext {
                    observations: None,
                    recent_context: None,
                },
                &PromptContext::default(),
                &mut recent,
                &events,
                None,
                &mut interrupt_rx,
                None,
            )
            .await
            .unwrap();
            let message = format!("end of turn {n}");
            let end = topics::Notification(NotifyName::from(SYSTEM_CHANNEL));
            publisher
                .publish(end, NoticeEvent::new(message))
                .await
                .unwrap();
        }

        let mut next = async || next_event("a notice", &mut notices).await.message;
        assert_eq!(next().await, THINKING_DROPPED_NOTICE);
        assert_eq!(next().await, "end of turn 0", "one notice for two requests");
        assert_eq!(
            next().await,
            THINKING_DROPPED_NOTICE,
            "the next turn tells the user again"
        );
        assert_eq!(next().await, "end of turn 1");
    }
}

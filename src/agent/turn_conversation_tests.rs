//! What a turn publishes to the main conversation: the order of its events,
//! the streamed text and reasoning a provider pushes while a model call runs,
//! and what stays on the endpoint topics for the chat interfaces.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::HopCounter;
use super::context::{MemoryContext, PromptContext};
use super::interrupt::Interrupt;
use super::recent_messages::RecentMessages;
use super::turn::{EventContext, EventTarget, TranscriptSink, TurnResources, execute_turn};
use crate::bus::{
    AgentMessageEvent, EndpointName, IntermediateEvent, MainConversationEvent, MessageEvent,
    SessionAddress, Subscriber, ToolActivityEvent, TurnUsageEvent, topics,
};
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    MessageSender, Role, StreamDelta, StreamSink, ThinkingBlock, ToolCall, ToolDefinition,
};
use crate::interfaces::types::MessageOrigin;
use crate::tools::{Tool, ToolError, ToolRegistry, ToolResult};
use crate::workspace::identity::IdentityFiles;

/// What a scripted provider does while a model call is in flight.
enum Push {
    /// Hand the sink a delta.
    Delta(StreamDelta),
    /// Let time pass.
    Wait(Duration),
    /// Never return.
    Hang,
}

/// One model call: what the provider pushes into the sink, then what it
/// returns.
struct Step {
    pushes: Vec<Push>,
    response: InferenceResponse,
}

impl Step {
    fn reply(response: InferenceResponse) -> Self {
        Self {
            pushes: Vec::new(),
            response,
        }
    }

    fn streaming(pushes: Vec<Push>, response: InferenceResponse) -> Self {
        Self { pushes, response }
    }
}

fn text(content: &str) -> InferenceResponse {
    InferenceResponse::new(content.to_string(), Vec::new())
}

fn tool_call_response(content: &str) -> InferenceResponse {
    InferenceResponse::new(
        content.to_string(),
        vec![ToolCall {
            id: "call-1".to_string(),
            name: "echo".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        }],
    )
}

fn delta_text(s: &str) -> Push {
    Push::Delta(StreamDelta::Text(s.to_string()))
}

fn delta_thinking(s: &str) -> Push {
    Push::Delta(StreamDelta::Thinking(s.to_string()))
}

struct ScriptedProvider {
    script: Mutex<VecDeque<Step>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            script: Mutex::new(steps.into()),
        }
    }

    fn next_step(&self) -> Step {
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script has a step for every model call")
    }
}

#[async_trait]
impl InferenceProvider for ScriptedProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        Ok(self.next_step().response)
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
        sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        let step = self.next_step();
        for push in step.pushes {
            match push {
                Push::Delta(delta) => sink.push(delta),
                Push::Wait(time) => tokio::time::sleep(time).await,
                Push::Hang => std::future::pending().await,
            }
        }
        Ok(step.response)
    }

    fn model_name(&self) -> &'static str {
        "scripted"
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success("echoed"))
    }
}

/// A session's durable transcript, as the turn loop writes it.
#[derive(Default)]
struct RecordingSink {
    recorded: Mutex<Vec<Message>>,
}

#[async_trait]
impl TranscriptSink for RecordingSink {
    async fn append(&self, messages: &[Message]) {
        self.recorded.lock().unwrap().extend_from_slice(messages);
    }
}

#[derive(Default)]
struct Options {
    interrupts: Vec<Interrupt>,
    max_tool_iterations: Option<usize>,
    stop_token: CancellationToken,
    /// Run as an agent session's turn instead of the main agent's.
    as_session: bool,
}

/// What a turn did, as the buses saw it.
struct Run {
    result: anyhow::Result<Vec<String>>,
    /// The main conversation, in order, up to the sentinel the harness ends it with.
    main: Vec<MainConversationEvent>,
    history: Vec<Message>,
    /// What the turn wrote to its durable transcript.
    transcript: Vec<Message>,
    endpoint_intermediate: Vec<IntermediateEvent>,
    endpoint_tools: Vec<ToolActivityEvent>,
    endpoint_usage: Vec<TurnUsageEvent>,
}

const TURN: &str = "turn-1";

async fn run_turn(steps: Vec<Step>) -> Run {
    run_turn_with(steps, Options::default()).await
}

async fn run_turn_with(steps: Vec<Step>, options: Options) -> Run {
    let provider = ScriptedProvider::new(steps);
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));
    let mcp_registry = crate::mcp::McpRegistry::new_shared();
    let identity = IdentityFiles::default();
    let completion = CompletionOptions::default();
    let hop_counter = HopCounter::new(0);
    let sink = RecordingSink::default();
    let resources = TurnResources {
        provider: &provider,
        tools: &tools,
        mcp_registry: &mcp_registry,
        identity: &identity,
        options: &completion,
        max_tool_iterations: options.max_tool_iterations,
        repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
        stop_token: &options.stop_token,
        transcript_sink: Some(&sink),
        usage_sink: None,
        hop_counter: &hop_counter,
    };

    let bus = crate::bus::spawn_broker();
    let publisher = bus.publisher();
    let ep = EndpointName::from("ws");
    let mut main: Subscriber<MainConversationEvent> =
        bus.subscribe(topics::MainConversation).await.unwrap();
    let mut intermediate: Subscriber<IntermediateEvent> =
        bus.subscribe(topics::Endpoint(ep.clone())).await.unwrap();
    let mut tool_activity: Subscriber<ToolActivityEvent> =
        bus.subscribe(topics::Endpoint(ep.clone())).await.unwrap();
    let mut usage: Subscriber<TurnUsageEvent> =
        bus.subscribe(topics::Endpoint(ep.clone())).await.unwrap();
    let session_address = SessionAddress::from("spawned-test-0001");
    let target = if options.as_session {
        EventTarget::Session {
            address: &session_address,
            run_id: "run-1",
        }
    } else {
        EventTarget::Endpoint {
            output_endpoint: Some(&ep),
            tool_activity_endpoint: Some(&ep),
            correlation_id: TURN,
        }
    };
    let events = EventContext {
        publisher: &publisher,
        target,
        session_conversation: None,
    };

    let (interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
    for interrupt in options.interrupts {
        interrupt_tx.send(interrupt).unwrap();
    }
    let mut recent = RecentMessages::new();
    recent.push(Message::user("go"));
    let result = execute_turn(
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
    .await;

    // The broker handles publishes in order, so by the time the sentinel
    // arrives everything the turn published has reached every subscriber.
    publisher
        .publish(
            topics::MainConversation,
            MainConversationEvent::TurnEnded {
                turn_id: "sentinel".to_string(),
            },
        )
        .await
        .unwrap();
    let mut main_events = Vec::new();
    loop {
        let event = main.recv().await.unwrap().unwrap();
        if matches!(&event, MainConversationEvent::TurnEnded { turn_id } if turn_id == "sentinel") {
            break;
        }
        main_events.push(event);
    }
    Run {
        result,
        main: main_events,
        history: recent.messages().to_vec(),
        transcript: sink.recorded.lock().unwrap().clone(),
        endpoint_intermediate: intermediate.drain(),
        endpoint_tools: tool_activity.drain(),
        endpoint_usage: usage.drain(),
    }
}

/// A one-word name per event, for comparing the order a turn published in.
fn shape(events: &[MainConversationEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            MainConversationEvent::TurnStarted { .. } => "turn_started".to_string(),
            MainConversationEvent::TurnEnded { .. } => "turn_ended".to_string(),
            MainConversationEvent::UserMessage { id, .. } => format!("user_message:{id}"),
            MainConversationEvent::ToolCall { call, .. } => format!("tool_call:{call}"),
            MainConversationEvent::ToolResult(_) => "tool_result".to_string(),
            MainConversationEvent::TextDelta { call, text, .. } => format!("text:{call}:{text}"),
            MainConversationEvent::ThinkingDelta { call, text, .. } => {
                format!("thinking_delta:{call}:{text}")
            }
            MainConversationEvent::StreamRestart { call, .. } => format!("restart:{call}"),
            MainConversationEvent::Thinking { call, content, .. } => {
                format!("thinking:{call}:{content}")
            }
            MainConversationEvent::Intermediate { call, content, .. } => {
                format!("intermediate:{call}:{content}")
            }
            MainConversationEvent::Response {
                call,
                endpoint,
                content,
                ..
            } => format!(
                "response:{}:{endpoint}:{content}",
                call.map_or_else(|| "-".to_string(), |c| c.to_string())
            ),
            MainConversationEvent::TurnUsage(_) => "usage".to_string(),
        })
        .collect()
}

fn person_message(id: &str, endpoint: &str) -> MessageEvent {
    MessageEvent {
        id: id.to_string(),
        content: format!("message {id}"),
        origin: MessageOrigin {
            endpoint: endpoint.to_string(),
            sender: Some(MessageSender {
                name: "Bear".to_string(),
                id: "42".to_string(),
                interface: endpoint.to_string(),
                location: None,
            }),
            conversation: None,
            agent_sender: None,
        },
        timestamp: chrono::Utc::now().naive_utc(),
        images: Vec::new(),
        context: None,
    }
}

// ── Order and numbering ─────────────────────────────────────────────────

#[tokio::test]
async fn a_turn_publishes_its_events_in_order_and_numbers_its_model_calls() {
    let run = run_turn(vec![
        Step::reply(tool_call_response("Checking.")),
        Step::reply(text("All done.")),
    ])
    .await;

    assert_eq!(run.result.unwrap(), ["All done."]);
    assert_eq!(
        shape(&run.main),
        [
            "intermediate:0:Checking.",
            "tool_call:0",
            "tool_result",
            "usage",
            "usage",
            "response:1:ws:All done.",
        ],
        "text and tool calls come in the order the model made them, each tagged with its call"
    );
}

#[tokio::test]
async fn a_turns_events_name_the_turn_they_belong_to() {
    let run = run_turn(vec![
        Step::reply(tool_call_response("Checking.")),
        Step::reply(text("All done.")),
    ])
    .await;

    for event in &run.main {
        match event {
            MainConversationEvent::Intermediate { turn_id, .. }
            | MainConversationEvent::Response { turn_id, .. } => assert_eq!(turn_id, TURN),
            MainConversationEvent::ToolCall { event, .. } => {
                assert_eq!(event.correlation_id, TURN);
            }
            MainConversationEvent::ToolResult(result) => assert_eq!(result.correlation_id, TURN),
            MainConversationEvent::TurnUsage(usage) => assert_eq!(usage.correlation_id, TURN),
            MainConversationEvent::TurnStarted { .. }
            | MainConversationEvent::TurnEnded { .. }
            | MainConversationEvent::UserMessage { .. }
            | MainConversationEvent::TextDelta { .. }
            | MainConversationEvent::ThinkingDelta { .. }
            | MainConversationEvent::StreamRestart { .. }
            | MainConversationEvent::Thinking { .. } => {
                panic!("a turn with no streaming published {event:?}")
            }
        }
    }
}

#[tokio::test]
async fn the_endpoint_topics_keep_carrying_what_the_chat_interfaces_read() {
    let run = run_turn(vec![
        Step::reply(tool_call_response("Checking.")),
        Step::reply(text("All done.")),
    ])
    .await;

    assert_eq!(
        run.endpoint_intermediate
            .iter()
            .map(|event| event.content.as_str())
            .collect::<Vec<_>>(),
        ["Checking."]
    );
    assert!(
        matches!(
            run.endpoint_tools.as_slice(),
            [ToolActivityEvent::Call(call), ToolActivityEvent::Result(result)]
                if call.name == "echo" && result.output == "echoed"
        ),
        "{:?}",
        run.endpoint_tools
    );
    assert_eq!(run.endpoint_usage.len(), 2);
}

#[tokio::test]
async fn a_sessions_turn_is_not_part_of_the_main_conversation() {
    let run = run_turn_with(
        vec![
            Step::reply(tool_call_response("Checking.")),
            Step::reply(text("All done.")),
        ],
        Options {
            as_session: true,
            ..Options::default()
        },
    )
    .await;

    assert_eq!(run.result.unwrap(), ["All done."]);
    assert!(
        run.main.is_empty(),
        "a session's turn is not part of the main conversation: {:?}",
        shape(&run.main)
    );
    assert!(run.endpoint_tools.is_empty() && run.endpoint_usage.is_empty());
}

#[tokio::test]
async fn text_that_ends_a_turn_for_a_limit_is_a_reply_no_model_call_wrote() {
    let run = run_turn_with(
        vec![Step::reply(tool_call_response(""))],
        Options {
            max_tool_iterations: Some(1),
            ..Options::default()
        },
    )
    .await;

    let shape = shape(&run.main);
    let reply = shape.last().unwrap();
    assert!(
        reply.starts_with("response:-:ws:I stopped after 1 tool calls"),
        "{reply}"
    );
}

// ── A person's message joining the turn ──────────────────────────────────

#[tokio::test]
async fn a_person_message_folded_in_mid_turn_is_announced_under_its_own_id() {
    let run = run_turn_with(
        vec![Step::reply(text("ok"))],
        Options {
            interrupts: vec![Interrupt::UserMessage(person_message("tg-9", "telegram"))],
            ..Options::default()
        },
    )
    .await;

    let message = run
        .main
        .iter()
        .find_map(|event| {
            if let MainConversationEvent::UserMessage {
                id,
                turn_id,
                content,
                sender,
                endpoint,
                ..
            } = event
            {
                Some((id, turn_id, content, sender, endpoint))
            } else {
                None
            }
        })
        .expect("the injected message is announced");
    assert_eq!(message.0, "tg-9");
    assert_eq!(message.1, TURN, "it joined the running turn");
    assert_eq!(message.2, "message tg-9");
    assert_eq!(message.3.as_ref().map(|s| s.name.as_str()), Some("Bear"));
    assert_eq!(message.4, "telegram");
    assert!(
        run.history
            .iter()
            .any(|m| m.role == Role::User && m.content == "message tg-9"),
        "and the model reads it"
    );
}

#[tokio::test]
async fn messages_no_person_sent_are_not_announced_as_user_messages() {
    let background = MessageEvent::from_background("a session finished".to_string());
    let from_agent = AgentMessageEvent {
        from: SessionAddress::from("agent:atlas"),
        from_category: "teammate".to_string(),
        content: "status?".to_string(),
        hop_count: 0,
    };
    let run = run_turn_with(
        vec![Step::reply(text("ok"))],
        Options {
            interrupts: vec![
                Interrupt::UserMessage(background),
                Interrupt::UserMessage(MessageEvent::from_agent(&from_agent)),
                Interrupt::AgentMessage(from_agent),
            ],
            ..Options::default()
        },
    )
    .await;

    assert_eq!(
        shape(&run.main),
        ["usage", "response:0:ws:ok"],
        "agent-to-agent traffic reaches the model but is not a person speaking"
    );
    assert!(
        run.history.iter().any(|m| m.content.contains("status?")),
        "the model still reads it"
    );
}

// ── Streaming ────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn streamed_text_arrives_batched_ahead_of_the_authoritative_reply() {
    let run = run_turn(vec![Step::streaming(
        vec![
            delta_text("Hel"),
            Push::Wait(Duration::from_millis(10)),
            delta_text("lo, "),
            Push::Wait(Duration::from_millis(100)),
            delta_text("world"),
        ],
        text("Hello, world"),
    )])
    .await;

    assert_eq!(
        shape(&run.main),
        [
            "text:0:Hello, ",
            "text:0:world",
            "usage",
            "response:0:ws:Hello, world"
        ],
        "deltas inside one interval share a frame; the reply follows the last of them"
    );
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_small_deltas_is_one_frame() {
    let pushes = (0..40).map(|_| delta_text("ab")).collect();
    let run = run_turn(vec![Step::streaming(pushes, text(&"ab".repeat(40)))]).await;

    let frames: Vec<String> = shape(&run.main)
        .into_iter()
        .filter(|s| s.starts_with("text:"))
        .collect();
    assert_eq!(frames, [format!("text:0:{}", "ab".repeat(40))]);
}

#[tokio::test(start_paused = true)]
async fn text_and_reasoning_keep_their_order_when_the_kind_changes() {
    let run = run_turn(vec![Step::streaming(
        vec![
            delta_thinking("hm"),
            delta_thinking("mm"),
            delta_text("Sure"),
            delta_thinking("again"),
        ],
        text("Sure"),
    )])
    .await;

    assert_eq!(
        shape(&run.main)
            .into_iter()
            .filter(|s| s.starts_with("thinking_delta") || s.starts_with("text"))
            .collect::<Vec<_>>(),
        [
            "thinking_delta:0:hmmm",
            "text:0:Sure",
            "thinking_delta:0:again"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_voids_what_was_streamed_for_that_call() {
    let run = run_turn(vec![Step::streaming(
        vec![
            delta_text("partial"),
            Push::Wait(Duration::from_millis(100)),
            Push::Delta(StreamDelta::Restart),
            delta_text("full"),
        ],
        text("full"),
    )])
    .await;

    assert_eq!(
        shape(&run.main),
        [
            "text:0:partial",
            "restart:0",
            "text:0:full",
            "usage",
            "response:0:ws:full"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_drops_text_still_waiting_to_be_published() {
    let run = run_turn(vec![Step::streaming(
        vec![
            delta_text("never shown"),
            Push::Delta(StreamDelta::Restart),
            delta_text("shown"),
        ],
        text("shown"),
    )])
    .await;

    assert_eq!(
        shape(&run.main)
            .into_iter()
            .filter(|s| s.starts_with("text") || s.starts_with("restart"))
            .collect::<Vec<_>>(),
        ["restart:0", "text:0:shown"]
    );
}

#[tokio::test(start_paused = true)]
async fn each_model_call_streams_under_its_own_index() {
    let run = run_turn(vec![
        Step::streaming(
            vec![delta_text("Checking.")],
            tool_call_response("Checking."),
        ),
        Step::streaming(vec![delta_text("Done.")], text("Done.")),
    ])
    .await;

    assert_eq!(
        shape(&run.main),
        [
            "text:0:Checking.",
            "intermediate:0:Checking.",
            "tool_call:0",
            "tool_result",
            "usage",
            "text:1:Done.",
            "usage",
            "response:1:ws:Done."
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_stopped_call_keeps_the_text_it_had_streamed_and_ends_quietly() {
    let stop = CancellationToken::new();
    let stopper = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        stopper.cancel();
    });

    let run = run_turn_with(
        vec![Step::streaming(
            vec![delta_text("Half an ans"), Push::Hang],
            text("never returned"),
        )],
        Options {
            stop_token: stop,
            ..Options::default()
        },
    )
    .await;

    assert!(
        run.result.unwrap().is_empty(),
        "a stopped turn has no reply"
    );
    assert_eq!(
        shape(&run.main),
        ["text:0:Half an ans"],
        "the partial text reaches the page, with no reply and nothing persisted for it"
    );
    assert_eq!(
        run.history.len(),
        2,
        "history holds the user message and the stop note only"
    );
    assert_eq!(run.history.last().map(|m| m.role), Some(Role::System));
}

#[tokio::test(start_paused = true)]
async fn an_empty_response_is_retried_as_the_same_call_after_voiding_what_it_streamed() {
    let mut empty = text("");
    empty.thinking = vec![ThinkingBlock::text("pondering")];
    let run = run_turn(vec![
        Step::streaming(vec![delta_thinking("pondering")], empty),
        Step::reply(text("Fine.")),
    ])
    .await;

    assert_eq!(
        shape(&run.main),
        [
            "thinking_delta:0:pondering",
            "usage",
            "restart:0",
            "usage",
            "response:0:ws:Fine."
        ],
        "the retry is the same model request, and its predecessor's reasoning is not kept"
    );
}

// ── Thinking ─────────────────────────────────────────────────────────────

fn with_thinking(mut response: InferenceResponse, blocks: Vec<ThinkingBlock>) -> InferenceResponse {
    response.thinking = blocks;
    response
}

#[tokio::test]
async fn a_calls_thinking_precedes_its_text_and_tool_calls_and_is_kept_in_history() {
    let first = with_thinking(
        tool_call_response("Checking."),
        vec![ThinkingBlock {
            text: "I should look.".to_string(),
            signature: Some("sig-1".to_string()),
            redacted: None,
            part: None,
        }],
    );
    let second = with_thinking(
        text("Done."),
        vec![
            ThinkingBlock::text("That worked."),
            ThinkingBlock {
                text: String::new(),
                signature: None,
                redacted: Some("encrypted".to_string()),
                part: None,
            },
            ThinkingBlock::text("Say so."),
        ],
    );
    let run = run_turn(vec![Step::reply(first), Step::reply(second)]).await;

    assert_eq!(
        shape(&run.main),
        [
            "thinking:0:I should look.",
            "intermediate:0:Checking.",
            "tool_call:0",
            "tool_result",
            "usage",
            "thinking:1:That worked.\n\nSay so.",
            "usage",
            "response:1:ws:Done."
        ],
        "the frame holds the readable blocks only, joined"
    );
    let assistant: Vec<&Message> = run
        .history
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .collect();
    assert_eq!(assistant.len(), 2);
    assert_eq!(
        assistant.first().unwrap().thinking,
        [ThinkingBlock {
            text: "I should look.".to_string(),
            signature: Some("sig-1".to_string()),
            redacted: None,
            part: None,
        }],
        "the intermediate assistant message keeps its blocks whole"
    );
    assert_eq!(
        assistant.last().unwrap().thinking.len(),
        3,
        "the final assistant message keeps every block, the encrypted one too"
    );
}

#[tokio::test]
async fn a_call_with_only_encrypted_reasoning_publishes_no_thinking_frame() {
    let response = with_thinking(
        text("Done."),
        vec![ThinkingBlock {
            text: String::new(),
            signature: None,
            redacted: Some("encrypted".to_string()),
            part: None,
        }],
    );
    let run = run_turn(vec![Step::reply(response)]).await;

    assert_eq!(shape(&run.main), ["usage", "response:0:ws:Done."]);
    assert_eq!(
        run.history.last().unwrap().thinking.len(),
        1,
        "the block is still kept for the provider"
    );
}

#[tokio::test]
async fn thinking_reaches_the_durable_transcript_with_the_message_it_belongs_to() {
    let reasoning = vec![ThinkingBlock {
        text: "I should look.".to_string(),
        signature: Some("sig-1".to_string()),
        redacted: None,
        part: None,
    }];
    let run = run_turn(vec![
        Step::reply(with_thinking(
            tool_call_response("Checking."),
            reasoning.clone(),
        )),
        Step::reply(with_thinking(text("Done."), reasoning.clone())),
    ])
    .await;

    let assistant: Vec<&Message> = run
        .transcript
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .collect();
    assert_eq!(assistant.len(), 2);
    assert!(
        assistant.iter().all(|m| m.thinking == reasoning),
        "every assistant message is recorded with its call's reasoning: {assistant:?}"
    );
}

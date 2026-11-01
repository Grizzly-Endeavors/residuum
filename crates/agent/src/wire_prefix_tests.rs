//! A conversation's requests keep their prefix on the wire.
//!
//! Each provider caches on the exact bytes of a request's start, so every
//! request a conversation sends must repeat the previous one's tools, system
//! prompt and messages byte for byte and only append to them. These tests run
//! a real [`Agent`] through the Anthropic and OpenAI-compatible clients (their
//! own request builders, against a wiremock server) over a conversation with
//! an image, a tool call and its result, a message that arrives mid-turn, a
//! message delivered between turns, reasoning from earlier turns, and a
//! restart that persists and reloads the history, and check every request
//! body against the one before it.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use residuum_bus::MessageEvent;
use residuum_bus::interrupt::Interrupt;
use residuum_bus::origin::MessageOrigin;
use residuum_config::{ThinkingConfig, ThinkingLevel};
use residuum_inference::{
    CacheHints, CacheRetention, CompletionOptions, ImageData, InferenceError, InferenceProvider,
    InferenceResponse, Message, StreamSink, ToolDefinition,
};
use residuum_memory::recent_messages::{append_recent_messages, load_messages_for_agent};
use residuum_workspace::identity::IdentityFiles;

use crate::context::PromptContext;
use crate::testing::EchoTool;
use crate::{Agent, AgentConfig, HopCounter, ToolRegistry};

/// The message the user sends while the first turn's tool call runs.
const MID_TURN_TEXT: &str = "actually, check it twice";
/// The message delivered between the first and second turns.
const BETWEEN_TURNS_TEXT: &str = "a note that reached your inbox";

/// Answers each request with the next reply of its script. While answering
/// the first request it also hands the running turn a user message, as if
/// the user sent it while the model was working; the turn reads it before
/// its next model call.
struct ScriptedReplies {
    replies: Mutex<VecDeque<Value>>,
    mid_turn: Mutex<Option<UnboundedSender<Interrupt>>>,
}

impl Respond for ScriptedReplies {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        if let Some(sender) = self.mid_turn.lock().unwrap().take() {
            sender
                .send(Interrupt::UserMessage(inbound("mid-turn", MID_TURN_TEXT)))
                .unwrap();
        }
        match self.replies.lock().unwrap().pop_front() {
            Some(body) => ResponseTemplate::new(200).set_body_json(body),
            None => ResponseTemplate::new(500).set_body_string("the script ran out"),
        }
    }
}

/// A provider that answers every call whole, so the scripted replies are
/// plain JSON rather than event streams. The request a streaming call sends
/// differs only in its top-level `stream` flag.
struct WholeReplies(Box<dyn InferenceProvider>);

#[async_trait]
impl InferenceProvider for WholeReplies {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.0.complete(messages, tools, options).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        _sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        self.0.complete(messages, tools, options).await
    }

    fn model_name(&self) -> &str {
        self.0.model_name()
    }
}

/// One provider's wire: how to reach it, what the turns ask of it, and the
/// replies it gives.
struct Wire {
    name: &'static str,
    endpoint: &'static str,
    client: fn(&str) -> Box<dyn InferenceProvider>,
    options: CompletionOptions,
    /// The first turn's tool call, its final reply, the second turn's reply,
    /// and the reply after the restart.
    replies: [Value; 4],
}

/// A request body's parts in the order Anthropic caches them, as the exact
/// bytes sent. An OpenAI-compatible body carries its system prompt as the
/// first message, so `system` is `None` there.
#[derive(Deserialize)]
struct WireParts<'a> {
    #[serde(borrow)]
    tools: Option<&'a RawValue>,
    #[serde(borrow)]
    system: Option<&'a RawValue>,
    #[serde(borrow)]
    messages: Vec<&'a RawValue>,
}

fn inbound(id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        id: id.to_string(),
        content: content.to_string(),
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
    }
}

/// A person's message from the web.
fn web_origin() -> MessageOrigin {
    MessageOrigin {
        endpoint: "ws".to_string(),
        sender: None,
        conversation: None,
        agent_sender: None,
    }
}

fn picture() -> ImageData {
    ImageData {
        media_type: "image/png".to_string(),
        data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==".to_string(),
    }
}

/// The main conversation's agent, with the echo tool, over `client`.
fn agent_over(client: Box<dyn InferenceProvider>, options: &CompletionOptions) -> Agent {
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));
    Agent::new(
        Box::new(WholeReplies(client)),
        tools,
        residuum_mcp::McpRegistry::new_shared(),
        IdentityFiles {
            soul: Some("You are the test agent.".to_string()),
            user: Some("The user tests caching.".to_string()),
            ..IdentityFiles::default()
        },
        AgentConfig {
            options: options.clone(),
            tz: chrono_tz::UTC,
            layout: None,
        },
        HopCounter::new(0),
    )
}

/// Run one turn of `agent` on a web message.
async fn send(
    agent: &mut Agent,
    text: &str,
    images: &[ImageData],
    correlation_id: &str,
    interrupts: &mut UnboundedReceiver<Interrupt>,
) {
    let publisher = residuum_bus::spawn_broker().publisher();
    let endpoint = residuum_bus::EndpointName::from("test");
    agent
        .process_message(
            text,
            &publisher,
            Some(&endpoint),
            correlation_id,
            Some(&web_origin()),
            &PromptContext::default(),
            interrupts,
            images,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
}

/// Hold the conversation over `wire` and return every request body it sent,
/// in order.
async fn converse(wire: &Wire) -> Vec<String> {
    let server = MockServer::start().await;
    let (mid_turn, mut interrupts) = unbounded_channel();
    Mock::given(method("POST"))
        .and(path(wire.endpoint))
        .respond_with(ScriptedReplies {
            replies: Mutex::new(wire.replies.clone().into()),
            mid_turn: Mutex::new(Some(mid_turn)),
        })
        .mount(&server)
        .await;
    let client = || (wire.client)(&server.uri());

    let mut agent = agent_over(client(), &wire.options);
    send(
        &mut agent,
        "what is in this picture?",
        &[picture()],
        "t1",
        &mut interrupts,
    )
    .await;
    agent.inject_inbound_message(inbound("between", BETWEEN_TURNS_TEXT));
    send(&mut agent, "and what now?", &[], "t2", &mut interrupts).await;

    // A restart: the history goes to disk and a new agent loads it.
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("recent_messages.json");
    append_recent_messages(
        &history,
        agent.messages_since(0),
        residuum_memory::types::Visibility::User,
        chrono_tz::UTC,
        Some("t2"),
    )
    .await
    .unwrap();
    let restore = load_messages_for_agent(&history).await.unwrap();
    let mut restarted = agent_over(client(), &wire.options);
    restarted.restore_messages(restore.messages);
    restarted.set_last_user_message_at(restore.last_user_message_at);
    send(&mut restarted, "still there?", &[], "t3", &mut interrupts).await;

    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect()
}

/// Assert each body repeats the previous body's tools, system prompt and
/// messages byte for byte, and appends at least one message.
fn assert_each_request_extends_the_last(wire: &Wire, bodies: &[String]) {
    for (call, pair) in bodies.windows(2).enumerate() {
        let [earlier, later] = pair else {
            unreachable!("windows(2) yields pairs");
        };
        let earlier: WireParts<'_> = serde_json::from_str(earlier).unwrap();
        let later: WireParts<'_> = serde_json::from_str(later).unwrap();
        let (first, next) = (call + 1, call + 2);
        assert_eq!(
            earlier.tools.map(RawValue::get),
            later.tools.map(RawValue::get),
            "{}: call {next} sends call {first}'s tools unchanged",
            wire.name
        );
        assert_eq!(
            earlier.system.map(RawValue::get),
            later.system.map(RawValue::get),
            "{}: call {next} sends call {first}'s system prompt unchanged",
            wire.name
        );
        assert!(
            later.messages.len() > earlier.messages.len(),
            "{}: call {next} appends to call {first}'s {} messages",
            wire.name,
            earlier.messages.len()
        );
        for (index, (sent, resent)) in earlier.messages.iter().zip(&later.messages).enumerate() {
            assert_eq!(
                sent.get(),
                resent.get(),
                "{}: call {next} repeats message {index} of call {first}",
                wire.name
            );
        }
    }
}

/// The options of a main-conversation turn: keyed to the conversation.
fn main_options(thinking: Option<ThinkingConfig>) -> CompletionOptions {
    CompletionOptions {
        thinking,
        cache: CacheHints {
            conversation_key: Some("residuum-main-test".to_string()),
            retention: CacheRetention::Short,
        },
        ..CompletionOptions::default()
    }
}

fn anthropic_reply(thinking: &str, signature: &str, last: &Value, stop_reason: &str) -> Value {
    json!({
        "content": [
            {"type": "thinking", "thinking": thinking, "signature": signature},
            last
        ],
        "stop_reason": stop_reason,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    })
}

fn anthropic_wire() -> Wire {
    Wire {
        name: "anthropic",
        endpoint: "/v1/messages",
        client: |uri| {
            residuum_inference::testing::clients::anthropic_client(uri, "claude-sonnet-5-5")
                .unwrap()
        },
        options: main_options(Some(ThinkingConfig::Level(ThinkingLevel::Medium))),
        replies: [
            anthropic_reply(
                "look at it first",
                "sig-1",
                &json!({"type": "tool_use", "id": "toolu_1", "name": "echo", "input": {}}),
                "tool_use",
            ),
            anthropic_reply(
                "wrap up",
                "sig-2",
                &json!({"type": "text", "text": "a lighthouse"}),
                "end_turn",
            ),
            anthropic_reply(
                "answer the follow-up",
                "sig-3",
                &json!({"type": "text", "text": "nothing new"}),
                "end_turn",
            ),
            anthropic_reply(
                "still here",
                "sig-4",
                &json!({"type": "text", "text": "yes"}),
                "end_turn",
            ),
        ],
    }
}

fn openai_reply(message: &Value, finish_reason: &str) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
}

fn openai_text(text: &str) -> Value {
    openai_reply(&json!({"role": "assistant", "content": text}), "stop")
}

/// An OpenAI-compatible host without reasoning: the reasoning of a tool
/// exchange goes back only while the exchange is in progress (see
/// `docs/systems-usage/config.md`), so a host that returns reasoning is
/// sent it in the exchange and not after it.
fn openai_compatible_wire() -> Wire {
    Wire {
        name: "openai-compatible",
        endpoint: "/chat/completions",
        client: |uri| {
            residuum_inference::testing::clients::openai_compatible_client(uri, "test-model")
                .unwrap()
        },
        options: main_options(None),
        replies: [
            openai_reply(
                &json!({"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "echo", "arguments": "{}"}}
                ]}),
                "tool_calls",
            ),
            openai_text("a lighthouse"),
            openai_text("nothing new"),
            openai_text("yes"),
        ],
    }
}

#[tokio::test]
async fn anthropic_requests_extend_each_other_across_turns_and_a_restart() {
    let wire = anthropic_wire();
    let bodies = converse(&wire).await;

    assert_eq!(bodies.len(), 4, "two calls in turn one, one each after");
    assert_each_request_extends_the_last(&wire, &bodies);
    let after_restart = bodies.last().unwrap();
    for (sent, what) in [
        ("\"type\":\"image\"", "the picture"),
        ("\"type\":\"tool_result\"", "the tool result"),
        (MID_TURN_TEXT, "the mid-turn message"),
        (BETWEEN_TURNS_TEXT, "the message between turns"),
        ("\"signature\":\"sig-1\"", "the tool call's reasoning"),
        (
            "\"signature\":\"sig-3\"",
            "the second turn's reasoning, bound before the restart",
        ),
    ] {
        assert!(
            after_restart.contains(sent),
            "the conversation sends {what} in its last request"
        );
    }
}

#[tokio::test]
async fn openai_compatible_requests_extend_each_other_across_turns_and_a_restart() {
    let wire = openai_compatible_wire();
    let bodies = converse(&wire).await;

    assert_eq!(bodies.len(), 4, "two calls in turn one, one each after");
    assert_each_request_extends_the_last(&wire, &bodies);
    let after_restart = bodies.last().unwrap();
    for (sent, what) in [
        ("\"image_url\"", "the picture"),
        ("\"tool_call_id\":\"call_1\"", "the tool result"),
        (MID_TURN_TEXT, "the mid-turn message"),
        (BETWEEN_TURNS_TEXT, "the message between turns"),
    ] {
        assert!(
            after_restart.contains(sent),
            "the conversation sends {what} in its last request"
        );
    }
}

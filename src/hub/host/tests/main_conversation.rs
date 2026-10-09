//! The main conversation as a web client sees it, against a real hub: every
//! main-agent turn reaches an open WebSocket in order, whichever endpoint
//! started it, and a person's message is announced wherever it enters.

use super::agent_watch::agent_bus;
use super::*;
use crate::bus::{MessageEvent, topics};
use crate::inference::MessageSender;
use crate::interfaces::types::MessageOrigin;

type AgentSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the agent's WebSocket with tool frames on, and wait until the server
/// has subscribed it to the bus: it answers a ping only once it has.
async fn connect(hub: &Fixture, name: &str) -> AgentSocket {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/{name}/ws", hub.addr))
            .await
            .unwrap();
    send(&mut ws, json!({ "type": "set_verbose", "enabled": true })).await;
    send(&mut ws, json!({ "type": "ping" })).await;
    let pong = next_frame(&mut ws).await;
    assert_eq!(str_at(&pong, "type"), "pong");
    ws
}

async fn send(ws: &mut AgentSocket, frame: Value) {
    ws.send(WsMessage::text(frame.to_string())).await.unwrap();
}

async fn next_frame(ws: &mut AgentSocket) -> Value {
    tokio::time::timeout(POLL_TIMEOUT, async {
        loop {
            let frame = ws.next().await.expect("the WebSocket stays open").unwrap();
            if let WsMessage::Text(raw) = frame {
                return serde_json::from_str(&raw).unwrap();
            }
        }
    })
    .await
    .expect("a frame arrives within the timeout")
}

/// Every frame up to and including the `turn_ended` of the turn named
/// `turn_id`, or of the next turn to end when `turn_id` is `None`.
async fn frames_until_turn_ends(ws: &mut AgentSocket, turn_id: Option<&str>) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(ws).await;
        let ended = str_at(&frame, "type") == "turn_ended"
            && turn_id.is_none_or(|id| str_at(&frame, "reply_to") == id);
        frames.push(frame);
        if ended {
            return frames;
        }
    }
}

/// The `type` of each frame, without the usage ticks that come and go with
/// each model call.
fn kinds(frames: &[Value]) -> Vec<&str> {
    frames
        .iter()
        .map(|frame| str_at(frame, "type"))
        .filter(|kind| *kind != "turn_usage")
        .collect()
}

fn frame_of<'a>(frames: &'a [Value], kind: &str) -> &'a Value {
    frames
        .iter()
        .find(|frame| str_at(frame, "type") == kind)
        .unwrap_or_else(|| panic!("no {kind} frame in {frames:?}"))
}

/// The owner's direct message to the agent on Telegram.
fn telegram_message(id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        id: id.to_string(),
        content: content.to_string(),
        origin: MessageOrigin {
            endpoint: "telegram".to_string(),
            sender: Some(MessageSender {
                name: "Bear".to_string(),
                id: "42".to_string(),
                interface: "telegram".to_string(),
                location: Some("direct message".to_string()),
            }),
            conversation: None,
            agent_sender: None,
        },
        timestamp: chrono::Utc::now().naive_utc(),
        images: Vec::new(),
        context: None,
    }
}

async fn deliver(hub: &Fixture, agent: &str, message: MessageEvent) {
    agent_bus(hub, agent)
        .publisher()
        .publish(topics::UserMessage, message)
        .await
        .unwrap();
}

/// A teammate's message to `agent`, which starts a background turn.
async fn message_from_teammate(hub: &Fixture, agent: &str, text: &str) {
    use crate::hub::team::{TeamLink, parse_team_address};

    let link = TeamLink::new("guest", Arc::clone(&hub.services.team_router));
    let target = parse_team_address(&format!("agent:{agent}"))
        .unwrap()
        .unwrap();
    let main =
        crate::bus::SessionAddress::from(crate::background::registry::MAIN_ADDRESS.to_string());
    link.send(&main, &target, text.to_string(), 0)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_turn_that_started_on_telegram_reaches_the_web_with_its_origin() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut ws = connect(&hub, "scout").await;

    deliver(&hub, "scout", telegram_message("tg-1", "status?")).await;
    let frames = frames_until_turn_ends(&mut ws, Some("tg-1")).await;

    assert_eq!(
        kinds(&frames),
        ["user_message", "turn_started", "response", "turn_ended"],
        "the web follows a turn that never touched the web endpoint"
    );
    let message = frame_of(&frames, "user_message");
    assert_eq!(str_at(message, "id"), "tg-1");
    assert_eq!(str_at(message, "turn_id"), "tg-1");
    assert_eq!(str_at(message, "content"), "status?");
    assert_eq!(str_at(message, "endpoint"), "telegram");
    assert_eq!(message["sender"]["name"], "Bear");
    let started = frame_of(&frames, "turn_started");
    assert_eq!(str_at(started, "reply_to"), "tg-1");
    assert_eq!(started["origin"]["endpoint"], "telegram");
    assert_eq!(started["origin"]["visibility"], "user");
    assert_eq!(started["origin"]["sender"]["id"], "42");
    let response = frame_of(&frames, "response");
    assert_eq!(str_at(response, "reply_to"), "tg-1");
    assert_eq!(response["call"], 0);
    assert_eq!(str_at(response, "endpoint"), "telegram");
    assert_eq!(str_at(response, "content"), "scout here");
}

#[tokio::test]
async fn a_message_from_the_web_starts_its_own_turn_and_is_echoed_under_its_id() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut ws = connect(&hub, "scout").await;

    send(
        &mut ws,
        json!({ "type": "send_message", "id": "web-1", "content": "hello" }),
    )
    .await;
    let frames = frames_until_turn_ends(&mut ws, Some("web-1")).await;

    assert_eq!(
        kinds(&frames),
        ["user_message", "turn_started", "response", "turn_ended"]
    );
    let message = frame_of(&frames, "user_message");
    assert_eq!(str_at(message, "id"), "web-1");
    assert_eq!(str_at(message, "endpoint"), "ws");
    assert_eq!(
        frame_of(&frames, "turn_started")["origin"]["endpoint"],
        "ws"
    );
    assert_eq!(str_at(frame_of(&frames, "response"), "endpoint"), "ws");
}

/// A model that asks for one tool call, slowly, then answers.
async fn script_one_tool_call_then_a_reply(hub: &Fixture, agent: &str, call_delay: Duration) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(call_delay)
                .set_body_json(json!({
                    "choices": [{ "message": {
                        "role": "assistant",
                        "content": "Looking.",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "list_endpoints", "arguments": "{}" }
                        }]
                    } }]
                })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(hub.mock(agent))
        .await;
}

#[tokio::test]
async fn a_web_message_sent_during_a_telegram_turn_joins_it_and_the_web_sees_it_end() {
    let hub = Fixture::new(&["scout"], "").await;
    script_one_tool_call_then_a_reply(&hub, "scout", Duration::from_millis(800)).await;
    hub.host.start("scout").await.unwrap();
    let mut ws = connect(&hub, "scout").await;

    deliver(
        &hub,
        "scout",
        telegram_message("tg-2", "check the endpoints"),
    )
    .await;
    let mut frames = Vec::new();
    loop {
        let frame = next_frame(&mut ws).await;
        let started = str_at(&frame, "type") == "turn_started";
        frames.push(frame);
        if started {
            break;
        }
    }
    // The turn is in its first, slow model call: this message is folded in
    // at the next checkpoint instead of starting a turn of its own.
    send(
        &mut ws,
        json!({ "type": "send_message", "id": "web-2", "content": "and the inbox" }),
    )
    .await;
    frames.extend(frames_until_turn_ends(&mut ws, Some("tg-2")).await);

    assert_eq!(
        kinds(&frames),
        [
            "user_message",
            "turn_started",
            "broadcast_response",
            "tool_call",
            "tool_result",
            "user_message",
            "response",
            "turn_ended",
        ],
        "the web message appears between the tool result and the reply, where the model reads it"
    );
    let joined = frames
        .iter()
        .filter(|frame| str_at(frame, "type") == "user_message")
        .nth(1)
        .unwrap();
    assert_eq!(str_at(joined, "id"), "web-2");
    assert_eq!(str_at(joined, "turn_id"), "tg-2");
    assert_eq!(str_at(joined, "endpoint"), "ws");
    assert_eq!(
        frames
            .iter()
            .filter(|frame| str_at(frame, "type") == "turn_started")
            .count(),
        1,
        "joining a running turn starts no turn of its own"
    );
    assert_eq!(str_at(frame_of(&frames, "response"), "reply_to"), "tg-2");
    assert_eq!(frame_of(&frames, "tool_call")["call"], 0);
    assert_eq!(frame_of(&frames, "response")["call"], 1);
}

#[tokio::test]
async fn a_background_turn_is_announced_with_its_visibility_live_and_in_history() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    // The owner's last endpoint is the web, where background replies follow.
    assert_eq!(hub.chat("scout", "hello").await, "scout here");
    let mut ws = connect(&hub, "scout").await;

    message_from_teammate(&hub, "scout", "status check").await;
    let frames = frames_until_turn_ends(&mut ws, None).await;

    assert_eq!(
        kinds(&frames),
        ["turn_started", "response", "turn_ended"],
        "no person's message started it"
    );
    let started = frame_of(&frames, "turn_started");
    assert_eq!(started["origin"]["endpoint"], "background");
    assert_eq!(started["origin"]["visibility"], "background");
    assert!(started["origin"].get("sender").is_none(), "{started}");
    assert_eq!(str_at(frame_of(&frames, "response"), "endpoint"), "ws");

    let (status, body) = hub.get("/api/agents/scout/chat/history").await;
    assert_eq!(status, 200, "{body}");
    let history: Value = serde_json::from_str(&body).unwrap();
    let turn_id = str_at(started, "reply_to");
    let of_turn: Vec<&Value> = array_at(&history, "messages")
        .iter()
        .filter(|message| message.get("turn_id") == Some(&json!(turn_id)))
        .collect();
    assert!(!of_turn.is_empty(), "history records the background turn");
    assert!(
        of_turn
            .iter()
            .all(|message| str_at(message, "visibility") == "background"),
        "after a reload the turn is still marked background: {of_turn:?}"
    );
}

#[tokio::test]
async fn a_background_turn_with_no_endpoint_to_follow_still_reaches_the_web() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut ws = connect(&hub, "scout").await;

    message_from_teammate(&hub, "scout", "status check").await;
    let frames = frames_until_turn_ends(&mut ws, None).await;

    assert_eq!(kinds(&frames), ["turn_started", "response", "turn_ended"]);
    assert_eq!(
        str_at(frame_of(&frames, "response"), "endpoint"),
        "",
        "the reply went to no endpoint, and the frame says so"
    );
}

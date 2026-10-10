//! Tests for the hub socket's artifact events and session relay, against the
//! fake directory's agents: `scout` (running), `quiet` (stopped) and the
//! session events the harness's feed of agent changes carries.
//!
//! The artifact events come from the hub's own watcher over a real team
//! change feed. Session events are relayed the way the per-agent watcher
//! relays them: onto the harness's feed.

use super::*;
use crate::background::registry::{SessionCategory, SessionInfo, SessionState};
use crate::bus::{
    AgentResultStatus, EventTrigger, SessionAddress, SessionEvent, SessionEventKind, ToolCallEvent,
    ToolResultEvent,
};
use crate::hub::agent_watch::AgentSessionEvent;
use crate::hub::services::TeamChangeFeed;

/// How many session events a connection can fall behind by.
const RELAY_CAPACITY: usize = 1024;

fn session_info(address: &str, label: &str) -> SessionInfo {
    SessionInfo {
        address: SessionAddress::from(address),
        run_id: "run-1".to_string(),
        category: SessionCategory::Artifact,
        trigger: EventTrigger::Artifact(label.to_string()),
        source_label: label.to_string(),
        state: SessionState::Forking,
        spawner: None,
        depth: 1,
        purpose: "draft the page".to_string(),
        agent_skill: None,
        model_tier: crate::config::BackgroundModelTier::default(),
        conversation_target: None,
        started_at: Utc::now(),
        usage: crate::agent::usage::SessionUsageTotals::default(),
        overlap: None,
    }
}

/// Relay one event of `agent`'s session at `address`, labelled `label`, the
/// way the agent's watcher does.
fn relay(h: &Harness, agent: &str, label: Option<&str>, address: &str, kind: SessionEventKind) {
    h.changes.relay_session(AgentSessionEvent {
        agent: agent.to_string(),
        source_label: label.map(str::to_string),
        event: SessionEvent {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            kind,
        },
    });
}

fn turn_started(turn: &str) -> SessionEventKind {
    SessionEventKind::TurnStarted {
        turn_id: turn.to_string(),
    }
}

/// A hub socket past its `hub_boot` and `agents_snapshot`.
async fn connect_ready(h: &Harness) -> ClientSocket {
    let mut socket = connect_hub(h.serve().await).await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");
    socket
}

async fn subscribe_session(socket: &mut ClientSocket, agent: &str, address: &str) -> Value {
    send_client(
        socket,
        &json!({ "type": "subscribe_session", "agent": agent, "address": address }),
    )
    .await;
    next_frame(socket).await
}

async fn subscribe_artifact(socket: &mut ClientSocket, artifact: &str) -> Value {
    send_client(
        socket,
        &json!({ "type": "subscribe_artifact_sessions", "artifact": artifact }),
    )
    .await;
    next_frame(socket).await
}

/// What a `session_frame` carries: the agent, the frame's type and its address.
fn described(frame: &Value) -> (String, String, String) {
    assert_eq!(frame["type"], "session_frame", "{frame}");
    let inner = &frame["frame"];
    let address = inner["address"]
        .as_str()
        .or_else(|| inner["session"]["address"].as_str())
        .unwrap_or_default();
    (
        frame["agent"].as_str().unwrap().to_string(),
        inner["type"].as_str().unwrap().to_string(),
        address.to_string(),
    )
}

#[tokio::test]
async fn artifact_events_reach_the_socket_with_no_agent_running() {
    // The hub's own watcher over a real team change feed: no agent runs
    // anywhere in this test.
    let team = tempfile::tempdir().unwrap();
    let team_root = team.path().join("team");
    let feed = TeamChangeFeed::start(team_root.clone()).await;
    let h = Harness::over_team_bus(feed.bus.clone());
    let mut socket = connect_ready(&h).await;

    // The feed arms its OS watcher on its own task, and a file written before
    // the watch is placed is never reported. The watcher is a separate thread
    // that a busy machine can starve, so write only once the feed says it runs.
    let mut health = feed.health.clone();
    tokio::time::timeout(
        Duration::from_secs(30),
        health.wait_for(|health| *health != WatchHealth::Starting),
    )
    .await
    .expect("the team change feed never started its watcher")
    .unwrap();
    crate::workspace::watch::assert_native_watch(*health.borrow(), "the team change feed");

    let workbench = team_root.join("workbench");
    std::fs::create_dir_all(&workbench).unwrap();
    std::fs::write(workbench.join("chart.html"), "<p>v1</p>").unwrap();
    // A frame follows an OS file notification, delivered on the watcher's own
    // thread, and the feed's debounce on the real clock, so a paused clock
    // can't stand in for either. `next_frame`'s 5s bound is meant for frames
    // the hub sends at once, so read the socket directly: the bound here only
    // turns a notification that never comes into a failure instead of a hang.
    let next = async |connection: &mut ClientSocket| {
        let message = tokio::time::timeout(Duration::from_secs(30), connection.next())
            .await
            .expect("timed out waiting for an artifact frame")
            .expect("socket closed")
            .expect("socket error");
        match message {
            ClientMessage::Text(text) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other @ (ClientMessage::Binary(_)
            | ClientMessage::Ping(_)
            | ClientMessage::Pong(_)
            | ClientMessage::Close(_)
            | ClientMessage::Frame(_)) => panic!("expected a text frame, got {other:?}"),
        }
    };
    assert_eq!(
        next(&mut socket).await,
        json!({ "type": "artifact_updated", "name": "chart" })
    );

    std::fs::remove_file(workbench.join("chart.html")).unwrap();
    assert_eq!(
        next(&mut socket).await,
        json!({ "type": "artifact_removed", "name": "chart" })
    );
}

#[tokio::test]
async fn a_subscribed_session_streams_every_event_tool_frames_included() {
    let h = Harness::new();
    let mut socket = connect_ready(&h).await;
    let label = Some("artifact:chart");

    // A session that finished before the subscription is not replayed.
    relay(&h, "scout", label, "spawned-a", turn_started("t-0"));
    let ack = subscribe_session(&mut socket, "scout", "spawned-a").await;
    assert_eq!(
        ack,
        json!({ "type": "subscribed", "kind": "session", "agent": "scout", "address": "spawned-a" })
    );

    let kinds = [
        SessionEventKind::Started(Box::new(session_info("spawned-a", "artifact:chart"))),
        SessionEventKind::StateChanged(SessionState::Running),
        turn_started("t-1"),
        SessionEventKind::Intermediate {
            content: "Looking.".to_string(),
        },
        SessionEventKind::ToolCall(ToolCallEvent {
            correlation_id: "c-1".to_string(),
            tool_call_id: "tc-1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({ "path": "notes.md" }),
            server: None,
        }),
        SessionEventKind::ToolResult(ToolResultEvent {
            correlation_id: "c-1".to_string(),
            tool_call_id: "tc-1".to_string(),
            name: "read_file".to_string(),
            output: "three notes".to_string(),
            is_error: false,
            auto_mode: None,
        }),
        SessionEventKind::Response {
            turn_id: "t-1".to_string(),
            content: "Done.".to_string(),
        },
        SessionEventKind::TurnEnded {
            turn_id: "t-1".to_string(),
        },
        SessionEventKind::Completed {
            status: AgentResultStatus::Completed,
            episode_id: None,
        },
    ];
    for kind in kinds {
        relay(&h, "scout", label, "spawned-a", kind);
    }

    let mut seen = Vec::new();
    for _ in 0..9 {
        let frame = next_frame(&mut socket).await;
        let (agent, kind, address) = described(&frame);
        assert_eq!((agent.as_str(), address.as_str()), ("scout", "spawned-a"));
        seen.push((kind, frame));
    }
    let types: Vec<&str> = seen.iter().map(|(kind, _)| kind.as_str()).collect();
    assert_eq!(
        types,
        [
            "session_started",
            "session_state_changed",
            "session_turn_started",
            "session_broadcast_response",
            "session_tool_call",
            "session_tool_result",
            "session_response",
            "session_turn_ended",
            "session_completed",
        ],
        "the stream arrives whole and in order, with no verbose flag set"
    );
    let call = &seen[4].1["frame"];
    assert_eq!(call["name"], "read_file");
    assert_eq!(call["arguments"], json!({ "path": "notes.md" }));
    assert_eq!(seen[5].1["frame"]["output"], "three notes");
    assert_eq!(
        seen[0].1["frame"]["session"]["source_label"],
        "artifact:chart"
    );
}

#[tokio::test]
async fn a_subscription_is_acknowledged_before_any_frame() {
    let h = Harness::new();
    let mut socket = connect_ready(&h).await;

    // An event of the session is already waiting when the subscription
    // arrives. Whether the hub reads it before or after the subscription, no
    // frame comes ahead of the acknowledgement.
    relay(&h, "scout", None, "spawned-a", turn_started("t-1"));
    send_client(
        &mut socket,
        &json!({ "type": "subscribe_session", "agent": "scout", "address": "spawned-a" }),
    )
    .await;
    assert_eq!(next_frame(&mut socket).await["type"], "subscribed");

    relay(&h, "scout", None, "spawned-a", turn_started("t-2"));
    let mut frame = next_frame(&mut socket).await;
    if frame["frame"]["turn_id"] == "t-1" {
        frame = next_frame(&mut socket).await;
    }
    assert_eq!(frame["frame"]["turn_id"], "t-2");

    let artifact_ack = subscribe_artifact(&mut socket, "chart").await;
    assert_eq!(
        artifact_ack,
        json!({ "type": "subscribed", "kind": "artifact_sessions", "artifact": "chart" })
    );
}

#[tokio::test]
async fn an_artifact_subscription_follows_sessions_started_later_on_two_agents() {
    let h = Harness::new();
    h.directory
        .agents
        .lock()
        .unwrap()
        .push(summary("nova", AgentState::Running));
    let mut socket = connect_ready(&h).await;
    subscribe_artifact(&mut socket, "chart").await;

    // Neither session exists yet. Each starts after the subscription.
    relay(
        &h,
        "scout",
        Some("artifact:chart"),
        "artifact-chart-0001",
        SessionEventKind::Started(Box::new(session_info(
            "artifact-chart-0001",
            "artifact:chart",
        ))),
    );
    relay(
        &h,
        "nova",
        Some("artifact:chart"),
        "artifact-chart-0001",
        SessionEventKind::Started(Box::new(session_info(
            "artifact-chart-0001",
            "artifact:chart",
        ))),
    );
    // Sessions of another artifact, another source, and one whose label is
    // unknown are not the artifact's.
    relay(
        &h,
        "scout",
        Some("artifact:other"),
        "artifact-other-0001",
        turn_started("t-x"),
    );
    relay(
        &h,
        "scout",
        Some("pulse:chart"),
        "scheduled-chart",
        turn_started("t-y"),
    );
    relay(&h, "nova", None, "mystery", turn_started("t-z"));
    // Later events of the two sessions follow, labelled as their start was.
    relay(
        &h,
        "nova",
        Some("artifact:chart"),
        "artifact-chart-0001",
        turn_started("t-1"),
    );
    relay(
        &h,
        "scout",
        Some("artifact:chart"),
        "artifact-chart-0001",
        turn_started("t-1"),
    );

    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(described(&next_frame(&mut socket).await));
    }
    let want = |agent: &str, kind: &str| {
        (
            agent.to_string(),
            kind.to_string(),
            "artifact-chart-0001".to_string(),
        )
    };
    assert_eq!(
        seen,
        [
            want("scout", "session_started"),
            want("nova", "session_started"),
            want("nova", "session_turn_started"),
            want("scout", "session_turn_started"),
        ]
    );
}

#[tokio::test]
async fn unsubscribing_stops_delivery() {
    let h = Harness::new();
    let mut socket = connect_ready(&h).await;
    subscribe_session(&mut socket, "scout", "spawned-a").await;
    subscribe_artifact(&mut socket, "chart").await;

    // An unsubscribe has no answer, but the hub reads a connection's messages
    // in order, so the answer to a message sent after it shows it was handled.
    send_client(
        &mut socket,
        &json!({ "type": "unsubscribe_session", "agent": "scout", "address": "spawned-a" }),
    )
    .await;
    send_client(&mut socket, &json!({ "type": "nonsense" })).await;
    assert_eq!(next_frame(&mut socket).await["type"], "notice");
    relay(&h, "scout", None, "spawned-a", turn_started("t-1"));
    relay(
        &h,
        "scout",
        Some("artifact:chart"),
        "artifact-chart-0001",
        turn_started("t-2"),
    );
    // The artifact's frame is the first to arrive, so the session's event was
    // dropped before it.
    let artifact_frame = next_frame(&mut socket).await;
    assert_eq!(
        artifact_frame["frame"]["turn_id"], "t-2",
        "{artifact_frame}"
    );

    send_client(
        &mut socket,
        &json!({ "type": "unsubscribe_artifact_sessions", "artifact": "chart" }),
    )
    .await;
    subscribe_session(&mut socket, "scout", "spawned-b").await;
    relay(
        &h,
        "scout",
        Some("artifact:chart"),
        "artifact-chart-0001",
        turn_started("t-3"),
    );
    relay(&h, "scout", None, "spawned-b", turn_started("t-4"));
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["frame"]["turn_id"], "t-4", "{frame}");

    // Unsubscribing what was never followed changes nothing.
    send_client(
        &mut socket,
        &json!({ "type": "unsubscribe_session", "agent": "ghost", "address": "x" }),
    )
    .await;
    relay(&h, "scout", None, "spawned-b", turn_started("t-5"));
    assert_eq!(next_frame(&mut socket).await["frame"]["turn_id"], "t-5");
}

#[tokio::test]
async fn a_connection_that_falls_behind_is_told_to_read_its_sessions_again() {
    let h = Harness::new();
    let mut following = connect_ready(&h).await;
    subscribe_session(&mut following, "scout", "spawned-a").await;
    let mut idle = connect_ready(&h).await;

    // Nothing runs on this thread between the sends, so neither connection
    // reads any of them before the relay has overflowed.
    for n in 0..RELAY_CAPACITY + 50 {
        relay(&h, "scout", None, "spawned-a", turn_started(&n.to_string()));
    }
    let lagged = next_frame(&mut following).await;
    assert_eq!(lagged, json!({ "type": "session_relay_lagged" }));
    // The events that are still held follow it.
    let frame = next_frame(&mut following).await;
    assert_eq!(frame["type"], "session_frame");

    // A connection that follows nothing lost nothing, so it hears no lag
    // notice among the answers it gets while the relay is read.
    for _ in 0..8 {
        send_client(&mut idle, &json!({ "type": "nonsense" })).await;
        assert_eq!(next_frame(&mut idle).await["type"], "notice");
    }
}

#[tokio::test]
async fn an_unknown_agent_gets_a_notice_and_no_acknowledgement() {
    let h = Harness::new();
    let mut socket = connect_ready(&h).await;

    let refusal = subscribe_session(&mut socket, "ghost", "spawned-a").await;
    assert_eq!(refusal["type"], "notice");
    assert_eq!(refusal["level"], "warn");
    assert!(refusal["message"].as_str().unwrap().contains("ghost"));

    // Nothing for it is delivered, and no acknowledgement is pending: the
    // next frame is the one the following subscription earns. A stopped agent
    // is a known one.
    relay(&h, "ghost", None, "spawned-a", turn_started("t-1"));
    let ack = subscribe_session(&mut socket, "quiet", "spawned-q").await;
    assert_eq!(
        ack,
        json!({ "type": "subscribed", "kind": "session", "agent": "quiet", "address": "spawned-q" })
    );
    relay(&h, "quiet", None, "spawned-q", turn_started("t-2"));
    assert_eq!(next_frame(&mut socket).await["frame"]["turn_id"], "t-2");

    // A subscription message missing a field is refused like any message the
    // hub can't read.
    send_client(
        &mut socket,
        &json!({ "type": "subscribe_session", "agent": "scout" }),
    )
    .await;
    assert_eq!(next_frame(&mut socket).await["type"], "notice");
}

#[tokio::test]
async fn subscriptions_end_with_the_connection() {
    let h = Harness::new();
    let mut first = connect_ready(&h).await;
    subscribe_session(&mut first, "scout", "spawned-a").await;
    subscribe_artifact(&mut first, "chart").await;
    assert_eq!(h.changes.session_relay_receivers(), 1);

    first.close(None).await.unwrap();
    drop(first);
    for _ in 0..100 {
        if h.changes.session_relay_receivers() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        h.changes.session_relay_receivers(),
        0,
        "the closed connection's end of the relay is gone"
    );

    // A connection that opens afterwards follows none of it.
    let mut second = connect_ready(&h).await;
    relay(&h, "scout", None, "spawned-a", turn_started("t-1"));
    relay(
        &h,
        "scout",
        Some("artifact:chart"),
        "artifact-chart-0001",
        turn_started("t-2"),
    );
    for _ in 0..8 {
        send_client(&mut second, &json!({ "type": "nonsense" })).await;
        assert_eq!(next_frame(&mut second).await["type"], "notice");
    }
}

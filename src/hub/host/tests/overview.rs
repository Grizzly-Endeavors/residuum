//! The team overview against real agents: what a running agent does reaches
//! its overview and the hub socket's `agent_overview` frames, and a stopped
//! agent is answered from its files.

use super::*;
use crate::hub::overview::{AgentOverview, LastMessageRole};
use crate::hub::team::{TeamLink, parse_team_address};
use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::Visibility;

/// The next frame of `agent`'s overview that satisfies `wanted`.
async fn overview_frame(
    frames: &mut broadcast::Receiver<AgentOverview>,
    agent: &str,
    what: &str,
    wanted: impl Fn(&AgentOverview) -> bool,
) -> AgentOverview {
    tokio::time::timeout(POLL_TIMEOUT, async {
        loop {
            match frames.recv().await {
                Ok(frame) if frame.name == agent && wanted(&frame) => return frame,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    panic!("the overview stopped sending frames")
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// What `GET /api/hub/overview` says about `agent`.
async fn overview_of(hub: &Fixture, agent: &str) -> Value {
    let (status, body) = hub.get("/api/hub/overview").await;
    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    array_at(&body, "agents")
        .iter()
        .find(|entry| str_at(entry, "name") == agent)
        .unwrap_or_else(|| panic!("no overview for {agent} in {body}"))
        .clone()
}

/// Let the model behind `agent` answer every request with `text`.
async fn answer_with(hub: &Fixture, agent: &str, text: &str) {
    hub.mock(agent).reset().await;
    mount_reply(hub.mock(agent), text, Duration::ZERO).await;
}

#[tokio::test]
async fn a_turn_the_user_was_part_of_is_the_last_message_and_a_background_turn_is_not() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut frames = hub.overview.subscribe();

    answer_with(&hub, "scout", "**Sunny** today").await;
    hub.chat("scout", "how is the weather").await;
    let reply = overview_frame(&mut frames, "scout", "the reply", |frame| {
        frame
            .last_message
            .as_ref()
            .is_some_and(|last| last.preview == "Sunny today")
    })
    .await;
    let last = reply.last_message.unwrap();
    assert_eq!(last.role, LastMessageRole::Assistant);
    chrono::DateTime::parse_from_rfc3339(&last.at).expect("`at` is RFC 3339 with an offset");
    assert_eq!(
        overview_of(&hub, "scout").await["last_message"]["preview"],
        "Sunny today"
    );

    // A teammate's message starts a background turn that still replies.
    answer_with(&hub, "scout", "pulse chatter").await;
    let mut turns = hub.host.agent_changes().subscribe();
    let link = TeamLink::new("guest", Arc::clone(&hub.services.team_router));
    let target = parse_team_address("agent:scout").unwrap().unwrap();
    let main =
        crate::bus::SessionAddress::from(crate::background::registry::MAIN_ADDRESS.to_string());
    link.send(&main, &target, "status check".to_string(), 0)
        .await
        .unwrap();
    loop {
        let change = tokio::time::timeout(POLL_TIMEOUT, turns.recv())
            .await
            .expect("the background turn ends")
            .unwrap();
        if matches!(change.kind, crate::hub::agent_watch::AgentChangeKind::TurnEnded(ref turn) if turn.visibility == Visibility::Background)
        {
            break;
        }
    }

    // Wait out the window a frame would have come in.
    tokio::time::sleep(OVERVIEW_WINDOW * 3).await;
    let mut shown = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        shown.extend(frame.last_message.map(|message| message.preview));
    }
    assert!(
        !shown.iter().any(|preview| preview == "pulse chatter"),
        "the background reply was never the last message: {shown:?}"
    );
    assert_eq!(
        overview_of(&hub, "scout").await["last_message"]["preview"],
        "Sunny today"
    );

    answer_with(&hub, "scout", "second answer").await;
    hub.chat("scout", "and tomorrow").await;
    overview_frame(&mut frames, "scout", "the second reply", |frame| {
        frame
            .last_message
            .as_ref()
            .is_some_and(|message| message.preview == "second answer")
    })
    .await;
}

#[tokio::test]
async fn a_started_agents_last_message_is_read_from_its_history_on_disk() {
    let hub = Fixture::new(&["scout"], "").await;
    let layout = WorkspaceLayout::new(hub.root.path().join("scout"));
    std::fs::create_dir_all(layout.memory_dir()).unwrap();
    let recent = vec![RecentMessage {
        message: crate::inference::Message::user("remember the milk"),
        timestamp: chrono::NaiveDate::from_ymd_opt(2026, 9, 30)
            .unwrap()
            .and_hms_opt(8, 15, 0)
            .unwrap(),
        visibility: Visibility::User,
        turn_id: None,
    }];
    std::fs::write(
        layout.recent_messages_json(),
        serde_json::to_string(&recent).unwrap(),
    )
    .unwrap();
    let stopped = overview_of(&hub, "scout").await;
    assert_eq!(stopped["last_message"]["preview"], "remember the milk");

    hub.host.start("scout").await.unwrap();

    let running = overview_of(&hub, "scout").await;
    assert_eq!(
        running["last_message"],
        json!({
            "role": "user",
            "preview": "remember the milk",
            "at": "2026-09-30T08:15:00Z",
            "at_precision": "minute",
        }),
        "a running agent starts from its files until a turn replaces them"
    );
}

#[tokio::test]
async fn a_session_is_live_with_its_source_label_until_it_ends_and_stopping_the_agent_ends_it() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut frames = hub.overview.subscribe();

    hub.start_session("scout", "look into it").await;

    let live = overview_frame(&mut frames, "scout", "the live session", |frame| {
        !frame.live_sessions.is_empty()
    })
    .await;
    assert_eq!(live.live_sessions.len(), 1, "{live:?}");
    let session = &live.live_sessions[0];
    assert_eq!(session.source_label, "artifact:test-artifact");
    assert_eq!(
        serde_json::to_value(session.category).unwrap(),
        "artifact",
        "{session:?}"
    );
    assert!(session.purpose.contains("look into it"), "{session:?}");
    assert_eq!(
        overview_of(&hub, "scout").await["live_sessions"][0]["run_id"],
        session.run_id
    );

    hub.host.stop("scout").await.unwrap();

    overview_frame(&mut frames, "scout", "the session to clear", |frame| {
        frame.live_sessions.is_empty()
    })
    .await;
    assert_eq!(overview_of(&hub, "scout").await["live_sessions"], json!([]));
}

#[tokio::test]
async fn the_unread_count_follows_the_inbox_tool_a_hand_placed_file_and_the_hubs_actions() {
    let hub = Fixture::new(&["scout"], "").await;
    mount_script(hub.mock("scout"), "filed it", |role, content| {
        is_user_message_with(role, content, "file a note").then(|| {
            (
                "user_inbox_add",
                json!({ "title": "Heads up", "body": "look at this" }),
            )
        })
    })
    .await;
    hub.host.start("scout").await.unwrap();
    assert_eq!(overview_of(&hub, "scout").await["inbox_unread"], 0);
    let mut frames = hub.overview.subscribe();

    hub.chat("scout", "please file a note").await;
    overview_frame(&mut frames, "scout", "the saved item", |frame| {
        frame.inbox_unread == 1
    })
    .await;

    hub.add_inbox_item("scout", "20260930_hand-placed");
    overview_frame(&mut frames, "scout", "the hand-placed file", |frame| {
        frame.inbox_unread == 2
    })
    .await;

    let response = hub
        .http
        .put(hub.url("/api/hub/inbox/scout/20260930_hand-placed/read"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{response:?}");
    overview_frame(
        &mut frames,
        "scout",
        "the count after the hub's read",
        |frame| frame.inbox_unread == 1,
    )
    .await;
    assert_eq!(overview_of(&hub, "scout").await["inbox_unread"], 1);
}

#[tokio::test]
async fn a_stopped_agents_counts_and_history_come_from_its_files_and_it_shows_nothing_live() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.add_inbox_item("scout", "20260930_waiting");

    let stopped = overview_of(&hub, "scout").await;

    assert_eq!(stopped["inbox_unread"], 1, "{stopped}");
    assert_eq!(stopped["live_sessions"], json!([]));
    assert_eq!(stopped["last_message"], Value::Null);
    assert_eq!(stopped["upcoming"], json!([]));
    assert_eq!(stopped["outbound_problems"], json!([]));
}

#[tokio::test]
async fn the_hub_socket_sends_an_agents_overview_after_a_chat_turn() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hub/ws", hub.addr))
        .await
        .unwrap();

    answer_with(&hub, "scout", "Hello from scout").await;
    hub.chat("scout", "hi").await;

    let frame = frame_where(&mut socket, "agent_overview", |frame| {
        frame["overview"]["last_message"]["preview"] == "Hello from scout"
    })
    .await;
    assert_eq!(frame["overview"]["name"], "scout");
    assert_eq!(frame["overview"]["last_message"]["role"], "assistant");
}

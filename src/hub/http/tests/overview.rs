//! Tests for `GET /api/hub/overview` and the overview frames, against the
//! fake directory's agents: `scout` (running), `quiet` (stopped) and, in some
//! tests, `broken` (failed).
//!
//! A change reaches the overview the way it does in the hub: published on the
//! harness's feed of agent changes, or on the directory's events. Files are
//! written where the agent's own would be.

use chrono::{NaiveDate, NaiveDateTime, TimeZone as _};
use tokio::time::Instant;

use super::*;
use crate::background::registry::{SessionCategory, SessionState};
use crate::bus::{AgentResultStatus, EventTrigger, SessionAddress};
use crate::hub::agent_watch::{AgentChange, AgentChangeKind, MainTurnEnded, WatchedPath};
use crate::hub::overview::{AgentOverview, LastMessageRole, TimePrecision};
use crate::inference::Message;
use crate::memory::episode_store::write_episode_transcript_tagged;
use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::{Episode, SourceTag, Visibility};

fn minute(hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 9, 30)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
}

/// `GET /api/hub/overview`, and the entry of `agent`.
pub(super) async fn overview_of(h: &Harness, agent: &str) -> Value {
    let body = h.get_expect("/api/hub/overview", StatusCode::OK).await;
    body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == agent)
        .unwrap_or_else(|| panic!("no overview for {agent} in {body}"))
        .clone()
}

/// Write `agent`'s recent history: `(message, visibility, when)`, oldest first.
fn write_recent(h: &Harness, agent: &str, messages: Vec<(Message, Visibility, NaiveDateTime)>) {
    let layout = WorkspaceLayout::new(h.agent_dir(agent));
    let recent: Vec<RecentMessage> = messages
        .into_iter()
        .map(|(message, visibility, timestamp)| RecentMessage {
            message,
            timestamp,
            visibility,
            turn_id: None,
        })
        .collect();
    std::fs::create_dir_all(layout.memory_dir()).unwrap();
    std::fs::write(
        layout.recent_messages_json(),
        serde_json::to_string(&recent).unwrap(),
    )
    .unwrap();
}

/// Write an episode of `agent`'s main conversation (or of a session run
/// when `session` is true) dated `date`.
async fn write_episode(
    h: &Harness,
    agent: &str,
    id: &str,
    date: NaiveDate,
    session: bool,
    messages: &[Message],
) {
    let dir = WorkspaceLayout::new(h.agent_dir(agent)).episodes_dir();
    let episode = Episode {
        id: id.to_string(),
        date,
        observations: Vec::new(),
    };
    let tag = if session {
        SourceTag::session("spawned-abc", "run-1", "spawned")
    } else {
        SourceTag::main()
    };
    write_episode_transcript_tagged(&dir, &episode, messages, &tag, None)
        .await
        .unwrap();
}

/// Save an unread active item `id` in `agent`'s user inbox.
async fn place_item(h: &Harness, agent: &str, id: &str) {
    let inbox = WorkspaceLayout::new(h.agent_dir(agent)).user_inbox_dir();
    tokio::fs::create_dir_all(&inbox).await.unwrap();
    let item = crate::inbox::InboxItem {
        title: format!("title of {id}"),
        body: String::new(),
        source: "test".to_string(),
        timestamp: minute(8, 0),
        read: false,
        attachments: Vec::new(),
    };
    crate::inbox::save_item(&inbox, &format!("{id}.json"), &item)
        .await
        .unwrap();
}

fn session_info(label: &str, state: SessionState) -> SessionInfo {
    SessionInfo {
        address: SessionAddress::from(format!("spawned-{label}")),
        run_id: format!("run-{label}"),
        category: SessionCategory::Artifact,
        trigger: EventTrigger::Artifact(label.to_string()),
        source_label: format!("artifact:{label}"),
        state,
        spawner: None,
        depth: 1,
        purpose: format!("draft the {label} page"),
        agent_skill: None,
        model_tier: crate::config::BackgroundModelTier::default(),
        conversation_target: None,
        started_at: Utc.with_ymd_and_hms(2026, 9, 30, 9, 30, 0).unwrap(),
        usage: crate::agent::usage::SessionUsageTotals::default(),
        overlap: None,
    }
}

/// Say that `agent` changed, as its watcher would.
pub(super) fn changed(h: &Harness, agent: &str, kind: AgentChangeKind) {
    h.changes.publish(&AgentChange {
        agent: agent.to_string(),
        kind,
    });
}

/// Say that `agent`'s main turn ended, and when.
fn turn_ended(
    h: &Harness,
    agent: &str,
    visibility: Visibility,
    user_message: Option<&str>,
    reply: Option<&str>,
) -> chrono::DateTime<Utc> {
    let at = Utc.with_ymd_and_hms(2026, 9, 30, 10, 5, 7).unwrap();
    changed(
        h,
        agent,
        AgentChangeKind::TurnEnded(MainTurnEnded {
            user_message: user_message.map(str::to_string),
            reply: reply.map(str::to_string),
            at,
            visibility,
            client_connected: false,
        }),
    );
    at
}

/// Move `agent` to `state`, and say so as the host does.
pub(super) fn move_to(h: &Harness, agent: &str, state: AgentState) {
    let moved = h.directory.set_state(agent, state).unwrap();
    h.directory
        .events
        .send(HubEvent::AgentState { agent: moved })
        .unwrap();
}

/// The next overview frame, within `within`.
pub(super) async fn frame_within(
    frames: &mut broadcast::Receiver<AgentOverview>,
    within: Duration,
) -> Option<AgentOverview> {
    tokio::time::timeout(within, frames.recv())
        .await
        .ok()
        .and_then(Result::ok)
}

/// The next overview frame, which must come.
pub(super) async fn next_overview(
    frames: &mut broadcast::Receiver<AgentOverview>,
) -> AgentOverview {
    frame_within(frames, Duration::from_secs(5))
        .await
        .expect("an overview frame arrives")
}

/// Check that no frame comes for several windows.
pub(super) async fn expect_no_frame(frames: &mut broadcast::Receiver<AgentOverview>) {
    let got = frame_within(frames, OVERVIEW_WINDOW * 3).await;
    assert!(got.is_none(), "no frame was due, but one came: {got:?}");
}

#[tokio::test]
async fn the_overview_lists_every_agent_by_name_in_the_contract_shape() {
    let h = Harness::new();
    h.add_failed_agent();

    let body = h.get_expect("/api/hub/overview", StatusCode::OK).await;

    assert_eq!(body["boot_id"], TEST_BOOT_ID, "{body}");
    let names: Vec<&str> = body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["broken", "quiet", "scout"]);
    assert_eq!(
        body["agents"][0],
        json!({
            "name": "broken",
            "last_message": null,
            "live_sessions": [],
            "upcoming": [],
            "inbox_unread": 0,
            "outbound_problems": [],
        }),
        "an agent with nothing to show"
    );
}

#[tokio::test]
async fn a_stopped_agents_last_message_is_the_newest_text_the_user_saw_in_its_recent_history() {
    let h = Harness::new();
    let tool_call = crate::inference::ToolCall {
        id: "call-1".to_string(),
        name: "read_file".to_string(),
        arguments: json!({}),
        server: None,
    };
    write_recent(
        &h,
        "quiet",
        vec![
            (
                Message::user("Can you check the **build**?"),
                Visibility::User,
                minute(8, 15),
            ),
            (
                Message::assistant(
                    "Yes, [the build](https://ci.example/1) is green.\n\nAll `42` tests pass.",
                    None,
                ),
                Visibility::User,
                minute(8, 16),
            ),
            (
                Message::assistant("", Some(vec![tool_call])),
                Visibility::User,
                minute(8, 17),
            ),
            (
                Message::tool("file contents", "call-1"),
                Visibility::User,
                minute(8, 18),
            ),
            (
                Message::assistant("pulse noise", None),
                Visibility::Background,
                minute(8, 19),
            ),
        ],
    );

    let last = overview_of(&h, "quiet").await["last_message"].clone();

    assert_eq!(
        last,
        json!({
            "role": "assistant",
            "preview": "Yes, the build is green. All 42 tests pass.",
            "at": "2026-09-30T08:16:00Z",
            "at_precision": "minute",
        }),
        "the tool call, the tool result and the background turn are passed over"
    );
}

#[tokio::test]
async fn a_message_time_is_read_in_the_hubs_timezone() {
    let h = Harness::new();
    *h.directory.timezone.lock().unwrap() = chrono_tz::America::New_York;
    write_recent(
        &h,
        "quiet",
        vec![(Message::user("hello"), Visibility::User, minute(8, 15))],
    );

    let last = overview_of(&h, "quiet").await["last_message"].clone();

    assert_eq!(last["at"], "2026-09-30T08:15:00-04:00", "{last}");
    assert_eq!(last["role"], "user");
}

#[tokio::test]
async fn with_no_recent_history_the_last_message_comes_from_the_newest_main_episode_dated_to_the_day()
 {
    let h = Harness::new();
    let day = |d| NaiveDate::from_ymd_opt(2026, 9, d).unwrap();
    write_episode(
        &h,
        "quiet",
        "ep-001",
        day(27),
        false,
        &[
            Message::user("old question"),
            Message::assistant("old answer", None),
        ],
    )
    .await;
    write_episode(
        &h,
        "quiet",
        "ep-002",
        day(28),
        false,
        &[
            Message::user("Plan the **trip**"),
            Message::assistant("Booked.", None),
        ],
    )
    .await;
    write_episode(
        &h,
        "quiet",
        "ep-003",
        day(29),
        true,
        &[Message::assistant("session chatter", None)],
    )
    .await;
    write_episode(
        &h,
        "quiet",
        "ep-004",
        day(30),
        false,
        &[Message::tool("only a tool result", "call-9")],
    )
    .await;

    let last = overview_of(&h, "quiet").await["last_message"].clone();

    assert_eq!(
        last,
        json!({
            "role": "assistant",
            "preview": "Booked.",
            "at": "2026-09-28T00:00:00Z",
            "at_precision": "day",
        }),
        "a session's episode and an episode with no text are passed over"
    );
}

#[tokio::test]
async fn recent_history_wins_over_an_episode_and_a_broken_file_falls_back_to_the_episodes() {
    let h = Harness::new();
    write_episode(
        &h,
        "quiet",
        "ep-001",
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        false,
        &[Message::user("from the episode")],
    )
    .await;
    write_recent(
        &h,
        "quiet",
        vec![(Message::user("from recent"), Visibility::User, minute(9, 0))],
    );
    let recent = overview_of(&h, "quiet").await["last_message"].clone();
    assert_eq!(recent["preview"], "from recent", "{recent}");

    let layout = WorkspaceLayout::new(h.agent_dir("quiet"));
    std::fs::write(layout.recent_messages_json(), "{ not json").unwrap();
    let fallback = overview_of(&h, "quiet").await["last_message"].clone();

    assert_eq!(fallback["preview"], "from the episode", "{fallback}");
    assert_eq!(fallback["at_precision"], "day");
}

#[tokio::test]
async fn a_running_agents_last_message_starts_from_its_files_and_follows_its_turns() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    write_recent(
        &h,
        "scout",
        vec![(Message::user("from disk"), Visibility::User, minute(8, 0))],
    );
    let first = overview_of(&h, "scout").await["last_message"].clone();
    assert_eq!(
        first["preview"], "from disk",
        "read from disk at the start: {first}"
    );
    let mut frames = h.overview.subscribe();

    let at = turn_ended(
        &h,
        "scout",
        Visibility::User,
        Some("what's the weather"),
        Some("Sunny, **22°C**\n\nTake a hat."),
    );
    let reply = next_overview(&mut frames).await;
    let last = reply.last_message.expect("the turn's reply is shown");
    assert_eq!(last.role, LastMessageRole::Assistant);
    assert_eq!(last.preview, "Sunny, 22°C Take a hat.");
    assert_eq!(
        last.at,
        crate::time::format_rfc3339(&at.with_timezone(&chrono_tz::UTC))
    );
    assert_eq!(last.at_precision, TimePrecision::Minute);

    turn_ended(
        &h,
        "scout",
        Visibility::Background,
        None,
        Some("pulse finished"),
    );
    expect_no_frame(&mut frames).await;

    turn_ended(&h, "scout", Visibility::User, Some("thanks"), None);
    let said = next_overview(&mut frames).await.last_message.unwrap();
    assert_eq!(
        (said.role, said.preview.as_str()),
        (LastMessageRole::User, "thanks")
    );

    let after = overview_of(&h, "scout").await["last_message"].clone();
    assert_eq!(
        after["preview"], "thanks",
        "a request doesn't undo what the hook said"
    );
}

#[tokio::test]
async fn a_turn_with_nothing_to_show_changes_nothing() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    overview_of(&h, "scout").await;
    let mut frames = h.overview.subscribe();

    turn_ended(&h, "scout", Visibility::User, Some("   "), Some("---"));
    turn_ended(&h, "scout", Visibility::User, None, None);

    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn live_sessions_appear_as_they_start_and_clear_as_they_end_or_when_the_agent_stops() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert_eq!(overview_of(&h, "scout").await["live_sessions"], json!([]));
    let mut frames = h.overview.subscribe();

    let notes = session_info("notes", SessionState::Forking);
    h.directory
        .sessions
        .lock()
        .unwrap()
        .push(("scout".to_string(), notes.clone()));
    changed(
        &h,
        "scout",
        AgentChangeKind::SessionStarted(Box::new(notes)),
    );
    let started = next_overview(&mut frames).await;
    assert_eq!(
        serde_json::to_value(&started.live_sessions).unwrap(),
        json!([{
            "address": "spawned-notes",
            "run_id": "run-notes",
            "category": "artifact",
            "source_label": "artifact:notes",
            "purpose": "draft the notes page",
            "state": "forking",
            "started_at": "2026-09-30T09:30:00Z",
        }])
    );

    h.directory.sessions.lock().unwrap()[0].1.state = SessionState::Idle;
    changed(
        &h,
        "scout",
        AgentChangeKind::SessionStateChanged {
            address: SessionAddress::from("spawned-notes"),
            run_id: "run-notes".to_string(),
            state: SessionState::Idle,
        },
    );
    let idle = next_overview(&mut frames).await;
    assert_eq!(idle.live_sessions.len(), 1);
    assert_eq!(idle.live_sessions[0].state, SessionState::Idle);

    h.directory.sessions.lock().unwrap().clear();
    changed(
        &h,
        "scout",
        AgentChangeKind::SessionCompleted {
            address: SessionAddress::from("spawned-notes"),
            run_id: "run-notes".to_string(),
            status: AgentResultStatus::Completed,
            episode_id: None,
        },
    );
    assert!(next_overview(&mut frames).await.live_sessions.is_empty());

    // The agent stops with a session the registry still lists: a stopped
    // agent has none.
    h.directory.sessions.lock().unwrap().push((
        "scout".to_string(),
        session_info("wiki", SessionState::Running),
    ));
    changed(&h, "scout", AgentChangeKind::Resync);
    assert_eq!(next_overview(&mut frames).await.live_sessions.len(), 1);
    move_to(&h, "scout", AgentState::Stopped);
    assert!(next_overview(&mut frames).await.live_sessions.is_empty());
    assert_eq!(overview_of(&h, "scout").await["live_sessions"], json!([]));
}

#[tokio::test]
async fn the_unread_count_follows_the_inbox_tool_and_files_placed_by_hand() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert_eq!(overview_of(&h, "scout").await["inbox_unread"], 0);
    let mut frames = h.overview.subscribe();

    place_item(&h, "scout", "20260930_tool").await;
    changed(
        &h,
        "scout",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_tool".to_string(),
        },
    );
    assert_eq!(next_overview(&mut frames).await.inbox_unread, 1);

    place_item(&h, "scout", "20260930_hand").await;
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox),
    );
    assert_eq!(next_overview(&mut frames).await.inbox_unread, 2);

    // Other watched files don't touch the count.
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::Config),
    );
    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn the_hubs_inbox_actions_update_the_count_of_a_running_and_of_a_stopped_agent() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    place_item(&h, "scout", "20260930_a").await;
    place_item(&h, "scout", "20260930_b").await;
    place_item(&h, "quiet", "20260930_c").await;
    let listed = h.get_expect("/api/hub/overview", StatusCode::OK).await;
    assert_eq!(listed["agents"][0]["inbox_unread"], 1, "quiet: {listed}");
    assert_eq!(listed["agents"][1]["inbox_unread"], 2, "scout: {listed}");
    let mut frames = h.overview.subscribe();

    h.expect(
        Method::PUT,
        "/api/hub/inbox/scout/20260930_a/read",
        None,
        StatusCode::OK,
    )
    .await;
    let read = next_overview(&mut frames).await;
    assert_eq!((read.name.as_str(), read.inbox_unread), ("scout", 1));

    h.post_expect("/api/hub/inbox/scout/20260930_b/archive", StatusCode::OK)
        .await;
    assert_eq!(next_overview(&mut frames).await.inbox_unread, 0);

    h.post_expect("/api/hub/inbox/scout/20260930_b/restore", StatusCode::OK)
        .await;
    assert_eq!(next_overview(&mut frames).await.inbox_unread, 1);

    h.expect(
        Method::PUT,
        "/api/hub/inbox/quiet/20260930_c/read",
        None,
        StatusCode::OK,
    )
    .await;
    let stopped = next_overview(&mut frames).await;
    assert_eq!(
        (stopped.name.as_str(), stopped.inbox_unread),
        ("quiet", 0),
        "no watcher reports a stopped agent's inbox, so the hub's action does"
    );

    // An action that fails changes nothing.
    h.expect(
        Method::PUT,
        "/api/hub/inbox/scout/missing/read",
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn a_request_counts_a_stopped_agents_inbox_again_and_tells_other_clients_what_changed() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert_eq!(overview_of(&h, "quiet").await["inbox_unread"], 0);
    let mut frames = h.overview.subscribe();

    place_item(&h, "quiet", "20260930_hand").await;
    assert_eq!(
        overview_of(&h, "quiet").await["inbox_unread"],
        1,
        "nothing watches a stopped agent, so asking is how a hand-placed file is found"
    );

    let told = next_overview(&mut frames).await;
    assert_eq!((told.name.as_str(), told.inbox_unread), ("quiet", 1));
    overview_of(&h, "quiet").await;
    expect_no_frame(&mut frames).await;
}

/// How long the burst waits between one change and the next. Forty of them
/// last three windows, so the overview has to gather the changes into more
/// than one frame.
const BURST_GAP: Duration = Duration::from_millis(15);

/// The overview sends a frame at most once a window and always sends the
/// last state.
///
/// The clock is paused so that the time a frame is taken is the time it was
/// sent: a paused clock moves only when every task is waiting, and the task
/// that takes the frames is ready as soon as one is sent. On the real clock a
/// test that takes the frames after the burst reads one that waited in the
/// channel next to one that was just sent, which looks like two frames less
/// than a window apart whenever the burst outlasts a window.
#[tokio::test(start_paused = true)]
async fn a_burst_of_changes_is_gathered_into_frames_a_window_apart_and_the_last_state_is_sent() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    overview_of(&h, "scout").await;
    let mut frames = h.overview.subscribe();
    let collector = crate::util::spawn_in_span(async move {
        let mut received = Vec::new();
        while let Some(frame) = frame_within(&mut frames, OVERVIEW_WINDOW * 3).await {
            received.push((Instant::now(), frame));
        }
        received
    });

    for n in 0..40 {
        let id = format!("20260930_{n:02}");
        place_item(&h, "scout", &id).await;
        changed(&h, "scout", AgentChangeKind::UserInboxAdded { item_id: id });
        crate::testing::clock::elapse(BURST_GAP).await;
    }
    let received = collector.await.unwrap();

    let counts: Vec<u32> = received
        .iter()
        .map(|(_, frame)| frame.inbox_unread)
        .collect();
    assert!(
        (2..=4).contains(&counts.len()),
        "40 changes over three windows made {counts:?}"
    );
    assert_eq!(
        counts.last(),
        Some(&40),
        "the last state arrives: {counts:?}"
    );
    for pair in received.windows(2) {
        let apart = pair[1].0.duration_since(pair[0].0);
        assert!(
            apart >= OVERVIEW_WINDOW,
            "frames came {apart:?} apart, under the {OVERVIEW_WINDOW:?} window: {counts:?}"
        );
    }
}

/// On the paused clock the second change lands halfway through the wait
/// however slow the machine is.
#[tokio::test(start_paused = true)]
async fn a_change_during_the_wait_is_in_the_frame_the_wait_ends_with() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    overview_of(&h, "scout").await;
    let mut frames = h.overview.subscribe();

    place_item(&h, "scout", "20260930_first").await;
    changed(
        &h,
        "scout",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_first".to_string(),
        },
    );
    crate::testing::clock::elapse(OVERVIEW_WINDOW / 2).await;
    place_item(&h, "scout", "20260930_second").await;
    changed(
        &h,
        "scout",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_second".to_string(),
        },
    );

    assert_eq!(
        next_overview(&mut frames).await.inbox_unread,
        2,
        "one frame holds both"
    );
    expect_no_frame(&mut frames).await;
}

/// On the paused clock a frame that waits for the window shows as the window
/// having passed, however fast the machine is.
#[tokio::test(start_paused = true)]
async fn a_new_agent_is_sent_at_once_and_a_deleted_agent_gets_no_further_frames() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    overview_of(&h, "scout").await;
    let mut frames = h.overview.subscribe();

    let nova = summary("nova", AgentState::Running);
    h.directory.agents.lock().unwrap().push(nova.clone());
    let created_at = Instant::now();
    h.directory
        .events
        .send(HubEvent::AgentCreated {
            agent: nova.clone(),
            by: Actor::User,
        })
        .unwrap();
    let first = next_overview(&mut frames).await;
    assert_eq!(first.name, "nova");
    assert!(
        created_at.elapsed() < OVERVIEW_WINDOW,
        "a created agent doesn't wait for a window: {:?}",
        created_at.elapsed()
    );

    place_item(&h, "nova", "20260930_x").await;
    changed(
        &h,
        "nova",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_x".to_string(),
        },
    );
    h.directory
        .agents
        .lock()
        .unwrap()
        .retain(|agent| agent.name != "nova");
    h.directory
        .events
        .send(HubEvent::AgentDeleted {
            name: "nova".to_string(),
            by: Actor::User,
        })
        .unwrap();

    expect_no_frame(&mut frames).await;
    let body = h.get_expect("/api/hub/overview", StatusCode::OK).await;
    assert!(
        body["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|agent| agent["name"] != "nova")
    );
}

#[tokio::test]
async fn the_hub_socket_sends_an_agents_overview_when_it_changes() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    let addr = h.serve().await;
    let mut socket = connect_hub(addr).await;
    let snapshot = next_frame(&mut socket).await;
    assert_eq!(snapshot["type"], "agents_snapshot");
    overview_of(&h, "scout").await;

    place_item(&h, "scout", "20260930_note").await;
    changed(
        &h,
        "scout",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_note".to_string(),
        },
    );

    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "agent_overview", "{frame}");
    assert_eq!(
        frame["overview"],
        json!({
            "name": "scout",
            "last_message": null,
            "live_sessions": [],
            "upcoming": [],
            "inbox_unread": 1,
            "outbound_problems": [],
        })
    );
}

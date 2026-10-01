//! The team event log against real agents: each kind of entry appears once,
//! from the thing that really causes it, and nothing else does.

use super::*;
use crate::hub::agent_watch::AgentChangeKind;
use crate::hub::team::{TeamLink, parse_team_address};
use crate::hub::team_events::{
    PageQuery, TeamEvent, TeamEventKind, TeamEventLevel, TeamEventTarget,
};

/// Every entry the recorder has written so far, oldest first.
fn logged(hub: &Fixture) -> Vec<TeamEvent> {
    let mut all = hub
        .team_events
        .page(&PageQuery {
            limit: Some(200),
            ..PageQuery::default()
        })
        .events;
    all.reverse();
    all
}

/// The entries of `kind` so far.
fn of_kind(hub: &Fixture, kind: TeamEventKind) -> Vec<TeamEvent> {
    logged(hub)
        .into_iter()
        .filter(|event| event.kind == kind)
        .collect()
}

/// Wait for an entry of `kind` about `agent` (any agent when `None`).
async fn entry(hub: &Fixture, kind: TeamEventKind, agent: Option<&str>) -> TeamEvent {
    eventually(&format!("a {kind:?} entry"), || async {
        of_kind(hub, kind)
            .into_iter()
            .find(|event| agent.is_none() || event.agent.as_deref() == agent)
    })
    .await
}

fn chat_of(agent: &str) -> TeamEventTarget {
    TeamEventTarget::AgentPlace {
        agent: agent.to_string(),
        place: crate::hub::team_events::TeamEventPlace::Chat,
    }
}

#[tokio::test]
async fn starting_and_stopping_an_agent_are_each_told_once() {
    let hub = Fixture::new(&["scout"], "").await;

    hub.host.start("scout").await.unwrap();
    let started = entry(&hub, TeamEventKind::AgentStarted, Some("scout")).await;
    assert_eq!(started.summary, "scout started");
    assert_eq!(started.level, TeamEventLevel::Info);
    assert_eq!(started.target, Some(chat_of("scout")));

    hub.host.stop("scout").await.unwrap();
    let stopped = entry(&hub, TeamEventKind::AgentStopped, Some("scout")).await;
    assert_eq!(stopped.summary, "scout stopped");
    assert_eq!(stopped.target, Some(chat_of("scout")));

    // Changing a setting publishes the agent's state again without changing
    // it; that is not a start.
    hub.host
        .patch(
            "scout",
            AgentPatch {
                autostart: Some(false),
                a2a_visibility: None,
            },
        )
        .await
        .unwrap();
    hub.host.start("scout").await.unwrap();
    entry(&hub, TeamEventKind::AgentStopped, Some("scout")).await;
    eventually("the second start", || async {
        (of_kind(&hub, TeamEventKind::AgentStarted).len() == 2).then_some(())
    })
    .await;
    assert_eq!(of_kind(&hub, TeamEventKind::AgentStopped).len(), 1);
}

#[tokio::test]
async fn an_agent_that_cannot_start_is_told_as_failed_with_the_reason() {
    let hub = Fixture::new(&["scout"], "").await;
    std::fs::write(
        hub.root.path().join("scout/config/providers.toml"),
        "not valid toml [[[",
    )
    .unwrap();

    hub.host.start("scout").await.unwrap_err();

    let failed = entry(&hub, TeamEventKind::AgentFailed, Some("scout")).await;
    let reason = hub
        .host
        .summary("scout")
        .unwrap()
        .last_error
        .unwrap()
        .reason;
    assert_eq!(failed.level, TeamEventLevel::Error);
    assert_eq!(failed.target, Some(chat_of("scout")));
    assert!(
        failed.summary.starts_with("scout couldn't start: "),
        "{}",
        failed.summary
    );
    assert!(
        failed
            .summary
            .contains(reason.split_whitespace().next().unwrap()),
        "the summary carries the reason: {} / {reason}",
        failed.summary
    );
    assert!(
        of_kind(&hub, TeamEventKind::AgentStarted).is_empty(),
        "an agent that never ran was not started"
    );
}

#[tokio::test]
async fn an_agent_that_crashes_is_told_as_stopped_unexpectedly() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    entry(&hub, TeamEventKind::AgentStarted, Some("scout")).await;

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    ws.send(WsMessage::text(
        json!({ "type": "server_command", "name": "panic_for_test" }).to_string(),
    ))
    .await
    .unwrap();

    let failed = entry(&hub, TeamEventKind::AgentFailed, Some("scout")).await;
    assert_eq!(failed.summary, "scout stopped unexpectedly");
    assert_eq!(failed.level, TeamEventLevel::Error);
}

#[tokio::test]
async fn creating_deleting_and_restoring_an_agent_are_each_told() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();

    hub.host
        .create(create_request("nova", None), Actor::User)
        .await
        .unwrap();
    let created = entry(&hub, TeamEventKind::AgentCreated, Some("nova")).await;
    assert_eq!(created.summary, "nova was created");
    assert_eq!(created.level, TeamEventLevel::Info);
    assert_eq!(created.target, Some(chat_of("nova")));

    hub.host
        .delete("nova", Actor::Agent("scout".to_string()))
        .await
        .unwrap();
    let deleted = entry(&hub, TeamEventKind::AgentDeleted, Some("nova")).await;
    assert_eq!(deleted.summary, "nova was deleted by scout");
    assert_eq!(deleted.target, None);
    assert!(
        of_kind(&hub, TeamEventKind::AgentStopped)
            .iter()
            .any(|event| event.agent.as_deref() == Some("nova")),
        "deleting a running agent stops it first"
    );

    hub.host
        .restore(
            RestoreAgentRequest {
                name: "nova".to_string(),
                checkpoint_id: None,
            },
            Actor::User,
        )
        .await
        .unwrap();
    let restored = entry(&hub, TeamEventKind::AgentRestored, Some("nova")).await;
    assert_eq!(restored.summary, "nova was restored");
    assert_eq!(restored.target, Some(chat_of("nova")));
}

#[tokio::test]
async fn a_hub_wide_notice_is_told_and_keeps_its_level_and_agent() {
    let hub = Fixture::new(&["scout"], "").await;

    hub.host.notice(
        crate::hub::types::NoticeLevel::Warn,
        "scout's Teams adapter can't use port 3978".to_string(),
        Some("scout".to_string()),
    );

    let notice = entry(&hub, TeamEventKind::HubNotice, Some("scout")).await;
    assert_eq!(notice.level, TeamEventLevel::Warn);
    assert_eq!(notice.summary, "scout's Teams adapter can't use port 3978");
    assert_eq!(notice.target, None);
}

#[tokio::test]
async fn a_reply_is_told_once_per_turn_and_a_background_turn_is_not_told() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut changes = hub.host.agent_changes().subscribe();

    hub.chat("scout", "first question").await;
    let replied = entry(&hub, TeamEventKind::AgentReplied, Some("scout")).await;
    assert_eq!(replied.summary, "scout replied in your conversation");
    assert_eq!(replied.level, TeamEventLevel::Info);
    assert_eq!(replied.target, Some(chat_of("scout")));

    // A teammate's message starts a background turn that still replies.
    let link = TeamLink::new("guest", Arc::clone(&hub.services.team_router));
    let target = parse_team_address("agent:scout").unwrap().unwrap();
    let main =
        crate::bus::SessionAddress::from(crate::background::registry::MAIN_ADDRESS.to_string());
    link.send(&main, &target, "status check".to_string(), 0)
        .await
        .unwrap();
    loop {
        let change = tokio::time::timeout(POLL_TIMEOUT, changes.recv())
            .await
            .expect("the background turn ends")
            .unwrap();
        if matches!(change.kind, AgentChangeKind::TurnEnded(ref turn) if turn.visibility == crate::memory::types::Visibility::Background)
        {
            break;
        }
    }

    // The feed is ordered, so once the second user turn is told, the
    // background turn before it has been read too.
    hub.chat("scout", "second question").await;
    eventually("the second reply", || async {
        (of_kind(&hub, TeamEventKind::AgentReplied).len() >= 2).then_some(())
    })
    .await;
    assert_eq!(
        of_kind(&hub, TeamEventKind::AgentReplied).len(),
        2,
        "one entry per user turn, none for the background turn"
    );
}

#[tokio::test]
async fn an_item_the_inbox_tool_saved_is_told_and_a_hand_placed_file_is_not() {
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

    hub.add_inbox_item("scout", "20260930_hand-placed");
    hub.chat("scout", "please file a note").await;

    let added = entry(&hub, TeamEventKind::InboxItemAdded, Some("scout")).await;
    assert_eq!(added.summary, "scout added an item to your inbox");
    assert_eq!(added.level, TeamEventLevel::Info);
    let Some(TeamEventTarget::InboxItem { agent, item_id }) = &added.target else {
        panic!("the entry points at the item: {added:?}");
    };
    assert_eq!(agent, "scout");
    assert!(
        hub.root
            .path()
            .join("scout/inbox/user")
            .join(format!("{item_id}.json"))
            .is_file(),
        "the entry names the saved item {item_id}"
    );
    // Later entries from the same agent arrive after the file changes did.
    hub.chat("scout", "anything else").await;
    eventually("the later reply", || async {
        (of_kind(&hub, TeamEventKind::AgentReplied).len() >= 2).then_some(())
    })
    .await;
    assert_eq!(
        of_kind(&hub, TeamEventKind::InboxItemAdded).len(),
        1,
        "the hand-placed file and the file change of the saved item add nothing"
    );
}

#[tokio::test]
async fn a_session_is_told_when_it_starts_and_when_stopping_the_agent_ends_it() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();

    hub.start_session("scout", "look into it").await;
    let started = entry(&hub, TeamEventKind::SessionStarted, Some("scout")).await;
    assert_eq!(started.level, TeamEventLevel::Info);
    assert!(
        started.summary.starts_with("scout started a session"),
        "{}",
        started.summary
    );
    let Some(TeamEventTarget::Session { agent, run_id }) = &started.target else {
        panic!("the entry points at the session run: {started:?}");
    };
    assert_eq!(agent, "scout");

    hub.host.stop("scout").await.unwrap();
    let finished = entry(&hub, TeamEventKind::SessionFinished, Some("scout")).await;
    assert_eq!(finished.level, TeamEventLevel::Warn, "{}", finished.summary);
    assert!(
        finished.summary.starts_with("scout's session was stopped"),
        "{}",
        finished.summary
    );
    assert_eq!(
        finished.target,
        Some(TeamEventTarget::Session {
            agent: "scout".to_string(),
            run_id: run_id.clone(),
        }),
        "the end points at the same run as the start"
    );
    assert!(
        of_kind(&hub, TeamEventKind::ScheduledRunFinished).is_empty(),
        "an artifact's session is not a scheduled run"
    );
}

/// Schedule an action named "nightly digest" that is already due, and let
/// scheduled sessions complete as soon as their turn ends.
fn schedule_a_due_action(hub: &Fixture) {
    let due = chrono::Utc::now() - chrono::Duration::seconds(5);
    std::fs::write(
        hub.root.path().join("scout/scheduled_actions.json"),
        json!([{
            "id": "action-00000001",
            "name": "nightly digest",
            "prompt": "summarize the day",
            "run_at": due,
            "created_at": due,
        }])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        hub.root.path().join("scout/config/config.toml"),
        "[background]\nidle_timeout_scheduled_minutes = 0\n",
    )
    .unwrap();
}

#[tokio::test]
async fn a_scheduled_run_is_told_only_when_it_finishes() {
    let hub = Fixture::new(&["scout"], "").await;
    schedule_a_due_action(&hub);

    hub.host.start("scout").await.unwrap();

    let finished = entry(&hub, TeamEventKind::ScheduledRunFinished, Some("scout")).await;
    assert_eq!(finished.level, TeamEventLevel::Info, "{}", finished.summary);
    assert_eq!(
        finished.summary,
        "scout finished the scheduled action \"nightly digest\""
    );
    assert!(
        matches!(finished.target, Some(TeamEventTarget::Session { .. })),
        "{finished:?}"
    );
    assert!(
        of_kind(&hub, TeamEventKind::SessionStarted).is_empty()
            && of_kind(&hub, TeamEventKind::SessionFinished).is_empty(),
        "a scheduled run is never a session: {:?}",
        logged(&hub)
    );
    assert_eq!(of_kind(&hub, TeamEventKind::ScheduledRunFinished).len(), 1);
}

#[tokio::test]
async fn a_scheduled_run_that_fails_is_told_as_an_error() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.mock("scout").reset().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "the model refused the request" }
        })))
        .mount(hub.mock("scout"))
        .await;
    schedule_a_due_action(&hub);

    hub.host.start("scout").await.unwrap();

    let finished = entry(&hub, TeamEventKind::ScheduledRunFinished, Some("scout")).await;
    assert_eq!(
        finished.level,
        TeamEventLevel::Error,
        "{}",
        finished.summary
    );
    assert!(
        finished
            .summary
            .starts_with("scout's scheduled action \"nightly digest\" failed: "),
        "{}",
        finished.summary
    );
    assert!(
        of_kind(&hub, TeamEventKind::SessionFinished).is_empty(),
        "{:?}",
        logged(&hub)
    );
}

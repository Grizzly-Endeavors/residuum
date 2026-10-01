//! The per-agent watcher and the turn hook against real agents: what a
//! running agent does reaches the hub's feed of agent changes, and stops
//! reaching it when the agent stops.

use super::*;
use crate::background::registry::SessionState;
use crate::bus::SessionEventKind;
use crate::hub::agent_watch::{
    AgentChange, AgentChangeKind, AgentChangeReceiver, MainTurnEnded, WatchedPath,
};
use crate::hub::team::{TeamLink, parse_team_address};
use crate::memory::types::Visibility;

/// Every change that arrives until the feed has been quiet for a moment.
async fn settled(changes: &mut AgentChangeReceiver) -> Vec<AgentChange> {
    let mut seen = Vec::new();
    while let Ok(Some(change)) =
        tokio::time::timeout(Duration::from_millis(400), changes.recv()).await
    {
        seen.push(change);
    }
    seen
}

/// Collect changes until one satisfies `found`, returning everything seen up
/// to and including it.
async fn until(
    what: &str,
    changes: &mut AgentChangeReceiver,
    found: impl Fn(&AgentChange) -> bool,
) -> Vec<AgentChange> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let change = tokio::time::timeout(left, changes.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}; saw {seen:?}"))
            .expect("the feed stays open");
        let done = found(&change);
        seen.push(change);
        if done {
            return seen;
        }
    }
}

fn turns(changes: &[AgentChange]) -> Vec<&MainTurnEnded> {
    changes
        .iter()
        .filter_map(|change| match &change.kind {
            AgentChangeKind::TurnEnded(turn) => Some(turn),
            AgentChangeKind::Resync
            | AgentChangeKind::SessionStarted(_)
            | AgentChangeKind::SessionStateChanged { .. }
            | AgentChangeKind::SessionCompleted { .. }
            | AgentChangeKind::OutboundTaskChanged(_)
            | AgentChangeKind::UserInboxAdded { .. }
            | AgentChangeKind::WatchedPathChanged(_) => None,
        })
        .collect()
}

/// The turn a batch of changes reports, which must be the only one.
fn only_turn(changes: &[AgentChange]) -> MainTurnEnded {
    let mut turns = turns(changes);
    assert_eq!(turns.len(), 1, "one turn is reported: {changes:?}");
    turns.pop().unwrap().clone()
}

fn is_turn(change: &AgentChange) -> bool {
    matches!(change.kind, AgentChangeKind::TurnEnded(_))
}

fn is_resync(change: &AgentChange) -> bool {
    matches!(change.kind, AgentChangeKind::Resync)
}

fn agent_dir(hub: &Fixture, name: &str) -> std::path::PathBuf {
    hub.root.path().join(name)
}

/// The bus of the running agent `name`.
pub(super) fn agent_bus(hub: &Fixture, name: &str) -> crate::bus::BusHandle {
    hub.host
        .slot(name)
        .unwrap()
        .lock()
        .running
        .as_ref()
        .expect("the agent is running")
        .control
        .bus
        .clone()
}

#[tokio::test]
async fn a_started_agent_is_resynced_and_each_main_turn_is_reported_once() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();

    hub.host.start("scout").await.unwrap();
    let started = until("the start to be announced", &mut changes, is_resync).await;
    assert_eq!(started.last().unwrap().agent, "scout");

    // A user turn, with the WebSocket open throughout.
    assert_eq!(hub.chat("scout", "hello there").await, "scout here");
    let seen = until("the user turn", &mut changes, is_turn).await;
    let user_turn = only_turn(&seen);
    assert_eq!(user_turn.user_message.as_deref(), Some("hello there"));
    assert_eq!(user_turn.reply.as_deref(), Some("scout here"));
    assert_eq!(user_turn.visibility, Visibility::User);
    assert!(user_turn.client_connected);

    // A teammate's message starts a background turn, with no user message
    // and, with no one connected, no client.
    let link = TeamLink::new("guest", Arc::clone(&hub.services.team_router));
    let target = parse_team_address("agent:scout").unwrap().unwrap();
    let main =
        crate::bus::SessionAddress::from(crate::background::registry::MAIN_ADDRESS.to_string());
    link.send(&main, &target, "status check".to_string(), 0)
        .await
        .unwrap();
    let background = until("the background turn", &mut changes, is_turn).await;
    let background_turn = only_turn(&background);
    assert_eq!(background_turn.user_message, None);
    assert_eq!(background_turn.reply.as_deref(), Some("scout here"));
    assert_eq!(background_turn.visibility, Visibility::Background);
    assert!(!background_turn.client_connected);

    let rest = settled(&mut changes).await;
    assert!(
        turns(&rest).is_empty(),
        "each turn is reported once: {rest:?}"
    );
}

#[tokio::test]
async fn the_user_inbox_tool_reaches_the_feed_as_user_inbox_added() {
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
    let mut changes = hub.host.agent_changes().subscribe();
    hub.host.start("scout").await.unwrap();

    hub.chat("scout", "please file a note").await;
    let seen = until("the inbox item", &mut changes, |change| {
        matches!(change.kind, AgentChangeKind::UserInboxAdded { .. })
    })
    .await;
    let AgentChangeKind::UserInboxAdded { item_id } = &seen.last().unwrap().kind else {
        unreachable!("the last change is the one waited for");
    };
    assert!(
        agent_dir(&hub, "scout")
            .join("inbox/user")
            .join(format!("{item_id}.json"))
            .is_file(),
        "the announced item {item_id} is on disk"
    );
    // The file itself is reported as a change to the inbox's files too.
    until("the inbox file change", &mut changes, |change| {
        matches!(
            change.kind,
            AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox)
        )
    })
    .await;
}

#[tokio::test]
async fn changes_to_the_files_the_hub_follows_reach_the_feed() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();
    hub.host.start("scout").await.unwrap();
    settled(&mut changes).await;

    let dir = agent_dir(&hub, "scout");
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes/todo.md"), "buy milk").unwrap();
    std::fs::write(dir.join("inbox/user/20260930_hand-placed.json"), "{}").unwrap();
    std::fs::write(dir.join("scheduled_actions.json"), "[]").unwrap();
    std::fs::write(dir.join("HEARTBEAT.yml"), "pulses: {}").unwrap();
    std::fs::write(dir.join("pulse_state.json"), "{}").unwrap();
    std::fs::write(dir.join("config/mcp.json"), "{}").unwrap();

    let mut reported = std::collections::BTreeSet::new();
    let mut seen = Vec::new();
    while reported.len() < 5 {
        let batch = until("every watched file", &mut changes, |change| {
            matches!(change.kind, AgentChangeKind::WatchedPathChanged(_))
        })
        .await;
        for change in &batch {
            if let AgentChangeKind::WatchedPathChanged(path) = change.kind {
                reported.insert(path);
            }
        }
        seen.extend(batch);
    }
    assert_eq!(
        reported.into_iter().collect::<Vec<_>>(),
        [
            WatchedPath::UserInbox,
            WatchedPath::ScheduledActions,
            WatchedPath::Heartbeat,
            WatchedPath::PulseState,
            WatchedPath::Config,
        ]
    );
    assert!(seen.iter().all(|change| change.agent == "scout"));
}

#[tokio::test]
async fn outbound_task_changes_on_the_agents_bus_reach_the_feed() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();
    hub.host.start("scout").await.unwrap();

    let now = chrono::Utc::now();
    let task = crate::a2a::TrackedTask {
        sender_address: "main".to_string(),
        agent: "laptop".to_string(),
        task_id: "t1".to_string(),
        context_id: "c1".to_string(),
        state: "working".to_string(),
        last_status_text: None,
        hop_count: 0,
        created_at: now,
        updated_at: now,
        first_unreachable_at: Some(now - chrono::Duration::minutes(11)),
        unreachable_notified: true,
        notified_this_turn: false,
        stopped_by_user: false,
    };
    agent_bus(&hub, "scout")
        .publisher()
        .publish(
            crate::bus::topics::Notification(crate::bus::NotifyName::from(
                crate::bus::SYSTEM_CHANNEL,
            )),
            crate::bus::OutboundA2aTaskEvent { task },
        )
        .await
        .unwrap();
    let seen = until("the outbound task", &mut changes, |change| {
        matches!(change.kind, AgentChangeKind::OutboundTaskChanged(_))
    })
    .await;
    assert!(matches!(
        &seen.last().unwrap().kind,
        AgentChangeKind::OutboundTaskChanged(outbound) if outbound.task_id == "t1" && outbound.unreachable_notified
    ));
}

#[tokio::test]
async fn sessions_reach_the_feed_and_the_relay_until_the_agent_stops() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();
    let mut relay = hub.host.agent_changes().subscribe_sessions();
    hub.host.start("scout").await.unwrap();

    let address = hub.start_session("scout", "look into it").await;
    let seen = until("the session to start", &mut changes, |change| {
        matches!(change.kind, AgentChangeKind::SessionStarted(_))
    })
    .await;
    let AgentChangeKind::SessionStarted(info) = &seen.last().unwrap().kind else {
        unreachable!("the last change is the one waited for");
    };
    assert_eq!(info.address.as_ref(), address);
    assert_eq!(info.source_label, "artifact:test-artifact");
    until("the session to run", &mut changes, |change| {
        matches!(
            change.kind,
            AgentChangeKind::SessionStateChanged {
                state: SessionState::Running,
                ..
            }
        )
    })
    .await;

    // The relay carries the session's turn events too, each with the label
    // the session started with.
    let mut relayed_kinds = Vec::new();
    while !relayed_kinds
        .iter()
        .any(|kind| matches!(kind, SessionEventKind::Response { .. }))
    {
        let event = tokio::time::timeout(POLL_TIMEOUT, relay.recv())
            .await
            .expect("the session answers")
            .unwrap();
        assert_eq!(event.agent, "scout");
        assert_eq!(
            event.source_label.as_deref(),
            Some("artifact:test-artifact")
        );
        relayed_kinds.push(event.event.kind);
    }

    // Stopping the agent records the live session as interrupted, and the
    // completion still reaches the feed before the stop returns.
    let bus = agent_bus(&hub, "scout");
    hub.host.stop("scout").await.unwrap();
    let mut after_stop = Vec::new();
    while let Ok(Some(change)) =
        tokio::time::timeout(Duration::from_millis(50), changes.recv()).await
    {
        after_stop.push(change);
    }
    assert!(
        after_stop.iter().any(|change| matches!(
            &change.kind,
            AgentChangeKind::SessionCompleted { address: done, .. } if done.as_ref() == address
        )),
        "the interrupted session's completion arrives: {after_stop:?}"
    );

    // Nothing arrives once the agent has stopped, even when something is
    // still published on its bus.
    bus.publisher()
        .publish(
            crate::bus::topics::UserInbox,
            crate::bus::UserInboxAddedEvent {
                item_id: "after-stop".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(settled(&mut changes).await.is_empty());
}

#[tokio::test]
async fn a_restarted_agent_is_watched_once_and_resynced_again() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();
    hub.host.start("scout").await.unwrap();
    until("the first start", &mut changes, is_resync).await;

    hub.host.restart("scout").await.unwrap();
    until("the second start", &mut changes, is_resync).await;
    settled(&mut changes).await;

    std::fs::write(
        agent_dir(&hub, "scout").join("scheduled_actions.json"),
        "[]",
    )
    .unwrap();
    let seen = until("the file change", &mut changes, |change| {
        matches!(change.kind, AgentChangeKind::WatchedPathChanged(_))
    })
    .await;
    let rest = settled(&mut changes).await;
    assert_eq!(
        seen.iter()
            .chain(&rest)
            .filter(|change| matches!(
                change.kind,
                AgentChangeKind::WatchedPathChanged(WatchedPath::ScheduledActions)
            ))
            .count(),
        1,
        "one watcher reports the change, not one per run: {seen:?} {rest:?}"
    );
}

#[tokio::test]
async fn a_turn_that_fails_is_still_reported_once_without_a_reply() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.mock("scout").reset().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "the model refused the request" }
        })))
        .mount(hub.mock("scout"))
        .await;
    let mut changes = hub.host.agent_changes().subscribe();
    hub.host.start("scout").await.unwrap();

    let mut ws = hub.start_a_turn("scout").await;
    let seen = until("the failed turn", &mut changes, is_turn).await;
    let failed = only_turn(&seen);
    assert_eq!(failed.user_message.as_deref(), Some("ping"));
    assert_eq!(failed.reply, None);
    assert_eq!(failed.visibility, Visibility::User);
    assert!(failed.client_connected);
    let rest = settled(&mut changes).await;
    assert!(turns(&rest).is_empty(), "reported once: {rest:?}");
    ws.close(None).await.ok();
}

#[tokio::test]
async fn an_agent_that_never_runs_is_never_reported() {
    let hub = Fixture::new(&["quiet"], "").await;
    let mut changes = hub.host.agent_changes().subscribe();
    std::fs::write(
        agent_dir(&hub, "quiet").join("scheduled_actions.json"),
        "[]",
    )
    .unwrap();
    assert!(settled(&mut changes).await.is_empty());
}

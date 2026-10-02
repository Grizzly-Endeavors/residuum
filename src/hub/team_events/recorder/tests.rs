//! The recorder's translation from hub events and agent changes into
//! entries, driven directly with the events the hub and the agents publish.

use chrono::Duration;

use super::*;
use crate::background::registry::SessionState;
use crate::bus::SessionAddress;
use crate::hub::agent_watch::{MainTurnEnded, WatchedPath};
use crate::hub::team_events::{PageQuery, TeamEvent};
use crate::hub::types::{A2aVisibility, AgentLastError};

fn recorder() -> Recorder {
    Recorder::new(TeamEventLog::new("boot"))
}

/// Every entry in the recorder's log, oldest first.
fn entries(recorder: &Recorder) -> Vec<TeamEvent> {
    let mut all = recorder
        .log
        .page(&PageQuery {
            limit: Some(200),
            ..PageQuery::default()
        })
        .events;
    all.reverse();
    all
}

/// The one entry the log holds.
fn only(recorder: &Recorder) -> TeamEvent {
    let mut all = entries(recorder);
    assert_eq!(all.len(), 1, "exactly one entry: {all:?}");
    all.remove(0)
}

fn summary(name: &str, state: AgentState) -> AgentSummary {
    AgentSummary {
        name: name.to_string(),
        display_name: name.to_string(),
        state,
        last_error: None,
        autostart: true,
        role: None,
        a2a_visibility: A2aVisibility::Private,
    }
}

fn failed(name: &str, kind: AgentErrorKind, reason: &str) -> AgentSummary {
    AgentSummary {
        last_error: Some(AgentLastError {
            message: format!("{name} couldn't start: {reason}. Fix its settings."),
            kind,
            reason: reason.to_string(),
            at: Utc::now(),
        }),
        ..summary(name, AgentState::Failed)
    }
}

fn state_event(agent: AgentSummary) -> HubEvent {
    HubEvent::AgentState { agent }
}

fn session_info(
    agent_run: (&str, &str),
    category: SessionCategory,
    trigger: EventTrigger,
    label: &str,
    purpose: &str,
) -> Box<SessionInfo> {
    let (address, run_id) = agent_run;
    Box::new(SessionInfo {
        address: SessionAddress::from(address),
        run_id: run_id.to_string(),
        category,
        trigger,
        source_label: label.to_string(),
        state: SessionState::Forking,
        spawner: None,
        depth: 1,
        purpose: purpose.to_string(),
        agent_skill: None,
        model_tier: crate::config::BackgroundModelTier::default(),
        conversation_target: None,
        started_at: Utc::now(),
        usage: crate::agent::usage::SessionUsageTotals::default(),
        overlap: None,
    })
}

fn start(recorder: &mut Recorder, agent: &str, info: Box<SessionInfo>) {
    recorder.on_agent_change(AgentChange {
        agent: agent.to_string(),
        kind: AgentChangeKind::SessionStarted(info),
    });
}

fn finish(
    recorder: &mut Recorder,
    agent: &str,
    address_and_run: (&str, &str),
    status: AgentResultStatus,
) {
    let (address, run_id) = address_and_run;
    recorder.on_agent_change(AgentChange {
        agent: agent.to_string(),
        kind: AgentChangeKind::SessionCompleted {
            address: SessionAddress::from(address),
            run_id: run_id.to_string(),
            status,
            episode_id: None,
        },
    });
}

fn turn(reply: Option<&str>, visibility: Visibility) -> AgentChangeKind {
    AgentChangeKind::TurnEnded(MainTurnEnded {
        user_message: Some("hello".to_string()),
        reply: reply.map(str::to_string),
        at: Utc::now() - Duration::minutes(2),
        visibility,
        client_connected: true,
    })
}

fn change(agent: &str, kind: AgentChangeKind) -> AgentChange {
    AgentChange {
        agent: agent.to_string(),
        kind,
    }
}

#[test]
fn an_agent_that_starts_is_told_once_and_only_when_its_state_changes() {
    let mut recorder = recorder();
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Starting)));
    assert!(
        entries(&recorder).is_empty(),
        "starting is not worth telling"
    );
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Running)));
    // A change of autostart or visibility republishes the same state.
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Running)));

    let event = only(&recorder);
    assert_eq!(event.kind, TeamEventKind::AgentStarted);
    assert_eq!(event.level, TeamEventLevel::Info);
    assert_eq!(event.agent.as_deref(), Some("atlas"));
    assert_eq!(event.summary, "atlas started");
    assert_eq!(event.target, Some(chat_place("atlas")));
}

#[test]
fn an_agent_that_stops_is_told_and_a_stopped_agent_changing_settings_is_not() {
    let mut recorder = recorder();
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Stopped)));
    assert!(
        entries(&recorder).is_empty(),
        "an agent nobody started was already stopped"
    );
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Running)));
    recorder.on_hub_event(HubEvent::AgentStopping {
        name: "atlas".to_string(),
    });
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Stopped)));

    let all = entries(&recorder);
    let last = all.last().unwrap();
    assert_eq!(last.kind, TeamEventKind::AgentStopped);
    assert_eq!(last.level, TeamEventLevel::Info);
    assert_eq!(last.summary, "atlas stopped");
    assert_eq!(last.target, Some(chat_place("atlas")));
    assert_eq!(all.len(), 2, "started, then stopped");
}

#[test]
fn a_start_that_fails_says_why_in_the_agents_words_and_points_at_its_chat() {
    let mut recorder = recorder();
    recorder.on_hub_event(state_event(summary("brittle", AgentState::Starting)));
    recorder.on_hub_event(state_event(failed(
        "brittle",
        AgentErrorKind::Config,
        "config error: providers.toml:\n  model 'gpt-9' is not offered by provider 'openai'.",
    )));

    let event = only(&recorder);
    assert_eq!(event.kind, TeamEventKind::AgentFailed);
    assert_eq!(event.level, TeamEventLevel::Error);
    assert_eq!(
        event.summary,
        "brittle couldn't start: config error: providers.toml: model 'gpt-9' is not offered by provider 'openai'"
    );
    assert_eq!(event.target, Some(chat_place("brittle")));
}

#[test]
fn every_failure_kind_has_its_own_plain_wording() {
    let cases = [
        (
            AgentErrorKind::PortConflict,
            AgentState::Starting,
            "atlas couldn't start: Teams port 3978 is already used by the agent 'scout'",
        ),
        (
            AgentErrorKind::Other,
            AgentState::Starting,
            "atlas couldn't start: Teams port 3978 is already used by the agent 'scout'",
        ),
        (
            AgentErrorKind::Crash,
            AgentState::Starting,
            "atlas couldn't start because of an internal error",
        ),
        (
            AgentErrorKind::Crash,
            AgentState::Running,
            "atlas stopped unexpectedly",
        ),
    ];
    for (kind, before, expected) in cases {
        let mut recorder = recorder();
        recorder.on_hub_event(state_event(summary("atlas", before)));
        recorder.on_hub_event(state_event(failed(
            "atlas",
            kind,
            "Teams port 3978 is already used by the agent 'scout'",
        )));
        let failure = entries(&recorder)
            .into_iter()
            .find(|event| event.kind == TeamEventKind::AgentFailed)
            .expect("the failure is told");
        assert_eq!(failure.summary, expected, "{kind:?} from {before}");
    }
}

#[test]
fn created_deleted_and_restored_agents_name_who_did_it_only_when_it_was_an_agent() {
    let mut recorder = recorder();
    recorder.on_hub_event(HubEvent::AgentCreated {
        agent: summary("nova", AgentState::Running),
        by: Actor::User,
    });
    recorder.on_hub_event(HubEvent::AgentCreated {
        agent: summary("echo", AgentState::Running),
        by: Actor::Agent("scout".to_string()),
    });
    recorder.on_hub_event(HubEvent::AgentDeleted {
        name: "nova".to_string(),
        by: Actor::User,
    });
    recorder.on_hub_event(HubEvent::AgentRestored {
        agent: summary("nova", AgentState::Running),
        by: Actor::Agent("scout".to_string()),
    });

    let told: Vec<_> = entries(&recorder)
        .into_iter()
        .map(|event| (event.kind, event.level, event.summary, event.target))
        .collect();
    assert_eq!(
        told,
        [
            (
                TeamEventKind::AgentCreated,
                TeamEventLevel::Info,
                "nova was created".to_string(),
                Some(chat_place("nova"))
            ),
            (
                TeamEventKind::AgentCreated,
                TeamEventLevel::Info,
                "echo was created by scout".to_string(),
                Some(chat_place("echo"))
            ),
            (
                TeamEventKind::AgentDeleted,
                TeamEventLevel::Info,
                "nova was deleted".to_string(),
                None
            ),
            (
                TeamEventKind::AgentRestored,
                TeamEventLevel::Info,
                "nova was restored by scout".to_string(),
                Some(chat_place("nova"))
            ),
        ]
    );
}

#[test]
fn a_hub_notice_is_told_at_its_own_level_and_a_reload_frame_is_not() {
    let mut recorder = recorder();
    recorder.on_hub_event(HubEvent::Notice {
        level: NoticeLevel::Warn,
        message: "gateway rebind failed:\nthe port is taken".to_string(),
        agent: None,
    });
    recorder.on_hub_event(HubEvent::Notice {
        level: NoticeLevel::Error,
        message: "scout's Teams adapter can't use port 3978".to_string(),
        agent: Some("scout".to_string()),
    });
    recorder.on_hub_event(HubEvent::Notice {
        level: NoticeLevel::Info,
        message: "hub configuration reloaded: timezone".to_string(),
        agent: None,
    });
    recorder.on_hub_event(HubEvent::HubConfigReloaded {
        ok: true,
        changed: true,
        message: Some("hub configuration reloaded: timezone".to_string()),
    });

    let told: Vec<_> = entries(&recorder)
        .into_iter()
        .map(|event| {
            (
                event.kind,
                event.level,
                event.agent,
                event.summary,
                event.target,
            )
        })
        .collect();
    assert_eq!(
        told,
        [
            (
                TeamEventKind::HubNotice,
                TeamEventLevel::Warn,
                None,
                "gateway rebind failed: the port is taken".to_string(),
                None
            ),
            (
                TeamEventKind::HubNotice,
                TeamEventLevel::Error,
                Some("scout".to_string()),
                "scout's Teams adapter can't use port 3978".to_string(),
                None
            ),
            (
                TeamEventKind::HubNotice,
                TeamEventLevel::Info,
                None,
                "hub configuration reloaded: timezone".to_string(),
                None
            ),
        ],
        "the reload frame repeats its notice and adds no entry of its own"
    );
}

#[test]
fn activity_and_stopping_frames_add_nothing() {
    let mut recorder = recorder();
    recorder.on_hub_event(HubEvent::AgentActivity {
        name: "atlas".to_string(),
        activity: crate::hub::types::AgentActivity::default(),
    });
    recorder.on_hub_event(HubEvent::AgentStopping {
        name: "atlas".to_string(),
    });
    assert!(entries(&recorder).is_empty());
}

#[test]
fn a_turn_with_a_reply_for_the_user_is_told_once_at_the_time_it_ended() {
    let mut recorder = recorder();
    let ended_at = Utc::now() - Duration::minutes(2);
    recorder.on_agent_change(change(
        "atlas",
        AgentChangeKind::TurnEnded(MainTurnEnded {
            user_message: Some("hi".to_string()),
            reply: Some("hello!".to_string()),
            at: ended_at,
            visibility: Visibility::User,
            client_connected: false,
        }),
    ));

    let event = only(&recorder);
    assert_eq!(event.kind, TeamEventKind::AgentReplied);
    assert_eq!(event.level, TeamEventLevel::Info);
    assert_eq!(event.summary, "atlas replied in your conversation");
    assert_eq!(event.at, ended_at);
    assert_eq!(event.target, Some(chat_place("atlas")));
}

#[test]
fn a_turn_without_a_reply_or_without_the_user_is_not_told() {
    let mut recorder = recorder();
    recorder.on_agent_change(change("atlas", turn(None, Visibility::User)));
    recorder.on_agent_change(change("atlas", turn(Some("noted"), Visibility::Background)));
    recorder.on_agent_change(change("atlas", turn(None, Visibility::Background)));
    assert!(entries(&recorder).is_empty());
}

#[test]
fn an_inbox_item_the_tool_saved_points_at_the_item() {
    let mut recorder = recorder();
    recorder.on_agent_change(change(
        "atlas",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_pelican".to_string(),
        },
    ));

    let event = only(&recorder);
    assert_eq!(event.kind, TeamEventKind::InboxItemAdded);
    assert_eq!(event.level, TeamEventLevel::Info);
    assert_eq!(event.summary, "atlas added an item to your inbox");
    assert_eq!(
        event.target,
        Some(TeamEventTarget::InboxItem {
            agent: "atlas".to_string(),
            item_id: "20260930_pelican".to_string(),
        })
    );
}

#[test]
fn a_change_to_the_inbox_files_is_not_an_added_item() {
    let mut recorder = recorder();
    recorder.on_agent_change(change(
        "atlas",
        AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox),
    ));
    recorder.on_agent_change(change("atlas", AgentChangeKind::Resync));
    assert!(entries(&recorder).is_empty());
}

#[test]
fn a_session_is_told_when_it_starts_and_how_it_ended() {
    let mut recorder = recorder();
    start(
        &mut recorder,
        "atlas",
        session_info(
            ("spawned-x", "run-1"),
            SessionCategory::Spawned,
            EventTrigger::Agent,
            "agent:researcher",
            "compare fallback\nstrategies",
        ),
    );
    finish(
        &mut recorder,
        "atlas",
        ("spawned-x", "run-1"),
        AgentResultStatus::Completed,
    );

    let all = entries(&recorder);
    let told: Vec<_> = all
        .iter()
        .map(|event| (event.kind, event.level, event.summary.as_str()))
        .collect();
    assert_eq!(
        told,
        [
            (
                TeamEventKind::SessionStarted,
                TeamEventLevel::Info,
                "atlas started a session: compare fallback strategies"
            ),
            (
                TeamEventKind::SessionFinished,
                TeamEventLevel::Info,
                "atlas finished a session: compare fallback strategies"
            ),
        ]
    );
    let target = Some(TeamEventTarget::Session {
        agent: "atlas".to_string(),
        run_id: "run-1".to_string(),
    });
    assert!(all.iter().all(|event| event.target == target));
}

#[test]
fn a_session_that_was_stopped_is_a_warning_and_one_that_failed_is_an_error() {
    let mut recorder = recorder();
    for (run, status) in [
        ("run-1", AgentResultStatus::Cancelled),
        (
            "run-2",
            AgentResultStatus::Failed {
                error: "The model rejected the request.".to_string(),
                details: None,
            },
        ),
    ] {
        start(
            &mut recorder,
            "atlas",
            session_info(
                ("artifact-notes", run),
                SessionCategory::Artifact,
                EventTrigger::Artifact("notes".to_string()),
                "artifact:notes",
                "tidy the notes",
            ),
        );
        finish(&mut recorder, "atlas", ("artifact-notes", run), status);
    }

    let finished: Vec<_> = entries(&recorder)
        .into_iter()
        .filter(|event| event.kind == TeamEventKind::SessionFinished)
        .map(|event| (event.level, event.summary))
        .collect();
    assert_eq!(
        finished,
        [
            (
                TeamEventLevel::Warn,
                "atlas's session was stopped: tidy the notes".to_string()
            ),
            (
                TeamEventLevel::Error,
                "atlas's session failed: The model rejected the request.".to_string()
            ),
        ]
    );
}

#[test]
fn a_scheduled_run_is_told_once_when_it_finishes_and_never_as_a_session() {
    let mut recorder = recorder();
    start(
        &mut recorder,
        "atlas",
        session_info(
            ("scheduled-1", "run-7"),
            SessionCategory::Scheduled,
            EventTrigger::Pulse,
            "pulse:email_check",
            "check the inbox",
        ),
    );
    assert!(
        entries(&recorder).is_empty(),
        "a scheduled run's start is not told"
    );
    finish(
        &mut recorder,
        "atlas",
        ("scheduled-1", "run-7"),
        AgentResultStatus::Completed,
    );

    let event = only(&recorder);
    assert_eq!(event.kind, TeamEventKind::ScheduledRunFinished);
    assert_eq!(event.level, TeamEventLevel::Info);
    assert_eq!(event.summary, "atlas finished the pulse \"email_check\"");
    assert_eq!(
        event.target,
        Some(TeamEventTarget::Session {
            agent: "atlas".to_string(),
            run_id: "run-7".to_string(),
        })
    );
}

#[test]
fn a_scheduled_run_that_failed_is_an_error_and_one_that_was_stopped_is_not() {
    let mut recorder = recorder();
    for (run, trigger, label, status) in [
        (
            "run-1",
            EventTrigger::Action,
            "action:nightly digest",
            AgentResultStatus::Failed {
                error: "The model is unavailable.".to_string(),
                details: None,
            },
        ),
        (
            "run-2",
            EventTrigger::Pulse,
            "pulse:email_check",
            AgentResultStatus::Cancelled,
        ),
    ] {
        start(
            &mut recorder,
            "atlas",
            session_info(
                ("scheduled", run),
                SessionCategory::Scheduled,
                trigger,
                label,
                "",
            ),
        );
        finish(&mut recorder, "atlas", ("scheduled", run), status);
    }

    let told: Vec<_> = entries(&recorder)
        .into_iter()
        .map(|event| (event.kind, event.level, event.summary))
        .collect();
    assert_eq!(
        told,
        [
            (
                TeamEventKind::ScheduledRunFinished,
                TeamEventLevel::Error,
                "atlas's scheduled action \"nightly digest\" failed: The model is unavailable."
                    .to_string()
            ),
            (
                TeamEventKind::ScheduledRunFinished,
                TeamEventLevel::Info,
                "atlas's pulse \"email_check\" was stopped".to_string()
            ),
        ]
    );
}

#[test]
fn runs_are_told_apart_by_agent_and_run_so_two_agents_can_share_an_address() {
    let mut recorder = recorder();
    start(
        &mut recorder,
        "atlas",
        session_info(
            ("spawned-x", "run-1"),
            SessionCategory::Scheduled,
            EventTrigger::Pulse,
            "pulse:a",
            "",
        ),
    );
    start(
        &mut recorder,
        "scout",
        session_info(
            ("spawned-x", "run-1"),
            SessionCategory::Spawned,
            EventTrigger::Agent,
            "agent:b",
            "",
        ),
    );
    finish(
        &mut recorder,
        "scout",
        ("spawned-x", "run-1"),
        AgentResultStatus::Completed,
    );
    finish(
        &mut recorder,
        "atlas",
        ("spawned-x", "run-1"),
        AgentResultStatus::Completed,
    );

    let kinds: Vec<_> = entries(&recorder)
        .into_iter()
        .map(|event| (event.agent.unwrap_or_default(), event.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("scout".to_string(), TeamEventKind::SessionStarted),
            ("scout".to_string(), TeamEventKind::SessionFinished),
            ("atlas".to_string(), TeamEventKind::ScheduledRunFinished),
        ]
    );
}

#[test]
fn a_session_that_ends_without_a_start_on_record_is_left_out() {
    let mut recorder = recorder();
    finish(
        &mut recorder,
        "atlas",
        ("spawned-x", "run-1"),
        AgentResultStatus::Completed,
    );
    assert!(entries(&recorder).is_empty());
}

#[test]
fn state_changes_of_a_session_and_outbound_task_changes_add_nothing() {
    let mut recorder = recorder();
    recorder.on_agent_change(change(
        "atlas",
        AgentChangeKind::SessionStateChanged {
            address: SessionAddress::from("spawned-x"),
            run_id: "run-1".to_string(),
            state: SessionState::Running,
        },
    ));
    assert!(entries(&recorder).is_empty());
}

#[test]
fn deleting_an_agent_forgets_its_runs_and_state() {
    let mut recorder = recorder();
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Running)));
    start(
        &mut recorder,
        "atlas",
        session_info(
            ("spawned-x", "run-1"),
            SessionCategory::Spawned,
            EventTrigger::Agent,
            "agent:x",
            "",
        ),
    );
    recorder.on_hub_event(HubEvent::AgentDeleted {
        name: "atlas".to_string(),
        by: Actor::User,
    });
    assert!(recorder.runs.is_empty());
    assert!(recorder.states.is_empty());

    // A new agent of the same name starts from nothing: it was not running.
    recorder.on_hub_event(state_event(summary("atlas", AgentState::Running)));
    assert_eq!(
        entries(&recorder).last().map(|event| event.kind),
        Some(TeamEventKind::AgentStarted)
    );
}

#[test]
fn the_start_of_the_hub_is_the_first_entry_and_points_nowhere() {
    let log = TeamEventLog::new("boot");
    record_hub_started(&log);
    let page = log.page(&PageQuery::default());
    let event = page.events.first().unwrap();
    assert_eq!(event.kind, TeamEventKind::HubStarted);
    assert_eq!(event.level, TeamEventLevel::Info);
    assert_eq!(event.summary, "Residuum started");
    assert_eq!((event.agent.as_deref(), &event.target), (None, &None));
}

#[tokio::test]
async fn the_running_recorder_reads_both_sources_until_it_is_dropped() {
    let log = TeamEventLog::new("boot");
    let (events, _keep) = tokio::sync::broadcast::channel(16);
    let feed = crate::hub::agent_watch::AgentChangeFeed::new();
    let recorder = TeamEventRecorder::spawn(Arc::clone(&log), events.subscribe(), feed.subscribe());
    let mut entries_seen = log.subscribe();

    events
        .send(HubEvent::Notice {
            level: NoticeLevel::Info,
            message: "hello".to_string(),
            agent: None,
        })
        .unwrap();
    feed.publish(&change(
        "atlas",
        AgentChangeKind::UserInboxAdded {
            item_id: "item".to_string(),
        },
    ));

    let mut kinds = Vec::new();
    for _ in 0..2 {
        let entry = tokio::time::timeout(std::time::Duration::from_secs(5), entries_seen.recv())
            .await
            .expect("an entry arrives")
            .unwrap();
        kinds.push(entry.kind);
    }
    kinds.sort_by_key(|kind| format!("{kind:?}"));
    assert_eq!(
        kinds,
        [TeamEventKind::HubNotice, TeamEventKind::InboxItemAdded]
    );

    drop(recorder);
    tokio::task::yield_now().await;
    events
        .send(HubEvent::Notice {
            level: NoticeLevel::Info,
            message: "after".to_string(),
            agent: None,
        })
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        log.page(&PageQuery::default()).events.len(),
        2,
        "nothing is recorded once the recorder is dropped"
    );
}

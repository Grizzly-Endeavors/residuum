//! The rules for when each push fires, and the words each one carries,
//! driven directly with what the hub and the agents publish.

use chrono::{Duration, TimeZone as _};

use super::*;
use crate::hub::agent_watch::{MainTurnEnded, WatchedPath};
use crate::hub::types::{A2aVisibility, Actor, AgentLastError};

fn summary(name: &str, state: AgentState) -> AgentSummary {
    AgentSummary {
        name: name.to_string(),
        state,
        last_error: None,
        autostart: true,
        role: None,
        a2a_visibility: A2aVisibility::Private,
    }
}

fn failed(name: &str, kind: AgentErrorKind) -> AgentSummary {
    AgentSummary {
        last_error: Some(AgentLastError {
            message: format!("{name} couldn't start. Fix its settings."),
            kind,
            reason: "bad model".to_string(),
            at: Utc::now(),
        }),
        ..summary(name, AgentState::Failed)
    }
}

fn state_event(agent: AgentSummary) -> HubEvent {
    HubEvent::AgentState { agent }
}

fn change(agent: &str, kind: AgentChangeKind) -> AgentChange {
    AgentChange {
        agent: agent.to_string(),
        kind,
    }
}

fn turn(visibility: Visibility, reply: Option<&str>, client_connected: bool) -> AgentChangeKind {
    AgentChangeKind::TurnEnded(MainTurnEnded {
        user_message: None,
        reply: reply.map(str::to_string),
        at: Utc::now(),
        visibility,
        client_connected,
    })
}

/// A task `scout` sent to `laptop`.
fn task(unreachable_notified: bool, state: &str) -> TrackedTask {
    let now = Utc::now();
    TrackedTask {
        sender_address: "main".to_string(),
        agent: "laptop".to_string(),
        task_id: "t1".to_string(),
        context_id: "c1".to_string(),
        state: state.to_string(),
        last_status_text: None,
        hop_count: 0,
        created_at: now,
        updated_at: now,
        first_unreachable_at: Some(now - Duration::minutes(11)),
        unreachable_notified,
        notified_this_turn: false,
        stopped_by_user: false,
    }
}

fn outbound(task: TrackedTask) -> AgentChangeKind {
    AgentChangeKind::OutboundTaskChanged(Box::new(task))
}

// ─── inbox_item ───────────────────────────────────────────────────────

#[test]
fn an_item_the_tool_saved_is_one_push_and_the_file_change_beside_it_is_none() {
    let mut rules = Rules::default();
    let added = rules.on_change(change(
        "scout",
        AgentChangeKind::UserInboxAdded {
            item_id: "20260930_note".to_string(),
        },
    ));
    assert_eq!(
        added,
        Some(Trigger::InboxItem {
            agent: "scout".to_string(),
            item_id: "20260930_note".to_string(),
        })
    );

    let file_change = rules.on_change(change(
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox),
    ));
    assert_eq!(file_change, None, "one save is one addition");
}

// ─── agent_failed ─────────────────────────────────────────────────────

#[test]
fn entering_the_failed_state_fires_once_with_what_failed() {
    let mut rules = Rules::default();
    assert_eq!(
        rules.on_hub_event(&state_event(summary("scout", AgentState::Starting))),
        None
    );

    let fired = rules.on_hub_event(&state_event(failed("scout", AgentErrorKind::Config)));
    assert_eq!(
        fired,
        Some(Trigger::AgentFailed {
            agent: "scout".to_string(),
            kind: AgentErrorKind::Config,
            was_running: false,
        })
    );

    // A change of settings while it stays failed is not another failure.
    assert_eq!(
        rules.on_hub_event(&state_event(failed("scout", AgentErrorKind::Config))),
        None
    );
}

#[test]
fn an_agent_that_failed_while_running_says_so() {
    let mut rules = Rules::default();
    rules.on_hub_event(&state_event(summary("scout", AgentState::Running)));

    let fired = rules.on_hub_event(&state_event(failed("scout", AgentErrorKind::Crash)));

    assert_eq!(
        fired,
        Some(Trigger::AgentFailed {
            agent: "scout".to_string(),
            kind: AgentErrorKind::Crash,
            was_running: true,
        })
    );
}

#[test]
fn a_failed_agent_started_again_that_fails_again_fires_again() {
    let mut rules = Rules::default();
    rules.on_hub_event(&state_event(failed("scout", AgentErrorKind::Config)));
    rules.on_hub_event(&state_event(summary("scout", AgentState::Starting)));

    let again = rules.on_hub_event(&state_event(failed("scout", AgentErrorKind::Config)));

    assert!(matches!(again, Some(Trigger::AgentFailed { .. })));
}

#[test]
fn a_created_agent_whose_start_failed_is_one_push_in_either_order() {
    let created = |agent: AgentSummary| HubEvent::AgentCreated {
        agent,
        by: Actor::User,
    };

    // The state change comes first, as the host publishes it.
    let mut state_first = Rules::default();
    let from_state = state_first.on_hub_event(&state_event(failed("nova", AgentErrorKind::Config)));
    let then_creation = state_first.on_hub_event(&created(failed("nova", AgentErrorKind::Config)));
    assert!(from_state.is_some());
    assert_eq!(then_creation, None);

    // And if the creation were seen first, the state change adds nothing.
    let mut creation_first = Rules::default();
    let from_creation =
        creation_first.on_hub_event(&created(failed("nova", AgentErrorKind::Config)));
    let then_state =
        creation_first.on_hub_event(&state_event(failed("nova", AgentErrorKind::Config)));
    assert!(from_creation.is_some());
    assert_eq!(then_state, None);
}

#[test]
fn a_failed_agent_without_an_error_record_is_an_other_failure() {
    let mut rules = Rules::default();

    let fired = rules.on_hub_event(&state_event(summary("scout", AgentState::Failed)));

    assert_eq!(
        fired,
        Some(Trigger::AgentFailed {
            agent: "scout".to_string(),
            kind: AgentErrorKind::Other,
            was_running: false,
        })
    );
}

#[test]
fn states_that_are_not_failures_fire_nothing() {
    let mut rules = Rules::default();
    for state in [
        AgentState::Starting,
        AgentState::Running,
        AgentState::Stopped,
    ] {
        assert_eq!(
            rules.on_hub_event(&state_event(summary("scout", state))),
            None,
            "{state}"
        );
    }
    for event in [
        HubEvent::AgentStopping {
            name: "scout".to_string(),
        },
        HubEvent::AgentDeleted {
            name: "scout".to_string(),
            by: Actor::User,
        },
    ] {
        assert_eq!(rules.on_hub_event(&event), None);
    }
}

#[test]
fn a_failure_whose_event_was_lost_is_found_by_reading_the_agents_again() {
    let mut rules = Rules::default();
    rules.on_hub_event(&state_event(summary("scout", AgentState::Running)));

    let found = rules.reconcile(&[
        failed("scout", AgentErrorKind::Crash),
        summary("atlas", AgentState::Running),
    ]);
    assert_eq!(
        found,
        vec![Trigger::AgentFailed {
            agent: "scout".to_string(),
            kind: AgentErrorKind::Crash,
            was_running: true,
        }]
    );

    let again = rules.reconcile(&[
        failed("scout", AgentErrorKind::Crash),
        summary("atlas", AgentState::Running),
    ]);
    assert!(again.is_empty(), "a failure already pushed is not repeated");
}

// ─── outbound_unreachable ─────────────────────────────────────────────

#[test]
fn an_unreachable_streak_is_one_push_when_it_passes_the_threshold() {
    let mut rules = Rules::default();

    // The streak starts: the tracker announces it, with no notice yet.
    assert_eq!(
        rules.on_change(change("scout", outbound(task(false, "working")))),
        None
    );

    // It passes the threshold: this is the push.
    let crossed = task(true, "working");
    let since = crossed.first_unreachable_at.unwrap();
    assert_eq!(
        rules.on_change(change("scout", outbound(crossed.clone()))),
        Some(Trigger::OutboundUnreachable {
            agent: "scout".to_string(),
            remote: "laptop".to_string(),
            task_id: "t1".to_string(),
            since,
        })
    );

    // Hearing of the same streak again sends nothing.
    assert_eq!(
        rules.on_change(change("scout", outbound(crossed))),
        None,
        "once per streak"
    );
}

#[test]
fn a_new_streak_after_recovery_is_pushed_again() {
    let mut rules = Rules::default();
    assert!(
        rules
            .on_change(change("scout", outbound(task(true, "working"))))
            .is_some()
    );

    // The remote answered: the streak ended.
    assert_eq!(
        rules.on_change(change("scout", outbound(task(false, "working")))),
        None
    );

    assert!(
        rules
            .on_change(change("scout", outbound(task(true, "working"))))
            .is_some(),
        "the next streak is its own"
    );
}

#[test]
fn a_closed_task_is_not_pushed_and_two_agents_tasks_are_counted_apart() {
    let mut rules = Rules::default();
    assert_eq!(
        rules.on_change(change("scout", outbound(task(true, "canceled")))),
        None,
        "nobody is waiting on a closed task"
    );

    // The same task id on two senders is two tasks.
    assert!(
        rules
            .on_change(change("scout", outbound(task(true, "working"))))
            .is_some()
    );
    assert!(
        rules
            .on_change(change("atlas", outbound(task(true, "working"))))
            .is_some()
    );
}

#[test]
fn a_deleted_agents_pushed_streaks_are_forgotten() {
    let mut rules = Rules::default();
    rules.on_change(change("scout", outbound(task(true, "working"))));

    rules.on_hub_event(&HubEvent::AgentDeleted {
        name: "scout".to_string(),
        by: Actor::User,
    });

    assert!(
        rules
            .on_change(change("scout", outbound(task(true, "working"))))
            .is_some(),
        "a restored agent's streak is new"
    );
}

// ─── reply_while_away ─────────────────────────────────────────────────

#[test]
fn a_reply_to_the_user_with_no_client_connected_is_a_push() {
    let mut rules = Rules::default();

    let fired = rules.on_change(change(
        "scout",
        turn(Visibility::User, Some("All done."), false),
    ));

    assert_eq!(
        fired,
        Some(Trigger::ReplyWhileAway {
            agent: "scout".to_string(),
            reply: "All done.".to_string(),
        })
    );
}

#[test]
fn a_reply_is_not_a_push_while_a_client_is_connected() {
    let mut rules = Rules::default();

    let fired = rules.on_change(change(
        "scout",
        turn(Visibility::User, Some("All done."), true),
    ));

    assert_eq!(fired, None, "the connected client shows it");
}

#[test]
fn a_background_turn_and_a_turn_without_a_reply_are_not_pushes() {
    let mut rules = Rules::default();

    assert_eq!(
        rules.on_change(change(
            "scout",
            turn(Visibility::Background, Some("pulse chatter"), false)
        )),
        None,
        "a background reply is not for the user"
    );
    assert_eq!(
        rules.on_change(change("scout", turn(Visibility::User, None, false))),
        None
    );
}

#[test]
fn the_rest_of_the_change_feed_fires_nothing() {
    let mut rules = Rules::default();
    assert_eq!(
        rules.on_change(change("scout", AgentChangeKind::Resync)),
        None
    );
    for path in [
        WatchedPath::ScheduledActions,
        WatchedPath::Heartbeat,
        WatchedPath::PulseState,
        WatchedPath::Config,
    ] {
        assert_eq!(
            rules.on_change(change("scout", AgentChangeKind::WatchedPathChanged(path))),
            None
        );
    }
}

// ─── The words ────────────────────────────────────────────────────────

#[test]
fn an_inbox_item_push_carries_the_items_title_and_the_start_of_its_body() {
    let payload = inbox_item_payload(
        "scout",
        "20260930_weekly_report",
        Some((
            "Weekly report".to_string(),
            "**Done:** the migration.\n\nNext: [docs](https://example.com)".to_string(),
        )),
        4,
    );

    assert_eq!(payload.event, PushEvent::InboxItem);
    assert_eq!(payload.agent, "scout");
    assert_eq!(payload.title, "Weekly report");
    assert_eq!(payload.body, "From scout: Done: the migration. Next: docs");
    assert_eq!(payload.tag, "inbox:scout:20260930_weekly_report");
    assert_eq!(payload.target, "/inbox?item=scout:20260930_weekly_report");
    assert_eq!(payload.badge, 4);
}

#[test]
fn a_long_inbox_body_is_cut_to_the_payload_limit() {
    let payload = inbox_item_payload(
        "scout",
        "20260930_long",
        Some(("Long".to_string(), "word ".repeat(100))),
        1,
    );

    assert!(payload.body.starts_with("From scout: word word"));
    assert!(payload.body.chars().count() <= crate::hub::push::MAX_BODY_CHARS);
    assert!(payload.body.ends_with('…'));
}

#[test]
fn an_inbox_item_that_could_not_be_read_still_says_where_it_is_from() {
    let payload = inbox_item_payload("scout", "20260930_note", None, 2);

    assert_eq!(payload.title, UNREADABLE_ITEM_TITLE);
    assert_eq!(payload.body, "From scout.");
    assert_eq!(payload.tag, "inbox:scout:20260930_note");
    assert_eq!(payload.badge, 2);
}

#[test]
fn an_item_with_an_empty_title_or_body_falls_back_per_part() {
    let no_title = inbox_item_payload(
        "scout",
        "20260930_x",
        Some((String::new(), "details".to_string())),
        0,
    );
    assert_eq!(no_title.title, UNREADABLE_ITEM_TITLE);
    assert_eq!(no_title.body, "From scout: details");

    let no_body = inbox_item_payload(
        "scout",
        "20260930_x",
        Some(("Heads up".to_string(), "  \n ".to_string())),
        0,
    );
    assert_eq!(no_body.title, "Heads up");
    assert_eq!(no_body.body, "From scout.");
}

#[test]
fn an_item_id_with_unusual_characters_stays_one_query_value() {
    let payload = inbox_item_payload("scout", "20260930_café & more=1", None, 0);

    assert_eq!(
        payload.target,
        "/inbox?item=scout:20260930_caf%C3%A9+%26+more%3D1"
    );
    assert_eq!(
        payload.tag, "inbox:scout:20260930_café & more=1",
        "the tag is not a URL"
    );
}

#[test]
fn an_agent_failure_push_names_the_agent_and_a_line_by_the_kind_of_error() {
    for (kind, line) in [
        (
            AgentErrorKind::Config,
            "Its settings need fixing before it can run.",
        ),
        (
            AgentErrorKind::PortConflict,
            "Another agent is using its Teams port.",
        ),
        (
            AgentErrorKind::Crash,
            "It hit an internal error. Open Residuum to restart it.",
        ),
        (
            AgentErrorKind::Other,
            "Open Residuum to see what went wrong.",
        ),
    ] {
        let payload = agent_failed_payload("scout", kind, false, 3);
        assert_eq!(payload.event, PushEvent::AgentFailed);
        assert_eq!(payload.title, "scout couldn't start");
        assert_eq!(payload.body, line, "{kind:?}");
        assert_eq!(payload.tag, "failed:scout");
        assert_eq!(payload.target, "/agent/scout");
        assert_eq!(payload.badge, 3);
    }
}

#[test]
fn an_agent_that_stopped_by_itself_does_not_claim_it_couldnt_start() {
    let payload = agent_failed_payload("scout", AgentErrorKind::Crash, true, 0);

    assert_eq!(payload.title, "scout stopped unexpectedly");
    assert_eq!(payload.tag, "failed:scout");
}

#[test]
fn an_unreachable_remote_push_names_the_agent_the_remote_and_the_time() {
    let payload = outbound_unreachable_payload("scout", "laptop", "t1", "14:05", 0);

    assert_eq!(payload.event, PushEvent::OutboundUnreachable);
    assert_eq!(payload.title, "scout can't reach laptop");
    assert_eq!(payload.body, "A task has been waiting since 14:05.");
    assert_eq!(payload.tag, "outbound:scout:t1");
    assert_eq!(payload.target, "/agent/scout/activity");
}

#[test]
fn a_time_is_the_clock_today_and_carries_the_date_on_another_day() {
    let tz = chrono_tz::America::New_York;
    let at = |day, hour, minute| tz.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap();

    assert_eq!(clock_text(at(30, 14, 5), at(30, 23, 0)), "14:05");
    assert_eq!(clock_text(at(29, 23, 55), at(30, 0, 5)), "Sep 29, 23:55");
}

#[test]
fn a_reply_push_is_the_reply_as_a_plain_preview() {
    let payload = reply_while_away_payload("scout", "Here's the **summary**:\n\n- one\n- two", 0);

    assert_eq!(payload.event, PushEvent::ReplyWhileAway);
    assert_eq!(payload.title, "scout replied");
    assert_eq!(payload.body, "Here's the summary: one two");
    assert_eq!(
        payload.tag, "reply:scout",
        "a later reply replaces this one"
    );
    assert_eq!(payload.target, "/agent/scout");
}

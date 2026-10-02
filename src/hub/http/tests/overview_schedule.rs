//! `upcoming` and `outbound_problems` of the overview, against the fake
//! directory's agents: `scout` (running) and `quiet` (stopped). Both are read
//! from files placed where an agent's own would be, and a running agent's
//! follow the changes its watcher reports.
//!
//! Times in the files are in 2099, so that what a test expects does not
//! depend on the day it runs.

use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::Request;
use chrono::{DateTime, Duration as TimeDelta, TimeZone as _};

use super::overview::{
    changed, expect_no_frame, frame_within, move_to, next_overview, overview_of,
};
use super::*;
use crate::a2a::TrackedTask;
use crate::a2a::client::tracker::UNREACHABLE_NOTICE_AFTER;
use crate::actions::store::ActionStore;
use crate::actions::types::ScheduledAction;
use crate::background::registry::SessionRegistry;
use crate::background::store::SessionStore;
use crate::gateway::web::scheduled::{ScheduledApiState, scheduled_api_router};
use crate::hub::agent_watch::{AgentChangeKind, WatchedPath};
use crate::hub::overview::AgentOverview;
use crate::hub::test_support::EventLog;

fn at_2099(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2099, month, day, hour, minute, 0)
        .unwrap()
}

/// `HEARTBEAT.yml` of `agent`, with a pulse for each of `(name, schedule,
/// active hours)`.
fn write_pulses(h: &Harness, agent: &str, pulses: &[(&str, &str, Option<&str>)]) {
    let mut yaml = String::from("pulses:\n");
    for (name, schedule, active_hours) in pulses {
        write!(
            yaml,
            "  - name: {name}\n    schedule: \"{schedule}\"\n    tasks: []\n"
        )
        .unwrap();
        if let Some(hours) = active_hours {
            writeln!(yaml, "    active_hours: \"{hours}\"").unwrap();
        }
    }
    std::fs::create_dir_all(h.agent_dir(agent)).unwrap();
    std::fs::write(h.agent_dir(agent).join("HEARTBEAT.yml"), yaml).unwrap();
}

/// `pulse_state.json` of `agent`: when each `(pulse, local time)` last ran.
fn write_pulse_state(h: &Harness, agent: &str, last_runs: &[(&str, &str)]) {
    let last_run: serde_json::Map<String, Value> = last_runs
        .iter()
        .map(|(name, at)| ((*name).to_string(), json!(at)))
        .collect();
    std::fs::write(
        h.agent_dir(agent).join("pulse_state.json"),
        json!({ "last_run": last_run }).to_string(),
    )
    .unwrap();
}

/// The `config.toml` of `agent`.
fn write_config(h: &Harness, agent: &str, toml: &str) {
    let dir = h.agent_dir(agent).join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
}

/// `scheduled_actions.json` of `agent`, holding an action for each
/// `(name, run_at)`.
fn write_actions(h: &Harness, agent: &str, actions: &[(&str, DateTime<Utc>)]) {
    let actions: Vec<ScheduledAction> = actions
        .iter()
        .enumerate()
        .map(|(n, (name, run_at))| ScheduledAction {
            id: format!("action-{n:08x}"),
            name: (*name).to_string(),
            prompt: "do it".to_string(),
            run_at: *run_at,
            agent: None,
            model_tier: None,
            created_at: at_2099(1, 1, 0, 0),
        })
        .collect();
    std::fs::create_dir_all(h.agent_dir(agent)).unwrap();
    std::fs::write(
        h.agent_dir(agent).join("scheduled_actions.json"),
        serde_json::to_string(&actions).unwrap(),
    )
    .unwrap();
}

/// What `upcoming` of `agent` says, as `kind:name@at` for each run.
async fn upcoming_of(h: &Harness, agent: &str) -> Vec<String> {
    overview_of(h, agent).await["upcoming"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            format!(
                "{}:{}@{}",
                run["kind"].as_str().unwrap(),
                run["name"].as_str().unwrap(),
                run["at"].as_str().unwrap()
            )
        })
        .collect()
}

fn outbound_task(id: &str, remote: &str, unreachable_since: Option<DateTime<Utc>>) -> TrackedTask {
    TrackedTask {
        sender_address: "main".to_string(),
        agent: remote.to_string(),
        task_id: id.to_string(),
        context_id: format!("context-{id}"),
        state: "working".to_string(),
        last_status_text: None,
        hop_count: 0,
        created_at: at_2099(1, 1, 0, 0),
        updated_at: at_2099(1, 1, 0, 0),
        first_unreachable_at: unreachable_since,
        unreachable_notified: false,
        notified_this_turn: false,
        stopped_by_user: false,
    }
}

/// `a2a/outbound.json` of `agent`, holding `tasks`.
fn write_outbound(h: &Harness, agent: &str, tasks: &[TrackedTask]) {
    let tasks: serde_json::Map<String, Value> = tasks
        .iter()
        .map(|task| (task.task_id.clone(), serde_json::to_value(task).unwrap()))
        .collect();
    let dir = h.agent_dir(agent).join("a2a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("outbound.json"),
        json!({ "tasks": tasks, "contexts": {} }).to_string(),
    )
    .unwrap();
}

/// Say that the tracker of `agent` recorded a change to `task`.
fn task_changed(h: &Harness, agent: &str, task: &TrackedTask) {
    changed(
        h,
        agent,
        AgentChangeKind::OutboundTaskChanged(Box::new(task.clone())),
    );
}

fn problem_ids(overview: &AgentOverview) -> Vec<&str> {
    overview
        .outbound_problems
        .iter()
        .map(|problem| problem.task_id.as_str())
        .collect()
}

/// The Schedule place's routes over the agent in `dir`, as its gateway builds
/// them.
fn scheduled_endpoint(dir: &std::path::Path, tz: chrono_tz::Tz) -> axum::Router {
    scheduled_api_router(ScheduledApiState {
        registry: Arc::new(SessionRegistry::new()),
        store: Arc::new(SessionStore::new(dir.join("sessions"))),
        action_store: Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
            dir.join("scheduled_actions.json"),
        ))),
        layout: WorkspaceLayout::new(dir),
        tz,
    })
}

/// `GET /api/scheduled/pulses` of `endpoint`.
async fn pulses_listed(endpoint: &axum::Router) -> Value {
    let response = tower::ServiceExt::oneshot(
        endpoint.clone(),
        Request::get("/api/scheduled/pulses")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn instant(text: &Value) -> DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(text.as_str().unwrap()).unwrap()
}

/// Check that each pulse the endpoint gives a next time is in `upcoming` at
/// that time, and that a pulse it gives none is not there. A pulse that has
/// never run is due now for both, each read at its own moment, so they may be
/// a minute tick apart.
fn assert_same_next_times(listed: &Value, upcoming: &Value) {
    for pulse in listed.as_array().unwrap() {
        let name = pulse["name"].as_str().unwrap();
        let in_overview = upcoming
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["name"] == name);
        match (pulse["next_fire_at"].is_null(), in_overview) {
            (true, None) => assert_eq!(name, "off", "only the disabled pulse has no next run"),
            (false, Some(run)) => {
                let endpoint_at = instant(&pulse["next_fire_at"]);
                let overview_at = instant(&run["at"]);
                if name == "never_run" {
                    assert!(
                        (endpoint_at - overview_at).num_seconds().abs() <= 60,
                        "{name}"
                    );
                } else {
                    assert_eq!(endpoint_at, overview_at, "{name}");
                }
            }
            (null, run) => panic!("{name}: next_fire_at null={null} but the overview has {run:?}"),
        }
    }
}

// ---- upcoming ---------------------------------------------------------------

#[tokio::test]
async fn a_run_pushed_past_the_active_hours_is_listed_for_when_they_open() {
    let h = Harness::new();
    write_pulses(
        &h,
        "quiet",
        &[
            ("digest", "1h", Some("09:00-17:00")),
            ("sweep", "2h", Some("09:00-17:00")),
        ],
    );
    write_pulse_state(
        &h,
        "quiet",
        &[
            ("digest", "2099-01-01T16:30:00"),
            ("sweep", "2099-01-01T10:00:00.250"),
        ],
    );

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        [
            "pulse:sweep@2099-01-01T12:00:00Z",
            "pulse:digest@2099-01-02T09:00:00Z"
        ],
        "17:30 is after the digest's active hours close; 12:00 is inside the sweep's"
    );
}

#[tokio::test]
async fn a_pulse_that_has_never_run_is_due_at_the_start_of_the_current_minute() {
    let h = Harness::new();
    write_pulses(&h, "quiet", &[("first_light", "6h", None)]);

    let before = Utc::now();
    let listed = overview_of(&h, "quiet").await["upcoming"].clone();
    let after = Utc::now();

    assert_eq!(listed[0]["kind"], "pulse", "{listed}");
    assert_eq!(listed[0]["name"], "first_light", "{listed}");
    let at = DateTime::parse_from_rfc3339(listed[0]["at"].as_str().unwrap()).unwrap();
    assert!(
        at <= after && at > before - TimeDelta::seconds(61),
        "due now, to the minute: {at} between {before} and {after}"
    );
    assert_eq!(chrono::Timelike::second(&at), 0, "{at}");
}

#[tokio::test]
async fn with_the_pulse_system_off_no_pulse_is_listed_and_the_actions_still_are() {
    let h = Harness::new();
    write_pulses(&h, "quiet", &[("digest", "1h", None)]);
    write_pulse_state(&h, "quiet", &[("digest", "2099-01-01T10:00:00")]);
    write_actions(&h, "quiet", &[("weekly", at_2099(1, 7, 8, 0))]);
    assert_eq!(
        upcoming_of(&h, "quiet").await,
        [
            "pulse:digest@2099-01-01T11:00:00Z",
            "action:weekly@2099-01-07T08:00:00Z"
        ],
        "on, by default"
    );

    write_config(&h, "quiet", "[pulse]\nenabled = false\n");

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        ["action:weekly@2099-01-07T08:00:00Z"]
    );
}

#[tokio::test]
async fn a_config_that_cannot_be_read_lists_no_runs_and_is_warned_about_once() {
    let h = Harness::new();
    write_pulses(&h, "quiet", &[("digest", "1h", None)]);
    write_pulse_state(&h, "quiet", &[("digest", "2099-01-01T10:00:00")]);
    write_actions(&h, "quiet", &[("weekly", at_2099(1, 7, 8, 0))]);
    write_config(&h, "quiet", "[pulse\nenabled = ");
    let log = EventLog::default();
    let _guard = log.capture();

    assert!(
        upcoming_of(&h, "quiet").await.is_empty(),
        "no pulse and no action is promised by an agent that can't be loaded"
    );
    assert!(upcoming_of(&h, "quiet").await.is_empty());
    let warned = log.matching("couldn't read the agent's config");
    let [event] = warned.as_slice() else {
        panic!("one warning for as long as it stays unreadable, got {warned:?}");
    };
    assert_eq!(event.level, tracing::Level::WARN);
    assert!(event.text.contains("agent=quiet"), "{}", event.text);
    assert!(event.text.contains("config.toml"), "{}", event.text);

    write_config(&h, "quiet", "");
    assert_eq!(
        upcoming_of(&h, "quiet").await.len(),
        2,
        "listed again once it can be read"
    );
    assert_eq!(
        log.matching("can read what it couldn't before").len(),
        1,
        "and that is noted once"
    );
}

#[tokio::test]
async fn scheduled_actions_that_cannot_be_read_cost_only_the_actions() {
    let h = Harness::new();
    write_pulses(&h, "quiet", &[("digest", "1h", None)]);
    write_pulse_state(&h, "quiet", &[("digest", "2099-01-01T10:00:00")]);
    std::fs::write(
        h.agent_dir("quiet").join("scheduled_actions.json"),
        "{ not json",
    )
    .unwrap();
    let log = EventLog::default();
    let _guard = log.capture();

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        ["pulse:digest@2099-01-01T11:00:00Z"]
    );
    let warned = log.matching("couldn't read the agent's scheduled actions");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].level, tracing::Level::WARN);
    assert!(
        h.agent_dir("quiet").join("scheduled_actions.json").exists(),
        "the broken file is left where it is for its agent to deal with"
    );
}

#[tokio::test]
async fn the_next_three_runs_are_listed_soonest_first_pulses_before_actions_at_the_same_time() {
    let h = Harness::new();
    write_pulses(
        &h,
        "quiet",
        &[("soon", "1h", None), ("tomorrow", "24h", None)],
    );
    write_pulse_state(
        &h,
        "quiet",
        &[
            ("soon", "2099-01-01T10:00:00"),
            ("tomorrow", "2099-01-01T10:00:00"),
        ],
    );
    write_actions(
        &h,
        "quiet",
        &[
            ("same_time", at_2099(1, 1, 11, 0)),
            (
                "overdue",
                Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap(),
            ),
            ("next_summer", at_2099(6, 1, 0, 0)),
        ],
    );

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        [
            "action:overdue@2000-01-01T00:00:00Z",
            "pulse:soon@2099-01-01T11:00:00Z",
            "action:same_time@2099-01-01T11:00:00Z",
        ],
        "an overdue action keeps the time it was set for, and only three are listed"
    );
}

#[tokio::test]
async fn times_are_told_in_the_hubs_timezone() {
    let h = Harness::new();
    *h.directory.timezone.lock().unwrap() = chrono_tz::America::New_York;
    write_pulses(&h, "quiet", &[("digest", "1h", Some("09:00-17:00"))]);
    write_pulse_state(&h, "quiet", &[("digest", "2099-01-01T16:30:00")]);
    write_actions(&h, "quiet", &[("weekly", at_2099(1, 7, 13, 0))]);

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        [
            "pulse:digest@2099-01-02T09:00:00-05:00",
            "action:weekly@2099-01-07T08:00:00-05:00"
        ],
        "the pulse's local times are read in New York, and the action's instant is told there"
    );
}

#[tokio::test]
async fn a_stopped_agents_upcoming_is_read_again_on_every_request() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert!(upcoming_of(&h, "quiet").await.is_empty());
    let mut frames = h.overview.subscribe();

    write_actions(&h, "quiet", &[("weekly", at_2099(1, 7, 8, 0))]);

    assert_eq!(
        upcoming_of(&h, "quiet").await,
        ["action:weekly@2099-01-07T08:00:00Z"],
        "nothing watches a stopped agent, so asking is how an edit is found"
    );
    let told = next_overview(&mut frames).await;
    assert_eq!((told.name.as_str(), told.upcoming.len()), ("quiet", 1));
    upcoming_of(&h, "quiet").await;
    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn a_running_agents_upcoming_follows_the_changes_its_watcher_reports() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert!(upcoming_of(&h, "scout").await.is_empty());
    let mut frames = h.overview.subscribe();

    write_actions(&h, "scout", &[("weekly", at_2099(1, 7, 8, 0))]);
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::ScheduledActions),
    );
    let first = next_overview(&mut frames).await;
    assert_eq!(first.upcoming.len(), 1, "{first:?}");
    assert_eq!(first.upcoming[0].name, "weekly");

    write_pulses(&h, "scout", &[("digest", "1h", None)]);
    write_pulse_state(&h, "scout", &[("digest", "2099-01-01T10:00:00")]);
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::Heartbeat),
    );
    let pulse = next_overview(&mut frames).await;
    assert_eq!(
        pulse
            .upcoming
            .iter()
            .map(|run| run.name.as_str())
            .collect::<Vec<_>>(),
        ["digest", "weekly"]
    );

    // The pulse ran: its next run moves.
    write_pulse_state(&h, "scout", &[("digest", "2099-01-01T11:00:00")]);
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::PulseState),
    );
    let ran = next_overview(&mut frames).await;
    assert_eq!(ran.upcoming[0].at, "2099-01-01T12:00:00Z");

    // The pulse system is switched off in the settings.
    write_config(&h, "scout", "[pulse]\nenabled = false\n");
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::Config),
    );
    let off = next_overview(&mut frames).await;
    assert_eq!(
        off.upcoming
            .iter()
            .map(|run| run.name.as_str())
            .collect::<Vec<_>>(),
        ["weekly"]
    );

    // A change that moves nothing sends nothing.
    changed(
        &h,
        "scout",
        AgentChangeKind::WatchedPathChanged(WatchedPath::PulseState),
    );
    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn the_pulses_endpoint_and_the_overview_give_a_pulse_the_same_next_time() {
    let h = Harness::new();
    let new_york = chrono_tz::America::New_York;
    *h.directory.timezone.lock().unwrap() = new_york;
    write_pulses(
        &h,
        "quiet",
        &[
            ("hourly", "1h", None),
            ("pushed", "1h", Some("09:00-17:00")),
            ("never_run", "6h", None),
            ("off", "1h", None),
        ],
    );
    let heartbeat = h.agent_dir("quiet").join("HEARTBEAT.yml");
    let yaml = std::fs::read_to_string(&heartbeat).unwrap();
    std::fs::write(
        &heartbeat,
        yaml.replace("  - name: off\n", "  - name: off\n    enabled: false\n"),
    )
    .unwrap();
    write_pulse_state(
        &h,
        "quiet",
        &[
            ("hourly", "2099-01-01T10:00:00"),
            ("pushed", "2099-01-01T16:30:00"),
            ("off", "2099-01-01T10:00:00"),
        ],
    );

    let endpoint = scheduled_endpoint(&h.agent_dir("quiet"), new_york);

    let listed = pulses_listed(&endpoint).await;
    let upcoming = overview_of(&h, "quiet").await["upcoming"].clone();
    assert_same_next_times(&listed, &upcoming);
    assert_eq!(listed.as_array().unwrap().len(), 4);
    assert_eq!(upcoming.as_array().unwrap().len(), 3);
    assert_eq!(
        instant(&upcoming[2]["at"]),
        DateTime::parse_from_rfc3339("2099-01-02T09:00:00-05:00").unwrap(),
        "the pushed pulse opens at 09:00 New York time"
    );

    write_config(&h, "quiet", "[pulse]\nenabled = false\n");
    let off = pulses_listed(&endpoint).await;
    assert!(
        off.as_array()
            .unwrap()
            .iter()
            .all(|pulse| pulse["next_fire_at"].is_null()),
        "no pulse runs while the pulse system is off: {off}"
    );
    assert!(
        overview_of(&h, "quiet").await["upcoming"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

// ---- outbound problems --------------------------------------------------------

#[tokio::test]
async fn an_unreachable_task_is_a_problem_once_its_streak_passes_the_threshold_and_until_it_ends() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    assert!(
        overview_of(&h, "scout").await["outbound_problems"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut frames = h.overview.subscribe();
    let since = Utc::now() - TimeDelta::minutes(11);

    // The streak has only just started: nothing to tell.
    let mut laptop = outbound_task("task-1", "laptop", Some(Utc::now()));
    write_outbound(&h, "scout", &[laptop.clone()]);
    task_changed(&h, "scout", &laptop);
    expect_no_frame(&mut frames).await;

    // The tracker announces that it passed the threshold.
    laptop.first_unreachable_at = Some(since);
    laptop.unreachable_notified = true;
    laptop.last_status_text = Some("Reading the papers".to_string());
    write_outbound(&h, "scout", &[laptop.clone()]);
    task_changed(&h, "scout", &laptop);
    let problem = next_overview(&mut frames).await;
    assert_eq!(problem_ids(&problem), ["task-1"]);
    let listed = &problem.outbound_problems[0];
    assert_eq!(listed.remote_agent, "laptop");
    assert_eq!(listed.status_text.as_deref(), Some("Reading the papers"));
    assert_eq!(listed.unreachable_since, since);

    // The agent answers: the streak ends.
    laptop.first_unreachable_at = None;
    laptop.unreachable_notified = false;
    write_outbound(&h, "scout", &[laptop.clone()]);
    task_changed(&h, "scout", &laptop);
    assert!(
        next_overview(&mut frames)
            .await
            .outbound_problems
            .is_empty()
    );

    // The streak starts again and the user stops watching the task.
    laptop.first_unreachable_at = Some(since);
    write_outbound(&h, "scout", &[laptop.clone()]);
    task_changed(&h, "scout", &laptop);
    assert_eq!(problem_ids(&next_overview(&mut frames).await), ["task-1"]);
    laptop.state = "canceled".to_string();
    laptop.stopped_by_user = true;
    laptop.first_unreachable_at = None;
    write_outbound(&h, "scout", &[laptop.clone()]);
    task_changed(&h, "scout", &laptop);
    assert!(
        next_overview(&mut frames)
            .await
            .outbound_problems
            .is_empty()
    );
}

#[tokio::test]
async fn the_longest_unreachable_task_is_listed_first_and_a_reachable_or_closed_one_is_not() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    let now = Utc::now();
    let mut done = outbound_task("done", "desktop", Some(now - TimeDelta::hours(5)));
    done.state = "completed".to_string();
    write_outbound(
        &h,
        "scout",
        &[
            outbound_task("recent", "phone", Some(now - TimeDelta::minutes(12))),
            outbound_task("long", "laptop", Some(now - TimeDelta::hours(3))),
            outbound_task("reachable", "lab", None),
            outbound_task("short", "nas", Some(now - TimeDelta::minutes(2))),
            done,
        ],
    );

    let overview = overview_of(&h, "scout").await;

    let ids: Vec<&str> = overview["outbound_problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| problem["task_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["long", "recent"]);
}

#[tokio::test]
async fn a_task_becomes_a_problem_when_its_streak_passes_the_threshold_with_no_event_to_say_so() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    // The streak passes the threshold in a second, and nothing is published
    // about it: the tracker is still waiting for its next failed poll.
    let crossing = Utc::now() + TimeDelta::seconds(1);
    let since = crossing - UNREACHABLE_NOTICE_AFTER;
    write_outbound(
        &h,
        "scout",
        &[outbound_task("task-1", "laptop", Some(since))],
    );

    let first = overview_of(&h, "scout").await;
    assert!(
        first["outbound_problems"].as_array().unwrap().is_empty(),
        "{first}"
    );
    let mut frames = h.overview.subscribe();

    let frame = next_overview(&mut frames).await;

    assert!(
        Utc::now() >= crossing,
        "the problem came at the threshold, not before"
    );
    assert_eq!(problem_ids(&frame), ["task-1"]);
    assert_eq!(frame.outbound_problems[0].unreachable_since, since);
    expect_no_frame(&mut frames).await;
}

#[tokio::test]
async fn a_task_that_ends_its_streak_before_the_threshold_never_becomes_a_problem() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    let crossing = Utc::now() + TimeDelta::seconds(1);
    let mut task = outbound_task(
        "task-1",
        "laptop",
        Some(crossing - UNREACHABLE_NOTICE_AFTER),
    );
    write_outbound(&h, "scout", &[task.clone()]);
    overview_of(&h, "scout").await;
    let mut frames = h.overview.subscribe();

    task.first_unreachable_at = None;
    write_outbound(&h, "scout", &[task.clone()]);
    task_changed(&h, "scout", &task);

    assert!(
        frame_within(&mut frames, std::time::Duration::from_secs(3))
            .await
            .is_none(),
        "no frame, at the old crossing or any other time"
    );
    assert!(
        overview_of(&h, "scout").await["outbound_problems"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_stopped_agent_has_no_outbound_problems_and_a_stopping_agent_drops_its_own() {
    let h = Harness::new();
    let _tracker = h.track_overview();
    let since = Utc::now() - TimeDelta::hours(1);
    let stuck = outbound_task("task-1", "laptop", Some(since));
    write_outbound(&h, "quiet", std::slice::from_ref(&stuck));
    write_outbound(&h, "scout", &[stuck]);

    let quiet = overview_of(&h, "quiet").await;
    assert!(
        quiet["outbound_problems"].as_array().unwrap().is_empty(),
        "nothing watches a stopped agent's tasks: {quiet}"
    );
    let scout = overview_of(&h, "scout").await;
    assert_eq!(
        scout["outbound_problems"].as_array().unwrap().len(),
        1,
        "{scout}"
    );
    let mut frames = h.overview.subscribe();

    move_to(&h, "scout", AgentState::Stopped);

    assert!(
        next_overview(&mut frames)
            .await
            .outbound_problems
            .is_empty()
    );
    assert!(
        overview_of(&h, "scout").await["outbound_problems"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn outbound_tasks_that_cannot_be_read_are_warned_about_once_and_list_no_problems() {
    let h = Harness::new();
    let dir = h.agent_dir("scout").join("a2a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("outbound.json"), "{ not json").unwrap();
    let log = EventLog::default();
    let _guard = log.capture();

    for _ in 0..2 {
        overview_of(&h, "scout").await;
        changed(&h, "scout", AgentChangeKind::Resync);
    }

    let warned = log.matching("couldn't read the agent's outbound tasks");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].level, tracing::Level::WARN);
    assert!(warned[0].text.contains("agent=scout"), "{}", warned[0].text);
}

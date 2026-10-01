//! Schedule place HTTP API: pulses and one-off scheduled actions, backing
//! the web UI's Schedule place.
//!
//! - `GET /api/agents/{name}/scheduled/pulses` — every pulse in HEARTBEAT.yml, its next
//!   fire time (the calculation the hub's team overview uses, see
//!   `crate::pulse::next_run`), last outcome, current run (with any overlap
//!   flag), and any per-pulse loading problems (see
//!   `crate::pulse::types::load_heartbeat`).
//! - `PUT /api/agents/{name}/scheduled/pulses/{pulse}/enabled` — flip a pulse's `enabled`
//!   field in HEARTBEAT.yml in place, preserving everything else in the
//!   file (see `crate::pulse::edit::set_pulse_enabled`).
//! - `GET /api/agents/{name}/scheduled/actions` — every pending scheduled action, its due
//!   time, and its current run if it has already fired.
//! - `DELETE /api/agents/{name}/scheduled/actions/{id}` — cancel a pending action.
//!
//! Pulse/action run state (current run, last outcome) changes are already
//! visible live over the WebSocket as `session_*` frames for the
//! `scheduled` category; a HEARTBEAT.yml or `scheduled_actions.json` edit
//! is visible as a `workspace_changed` frame naming that path. The web UI
//! refetches this API on either signal rather than polling it.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::actions::store::ActionStore;
use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::background::store::{RunFilter, SessionStore};
use crate::gateway::protocol::{
    ActionInfo, PulseInfo, ScheduledCurrentRun, ScheduledRunOutcome, SessionRunStatus,
};
use crate::pulse::next_run::{load_last_runs, next_run_at};
use crate::pulse::types::{HeartbeatProblem, PulseDef, load_heartbeat};
use crate::workspace::layout::WorkspaceLayout;

/// Shared state for the Schedule place API.
#[derive(Clone)]
pub(crate) struct ScheduledApiState {
    pub registry: Arc<SessionRegistry>,
    pub store: Arc<SessionStore>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
        .into_response()
}

/// Build the Schedule place API router.
pub(crate) fn scheduled_api_router(state: ScheduledApiState) -> axum::Router {
    use axum::routing::{delete, get, put};
    axum::Router::new()
        .route("/api/scheduled/pulses", get(api_scheduled_pulses))
        .route(
            "/api/scheduled/pulses/{name}/enabled",
            put(api_scheduled_pulse_set_enabled),
        )
        .route("/api/scheduled/actions", get(api_scheduled_actions))
        .route(
            "/api/scheduled/actions/{id}",
            delete(api_scheduled_action_cancel),
        )
        .with_state(state)
}

/// `GET /api/agents/{name}/scheduled/pulses`
async fn api_scheduled_pulses(State(state): State<ScheduledApiState>) -> Response {
    let heartbeat_path = state.layout.heartbeat_yml();
    let mut last_parse_error = None;
    let mut problems = Vec::new();
    let cfg = load_heartbeat(&heartbeat_path, &mut last_parse_error, &mut problems, &[]);
    let pulses = cfg.map(|c| c.pulses).unwrap_or_default();
    // Read from the files rather than through a live `PulseScheduler` (this
    // API has none), the same as the gateway's own hot-reload does for
    // HEARTBEAT.yml.
    let last_run = load_last_runs(&state.layout.pulse_state_json());
    let pulse_enabled = pulse_system_enabled(&state.layout);

    let infos = build_pulse_infos(
        &pulses,
        &problems,
        &last_run,
        (Utc::now(), pulse_enabled),
        &state,
    )
    .await;
    Json(infos).into_response()
}

/// Whether the agent's pulse system is on. A `config.toml` that can't be
/// read is logged and counts as off, so no pulse promises a run: the same
/// answer the hub's team overview gives for it.
fn pulse_system_enabled(layout: &WorkspaceLayout) -> bool {
    match crate::config::Config::pulse_enabled_at(layout.root()) {
        Ok(enabled) => enabled,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't read whether the pulse system is on; listing no next run times");
            false
        }
    }
}

/// Build one `PulseInfo` per pulse that loaded, plus a synthetic entry for
/// every problem that names a pulse which didn't (a pulse rejected for
/// using a removed option, a bad schedule, or a per-entry deserialize
/// failure never makes it into `pulses` at all — the owner still needs to
/// see it in the Schedule place, not just in a log line).
async fn build_pulse_infos(
    pulses: &[PulseDef],
    problems: &[HeartbeatProblem],
    last_run: &HashMap<String, NaiveDateTime>,
    (now, pulse_enabled): (DateTime<Utc>, bool),
    state: &ScheduledApiState,
) -> Vec<PulseInfo> {
    let mut infos = Vec::with_capacity(pulses.len());
    let mut named = std::collections::HashSet::new();

    for pulse in pulses {
        named.insert(pulse.name.clone());
        let source_label = format!("pulse:{}", pulse.name);
        let pulse_problems: Vec<String> = problems
            .iter()
            .filter(|p| p.name == pulse.name)
            .map(|p| p.message.clone())
            .collect();
        infos.push(PulseInfo {
            name: pulse.name.clone(),
            enabled: pulse.enabled,
            schedule: Some(pulse.schedule.clone()),
            active_hours: pulse.active_hours.clone(),
            agent: pulse.agent.clone(),
            next_fire_at: pulse_enabled
                .then(|| next_run_at(pulse, last_run.get(&pulse.name).copied(), now, state.tz))
                .flatten(),
            last_outcome: last_outcome(&state.store, &source_label).await,
            current_run: current_run(&state.registry, &source_label),
            problems: pulse_problems,
        });
    }

    // Problems naming a pulse that isn't in `pulses` at all — it was
    // rejected entirely (removed option, bad schedule, per-entry
    // deserialize failure) rather than just flagged alongside a value that
    // still loaded.
    let mut rejected_names: Vec<&str> = problems
        .iter()
        .map(|p| p.name.as_str())
        .filter(|name| !named.contains(*name) && *name != "HEARTBEAT.yml")
        .collect();
    rejected_names.sort_unstable();
    rejected_names.dedup();

    for name in rejected_names {
        let messages: Vec<String> = problems
            .iter()
            .filter(|p| p.name == name)
            .map(|p| p.message.clone())
            .collect();
        infos.push(PulseInfo {
            name: name.to_string(),
            enabled: false,
            schedule: None,
            active_hours: None,
            agent: None,
            next_fire_at: None,
            last_outcome: None,
            current_run: None,
            problems: messages,
        });
    }

    infos
}

/// The most recent completed run's outcome for a given source label
/// (`pulse:<name>` or `action:<name>`), if any run has ever completed.
async fn last_outcome(store: &SessionStore, source_label: &str) -> Option<ScheduledRunOutcome> {
    let page = store
        .list_completed_runs(
            RunFilter {
                source_label: Some(source_label),
                ..Default::default()
            },
            None,
            1,
        )
        .await
        .ok()?;
    let record = page.runs.into_iter().next()?;
    let status = SessionRunStatus::from_label(record.outcome.as_deref()?)?;
    Some(ScheduledRunOutcome {
        status,
        at: record.completed_at.unwrap_or(record.started_at),
        error: record.outcome_error,
    })
}

/// The currently live run for a given source label, if it's running right now.
fn current_run(registry: &SessionRegistry, source_label: &str) -> Option<ScheduledCurrentRun> {
    registry
        .list_live()
        .into_iter()
        .find(|info| {
            info.category == SessionCategory::Scheduled && info.source_label == source_label
        })
        .map(|info| ScheduledCurrentRun {
            address: info.address.to_string(),
            run_id: info.run_id,
            overlap: info.overlap,
        })
}

#[derive(Debug, Deserialize)]
struct SetEnabledRequest {
    enabled: bool,
}

/// `PUT /api/agents/{name}/scheduled/pulses/{pulse}/enabled`
async fn api_scheduled_pulse_set_enabled(
    State(state): State<ScheduledApiState>,
    Path(name): Path<String>,
    body: Result<Json<SetEnabledRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(b) => b,
        Err(rejection) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid request body: {}", rejection.body_text()),
            );
        }
    };

    let heartbeat_path = state.layout.heartbeat_yml();
    let contents = match tokio::fs::read_to_string(&heartbeat_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read HEARTBEAT.yml for the enabled toggle");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't read HEARTBEAT.yml. Check the logs for details.",
            );
        }
    };

    let Ok(updated) = crate::pulse::edit::set_pulse_enabled(&contents, &name, body.enabled) else {
        return json_error(
            StatusCode::NOT_FOUND,
            format!("No pulse named \"{name}\" was found in HEARTBEAT.yml."),
        );
    };

    if let Err(e) = crate::util::fs::atomic_write(&heartbeat_path, &updated).await {
        tracing::warn!(error = %e, pulse = %name, "failed to save HEARTBEAT.yml after toggling enabled");
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't save HEARTBEAT.yml. Check the logs for details.",
        );
    }

    tracing::info!(pulse = %name, enabled = body.enabled, "pulse enabled flag toggled from the Schedule place");
    Json(serde_json::json!({ "name": name, "enabled": body.enabled })).into_response()
}

/// `GET /api/agents/{name}/scheduled/actions`
async fn api_scheduled_actions(State(state): State<ScheduledApiState>) -> Response {
    let actions = state.action_store.lock().await.list().to_vec();
    let mut infos = Vec::with_capacity(actions.len());
    for action in actions {
        let source_label = format!("action:{}", action.name);
        infos.push(ActionInfo {
            id: action.id,
            name: action.name,
            run_at: action.run_at,
            agent: action.agent,
            model_tier: action.model_tier,
            current_run: current_run(&state.registry, &source_label),
        });
    }
    Json(infos).into_response()
}

/// `DELETE /api/agents/{name}/scheduled/actions/{id}`
async fn api_scheduled_action_cancel(
    State(state): State<ScheduledApiState>,
    Path(id): Path<String>,
) -> Response {
    let mut store = state.action_store.lock().await;
    if !store.remove(&id) {
        return json_error(
            StatusCode::NOT_FOUND,
            format!("No scheduled action with id \"{id}\" was found."),
        );
    }
    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, id = %id, "failed to save action store after cancelling an action");
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The action was cancelled but couldn't be saved. Check the logs for details.",
        );
    }
    tracing::info!(id = %id, "scheduled action cancelled from the Schedule place");
    Json(serde_json::json!({ "id": id, "cancelled": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::{
        MAIN_DEPTH, SessionInfo, SessionRegistry, SessionState, generate_address,
    };
    use crate::background::store::SessionStore;
    use crate::bus::{AgentResultStatus, EventTrigger, PulseOverlap};
    use crate::pulse::types::ProblemKind;
    use tokio_util::sync::CancellationToken;

    fn sample_pulse(name: &str) -> PulseDef {
        PulseDef {
            name: name.to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: vec![],
        }
    }

    fn scheduled_info(source_label: &str, overlap: Option<PulseOverlap>) -> SessionInfo {
        let trigger = EventTrigger::Pulse;
        let address = generate_address(&trigger, source_label);
        SessionInfo {
            address,
            run_id: "run-1".to_string(),
            category: SessionCategory::Scheduled,
            trigger,
            source_label: source_label.to_string(),
            state: SessionState::Running,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "check".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Small,
            conversation_target: None,
            started_at: Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
            overlap,
        }
    }

    fn test_state(dir: &std::path::Path) -> ScheduledApiState {
        ScheduledApiState {
            registry: Arc::new(SessionRegistry::new()),
            store: Arc::new(SessionStore::new(dir.join("sessions"))),
            action_store: Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
                dir.join("scheduled_actions.json"),
            ))),
            layout: WorkspaceLayout::new(dir),
            tz: chrono_tz::UTC,
        }
    }

    /// The pulses `sample_pulse` makes, as listed at the start of 2099.
    async fn infos_at(
        dir: &std::path::Path,
        pulses: &[PulseDef],
        last_run: &HashMap<String, NaiveDateTime>,
        pulse_enabled: bool,
    ) -> Vec<PulseInfo> {
        let state = test_state(dir);
        let now = chrono::NaiveDate::from_ymd_opt(2099, 1, 1)
            .unwrap()
            .and_hms_opt(12, 20, 40)
            .unwrap()
            .and_utc();
        build_pulse_infos(pulses, &[], last_run, (now, pulse_enabled), &state).await
    }

    fn day_2099(hour: u32, minute: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2099, 1, 1)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn the_next_fire_time_follows_the_schedule_the_active_hours_and_the_last_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut disabled = sample_pulse("disabled");
        disabled.enabled = false;
        let mut quiet = sample_pulse("quiet");
        quiet.active_hours = Some("09:00-12:00".to_string());
        let pulses = [
            sample_pulse("recent"),
            sample_pulse("never_run"),
            quiet,
            disabled,
        ];
        let last_run = HashMap::from([
            ("recent".to_string(), day_2099(12, 0)),
            ("quiet".to_string(), day_2099(11, 30)),
        ]);

        let infos = infos_at(dir.path(), &pulses, &last_run, true).await;

        let next: Vec<(&str, Option<String>)> = infos
            .iter()
            .map(|info| {
                (
                    info.name.as_str(),
                    info.next_fire_at.map(|at| at.to_rfc3339()),
                )
            })
            .collect();
        assert_eq!(
            next,
            [
                ("recent", Some("2099-01-01T13:00:00+00:00".to_string())),
                ("never_run", Some("2099-01-01T12:20:00+00:00".to_string())),
                ("quiet", Some("2099-01-02T09:00:00+00:00".to_string())),
                ("disabled", None),
            ],
            "a pulse due already is reported at the start of the minute, and 12:30 is after 12:00"
        );
    }

    #[tokio::test]
    async fn no_pulse_has_a_next_fire_time_when_the_pulse_system_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let infos = infos_at(dir.path(), &[sample_pulse("p")], &HashMap::new(), false).await;
        assert!(infos.iter().all(|info| info.next_fire_at.is_none()));
        assert!(
            infos.iter().all(|info| info.enabled),
            "its own switch is unchanged"
        );
    }

    #[test]
    fn the_pulse_system_is_off_when_its_config_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        assert!(
            pulse_system_enabled(&layout),
            "no config: the default is on"
        );

        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/config.toml"),
            "[pulse]\nenabled = false\n",
        )
        .unwrap();
        assert!(!pulse_system_enabled(&layout), "switched off");

        std::fs::write(dir.path().join("config/config.toml"), "[pulse\n").unwrap();
        assert!(!pulse_system_enabled(&layout), "unreadable");
    }

    #[tokio::test]
    async fn build_pulse_infos_includes_a_live_current_run_and_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let info = scheduled_info(
            "pulse:email_check",
            Some(PulseOverlap {
                previous_run_id: "run-old".to_string(),
                previous_started_at: Utc::now(),
            }),
        );
        state
            .registry
            .register(info, CancellationToken::new())
            .unwrap();

        let pulses = vec![sample_pulse("email_check")];
        let infos =
            build_pulse_infos(&pulses, &[], &HashMap::new(), (Utc::now(), true), &state).await;
        assert_eq!(infos.len(), 1);
        let pulse_info = infos.first().expect("one pulse info");
        let current = pulse_info
            .current_run
            .as_ref()
            .expect("should have a current run");
        assert_eq!(current.run_id, "run-1");
        assert!(current.overlap.is_some(), "overlap flag should surface");
    }

    #[tokio::test]
    async fn build_pulse_infos_includes_last_outcome_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let info = scheduled_info("pulse:email_check", None);
        state.store.begin_run(&info).await;
        state
            .store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Failed {
                    error: "the model call timed out".to_string(),
                    details: None,
                },
                vec![],
                None,
            )
            .await;

        let pulses = vec![sample_pulse("email_check")];
        let infos =
            build_pulse_infos(&pulses, &[], &HashMap::new(), (Utc::now(), true), &state).await;
        let pulse_info = infos.first().expect("one pulse info");
        let outcome = pulse_info
            .last_outcome
            .as_ref()
            .expect("should have an outcome");
        assert_eq!(outcome.status, SessionRunStatus::Failed);
        assert_eq!(outcome.error.as_deref(), Some("the model call timed out"));
    }

    #[tokio::test]
    async fn build_pulse_infos_surfaces_a_rejected_pulse_that_never_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let problems = vec![HeartbeatProblem {
            name: "wake_main".to_string(),
            message: "pulse 'wake_main' uses agent: \"main\", which is no longer supported"
                .to_string(),
            kind: ProblemKind::RemovedOption,
        }];
        let infos =
            build_pulse_infos(&[], &problems, &HashMap::new(), (Utc::now(), true), &state).await;
        assert_eq!(infos.len(), 1);
        let info = infos.first().expect("one pulse info");
        assert_eq!(info.name, "wake_main");
        assert!(!info.enabled);
        assert_eq!(info.problems.len(), 1);
    }

    #[tokio::test]
    async fn set_enabled_endpoint_rejects_unknown_pulse() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("HEARTBEAT.yml"), "pulses: []\n")
            .await
            .unwrap();
        let state = test_state(dir.path());
        let resp = api_scheduled_pulse_set_enabled(
            State(state),
            Path("does_not_exist".to_string()),
            Ok(Json(SetEnabledRequest { enabled: false })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn set_enabled_endpoint_edits_the_file_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.yml");
        tokio::fs::write(
            &heartbeat_path,
            "pulses:\n  - name: p1\n    enabled: true\n    schedule: \"1h\"\n    tasks: []\n",
        )
        .await
        .unwrap();
        let state = test_state(dir.path());

        let resp = api_scheduled_pulse_set_enabled(
            State(state),
            Path("p1".to_string()),
            Ok(Json(SetEnabledRequest { enabled: false })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let saved = tokio::fs::read_to_string(&heartbeat_path).await.unwrap();
        assert!(saved.contains("enabled: false"));
        assert!(saved.contains("schedule: \"1h\""));
    }

    #[tokio::test]
    async fn cancel_action_endpoint_removes_it_and_404s_on_retry() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        {
            let mut store = state.action_store.lock().await;
            store.add(crate::actions::types::ScheduledAction {
                id: "action-aaaaaaaa".to_string(),
                name: "test".to_string(),
                prompt: "do it".to_string(),
                run_at: Utc::now(),
                agent: None,
                model_tier: None,
                created_at: Utc::now(),
            });
        }

        let resp =
            api_scheduled_action_cancel(State(state.clone()), Path("action-aaaaaaaa".to_string()))
                .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp_again =
            api_scheduled_action_cancel(State(state), Path("action-aaaaaaaa".to_string())).await;
        assert_eq!(resp_again.status(), StatusCode::NOT_FOUND);
    }
}

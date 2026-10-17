//! The overview's upcoming runs against the Schedule place's pulse list: the
//! hub's `upcoming` reader and the gateway's `/api/scheduled/pulses` route,
//! over the same agent files, give each pulse the same next time.
//!
//! Times in the files are in 2099, so that what the test expects does not
//! depend on the day it runs.
//!
//! covers: residuum-hub, residuum-gateway

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::actions::store::ActionStore;
use crate::background::registry::SessionRegistry;
use crate::background::store::SessionStore;
use crate::config::WorkspaceLayout;
use crate::gateway::web::scheduled::{ScheduledApiState, scheduled_api_router};
use crate::hub::AgentFiles;
use crate::hub::overview::upcoming;

/// `HEARTBEAT.yml` in `dir`, with a pulse for each of `(name, schedule,
/// active hours)`.
fn write_pulses(dir: &Path, pulses: &[(&str, &str, Option<&str>)]) {
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
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("HEARTBEAT.yml"), yaml).unwrap();
}

/// `pulse_state.json` in `dir`: when each `(pulse, local time)` last ran.
fn write_pulse_state(dir: &Path, last_runs: &[(&str, &str)]) {
    let last_run: serde_json::Map<String, Value> = last_runs
        .iter()
        .map(|(name, at)| ((*name).to_string(), json!(at)))
        .collect();
    std::fs::write(
        dir.join("pulse_state.json"),
        json!({ "last_run": last_run }).to_string(),
    )
    .unwrap();
}

/// The `config.toml` of the agent in `dir`.
fn write_config(dir: &Path, toml: &str) {
    let config = dir.join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), toml).unwrap();
}

/// The Schedule place's routes over the agent in `dir`, as its gateway builds
/// them.
fn scheduled_endpoint(dir: &Path, tz: chrono_tz::Tz) -> axum::Router {
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

/// The upcoming runs the overview lists for the agent `quiet` in `files`, as
/// of now, in the form the overview sends them.
async fn upcoming_of(files: &AgentFiles) -> Value {
    let read = upcoming::read("quiet", files, Utc::now()).await;
    serde_json::to_value(read.runs).unwrap()
}

fn instant(text: &Value) -> DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(text.as_str().unwrap()).unwrap()
}

/// Check that each pulse the endpoint gives a next time is in `upcoming` at
/// that time, and that a pulse it gives none is not there. A pulse that has
/// never run is counted from now for both, each read at its own moment, so
/// they may be a minute tick apart.
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

#[tokio::test]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes parsed JSON for clarity"
)]
async fn the_pulses_endpoint_and_the_overview_give_a_pulse_the_same_next_time() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("quiet");
    let new_york = chrono_tz::America::New_York;
    let files = AgentFiles {
        dir: dir.clone(),
        timezone: new_york,
    };
    write_pulses(
        &dir,
        &[
            ("hourly", "1h", None),
            ("pushed", "1h", Some("09:00-17:00")),
            ("never_run", "6h", None),
            ("off", "1h", None),
        ],
    );
    let heartbeat = dir.join("HEARTBEAT.yml");
    let yaml = std::fs::read_to_string(&heartbeat).unwrap();
    std::fs::write(
        &heartbeat,
        yaml.replace("  - name: off\n", "  - name: off\n    enabled: false\n"),
    )
    .unwrap();
    write_pulse_state(
        &dir,
        &[
            ("hourly", "2099-01-01T10:00:00"),
            ("pushed", "2099-01-01T16:30:00"),
            ("off", "2099-01-01T10:00:00"),
        ],
    );

    let endpoint = scheduled_endpoint(&dir, new_york);

    let listed = pulses_listed(&endpoint).await;
    let upcoming = upcoming_of(&files).await;
    assert_same_next_times(&listed, &upcoming);
    assert_eq!(listed.as_array().unwrap().len(), 4);
    assert_eq!(upcoming.as_array().unwrap().len(), 3);
    assert_eq!(
        instant(&upcoming[2]["at"]),
        DateTime::parse_from_rfc3339("2099-01-02T09:00:00-05:00").unwrap(),
        "the pushed pulse opens at 09:00 New York time"
    );

    write_config(&dir, "[pulse]\nenabled = false\n");
    let off = pulses_listed(&endpoint).await;
    assert!(
        off.as_array()
            .unwrap()
            .iter()
            .all(|pulse| pulse["next_fire_at"].is_null()),
        "no pulse runs while the pulse system is off: {off}"
    );
    let upcoming_off = upcoming_of(&files).await;
    let actual = upcoming_off.as_array().unwrap().as_slice();
    assert!(actual.is_empty(), "expected {actual:?} to be empty");
}

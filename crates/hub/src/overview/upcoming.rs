use chrono::{DateTime, Utc};
use tracing::Instrument as _;

use super::types::{UpcomingKind, UpcomingRun};
use crate::AgentFiles;
use residuum_config::Config;
use residuum_config::WorkspaceLayout;
use residuum_core::time::format_rfc3339;
use residuum_proactivity::actions::store::ActionStore;
use residuum_proactivity::pulse::next_run::{load_anchors, next_run_at};
use residuum_proactivity::pulse::types::load_heartbeat;

pub(super) const MOST_UPCOMING: usize = 3;

pub struct UpcomingRead {
    pub runs: Vec<UpcomingRun>,
    pub problems: Vec<String>,
}

type Run = (DateTime<Utc>, UpcomingKind, String);

pub async fn read(agent: &str, files: &AgentFiles, now: DateTime<Utc>) -> UpcomingRead {
    read_files(files, now)
        .instrument(tracing::info_span!("agent", agent = %agent))
        .await
}

async fn read_files(files: &AgentFiles, now: DateTime<Utc>) -> UpcomingRead {
    let layout = WorkspaceLayout::new(&files.dir);
    let timezone = files.timezone;
    let mut problems = Vec::new();

    let pulses = residuum_core::util::spawn_blocking_in_span({
        let layout = layout.clone();
        move || pulse_runs(&layout, now, timezone)
    })
    .await;
    let mut runs: Vec<Run> = match pulses {
        Ok(Ok(runs)) => runs,
        Ok(Err(config_problem)) => {
            return UpcomingRead {
                runs: Vec::new(),
                problems: vec![config_problem],
            };
        }
        Err(e) => {
            problems.push(format!(
                "reading the agent's pulses ended abnormally, so none are listed: {e}"
            ));
            Vec::new()
        }
    };

    match ActionStore::read_pending(&layout.scheduled_actions_json()).await {
        Ok(actions) => runs.extend(
            actions
                .into_iter()
                .map(|action| (action.run_at, UpcomingKind::Action, action.name)),
        ),
        Err(e) => problems.push(format!(
            "couldn't read the agent's scheduled actions, so none are listed: {e:#}"
        )),
    }

    runs.sort();
    runs.truncate(MOST_UPCOMING);
    UpcomingRead {
        runs: runs
            .into_iter()
            .map(|(at, kind, name)| UpcomingRun {
                kind,
                name,
                at: format_rfc3339(&at.with_timezone(&timezone)),
            })
            .collect(),
        problems,
    }
}

fn pulse_runs(
    layout: &WorkspaceLayout,
    now: DateTime<Utc>,
    timezone: chrono_tz::Tz,
) -> Result<Vec<Run>, String> {
    let pulse_enabled = Config::pulse_enabled_at(layout.root()).map_err(|e| {
        format!("couldn't read the agent's config, so no upcoming runs are listed: {e}")
    })?;
    if !pulse_enabled {
        return Ok(Vec::new());
    }
    let mut parse_error = None;
    let mut heartbeat_problems = Vec::new();
    let Some(heartbeat) = load_heartbeat(
        &layout.heartbeat_yml(),
        &mut parse_error,
        &mut heartbeat_problems,
        &[],
    ) else {
        return Ok(Vec::new());
    };
    let anchors = load_anchors(&layout.pulse_state_json());
    Ok(heartbeat
        .pulses
        .iter()
        .filter_map(|pulse| {
            let at = next_run_at(pulse, anchors.get(&pulse.name).copied(), now, timezone)?;
            Some((at, UpcomingKind::Pulse, pulse.name.clone()))
        })
        .collect())
}

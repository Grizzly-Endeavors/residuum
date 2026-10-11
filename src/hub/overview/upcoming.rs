//! `upcoming`: the next runs of an agent's pulses and scheduled actions, read
//! from its files, the same way whether the agent is running or not.
//!
//! A pulse's time comes from [`next_run_at`], which the Schedule place's
//! `next_fire_at` uses too. A scheduled action's is the time it was set for.

use chrono::{DateTime, Utc};
use tracing::Instrument as _;

use super::types::{UpcomingKind, UpcomingRun};
use crate::actions::store::ActionStore;
use crate::config::Config;
use crate::hub::AgentFiles;
use crate::pulse::next_run::{load_last_runs, next_run_at};
use crate::pulse::types::load_heartbeat;
use crate::time::format_rfc3339;
use crate::workspace::layout::WorkspaceLayout;

/// How many upcoming runs an overview lists.
pub(super) const MOST_UPCOMING: usize = 3;

/// What reading an agent's upcoming runs found.
pub(super) struct UpcomingRead {
    /// The next runs, soonest first, at most [`MOST_UPCOMING`].
    pub runs: Vec<UpcomingRun>,
    /// What could not be read, one plain sentence each, which the service
    /// reports once for as long as it stays unreadable.
    pub problems: Vec<String>,
}

/// One run before it is told in the agent's time: when, what, and its name.
type Run = (DateTime<Utc>, UpcomingKind, String);

/// The next runs of the agent's pulses and scheduled actions as of `now`.
///
/// An agent whose `config.toml` can't be read lists nothing: whether its
/// pulses run is that file's to say, and an agent that can't be loaded
/// can't run an action either. An unreadable `scheduled_actions.json` costs
/// only the actions, and a `HEARTBEAT.yml` with a problem lists the pulses
/// that loaded. The agent's own scheduler reports the pulse problems.
pub(super) async fn read(agent: &str, files: &AgentFiles, now: DateTime<Utc>) -> UpcomingRead {
    // The pulse files are read by the code the agent's own scheduler uses, which
    // logs on the agent's behalf.
    read_files(files, now)
        .instrument(tracing::info_span!("agent", agent = %agent))
        .await
}

async fn read_files(files: &AgentFiles, now: DateTime<Utc>) -> UpcomingRead {
    let layout = WorkspaceLayout::new(&files.dir);
    let timezone = files.timezone;
    let mut problems = Vec::new();

    let pulses = crate::util::spawn_blocking_in_span({
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

/// The next run of each pulse that will run. `Err` carries the sentence for a
/// `config.toml` that can't be read.
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
    let last_runs = load_last_runs(&layout.pulse_state_json());
    Ok(heartbeat
        .pulses
        .iter()
        .filter_map(|pulse| {
            let at = next_run_at(pulse, last_runs.get(&pulse.name).copied(), now, timezone)?;
            Some((at, UpcomingKind::Pulse, pulse.name.clone()))
        })
        .collect())
}

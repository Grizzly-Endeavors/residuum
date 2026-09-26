//! Pulse execution handling and scheduling in the event loop.

use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::bus::{PulseOverlap, topics};
use crate::gateway::helpers::publish_notice;
use crate::gateway::types::GatewayRuntime;

/// Fork a session for a single due pulse.
#[tracing::instrument(skip_all)]
pub async fn handle_pulse_execution(
    spawn_event: crate::bus::SpawnRequestEvent,
    rt: &mut GatewayRuntime,
) {
    let source_label = spawn_event.source_label.clone();
    if let Err(e) = rt.publisher.publish(topics::Background, spawn_event).await {
        tracing::warn!(pulse = %source_label, error = %e, "failed to publish pulse spawn request");
    }
}

/// Look for a still-live session from an earlier fire of this pulse. Never
/// used to block or skip the new fire — only to flag it, since the owner
/// decided pulses that overlap their own previous run should start anyway.
fn detect_pulse_overlap(registry: &SessionRegistry, pulse_name: &str) -> Option<PulseOverlap> {
    let source_label = format!("pulse:{pulse_name}");
    registry
        .list_live()
        .into_iter()
        .find(|info| {
            info.category == SessionCategory::Scheduled && info.source_label == source_label
        })
        .map(|info| PulseOverlap {
            previous_run_id: info.run_id,
            previous_started_at: info.started_at,
        })
}

/// Build the plain-language outcome of this tick's `HEARTBEAT.yml` reload,
/// for delivery to the agent whose own write caused it (see
/// `ConfigWriteWatch`). Independent of the deduped, change-only user notice
/// `take_problem_notice` returns — the agent asked about this specific
/// write, not "is there anything new to tell the owner".
fn heartbeat_reload_outcome(scheduler: &crate::pulse::scheduler::PulseScheduler) -> String {
    if let Some(err) = scheduler.last_parse_error() {
        return format!(
            "HEARTBEAT.yml failed to parse, your previous pulses are still running: {err}"
        );
    }
    let problems = scheduler.current_problems();
    if problems.is_empty() {
        "HEARTBEAT.yml reloaded cleanly — no problems found".to_string()
    } else {
        crate::pulse::types::heartbeat_problems_notice(problems)
    }
}

/// Process all due pulses.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn handle_pulse_tick(rt: &mut GatewayRuntime) {
    use crate::pulse::executor::build_pulse_execution;
    use crate::time;

    // Consumed once, up front: whether this tick is the first one after the
    // agent's own write to HEARTBEAT.yml — see `ConfigWriteWatch`. Unlike
    // config.toml/mcp.json, HEARTBEAT.yml has no discrete `ReloadSignal`; it
    // hot-reloads on every scheduler tick, so "the reload this write
    // triggered" is simply the very next tick.
    let deliver_to_agent = rt
        .config_reload_tracker
        .take_if_matches(crate::tools::config_reload_tracker::ConfigReloadKind::Heartbeat);

    let now = time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
    if deliver_to_agent {
        let outcome = heartbeat_reload_outcome(&rt.pulse_scheduler);
        rt.agent.inject_system_message(outcome);
    }
    if let Some(notice) = rt.pulse_scheduler.take_problem_notice() {
        publish_notice(&rt.publisher, notice).await;
    }
    if !due.is_empty() {
        tracing::debug!(count = due.len(), "processing due pulses");
    }
    for pulse in &due {
        let overlap = detect_pulse_overlap(&rt.session_registry, &pulse.name);
        if let Some(ov) = &overlap {
            let still_going_secs = (chrono::Utc::now() - ov.previous_started_at)
                .num_seconds()
                .max(0);
            tracing::info!(
                pulse = %pulse.name,
                previous_run_id = %ov.previous_run_id,
                previous_running_for_secs = still_going_secs,
                "pulse fired again while its previous run is still going; starting the new run \
                 anyway and flagging the overlap"
            );
        }
        let spawn_event = build_pulse_execution(pulse, overlap);
        handle_pulse_execution(spawn_event, rt).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::scheduler::PulseScheduler;

    #[test]
    fn a_clean_heartbeat_reports_reloaded_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "pulses: []\n").unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);

        assert_eq!(
            heartbeat_reload_outcome(&scheduler),
            "HEARTBEAT.yml reloaded cleanly — no problems found"
        );
    }

    #[test]
    fn a_heartbeat_with_problems_reports_them_regardless_of_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = "pulses:\n  - name: dup\n    schedule: \"1h\"\n    tasks: []\n  - name: dup\n    schedule: \"2h\"\n    tasks: []\n";
        std::fs::write(&path, yaml).unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);
        // Drain the deduped owner notice — the agent-facing outcome must not
        // depend on whether anything is left for the owner to be told.
        let _notice = scheduler.take_problem_notice();

        let outcome = heartbeat_reload_outcome(&scheduler);
        assert!(
            outcome.contains("dup"),
            "outcome should name the problem even with no pending owner notice: {outcome}"
        );
    }

    #[test]
    fn an_unparseable_heartbeat_reports_the_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);

        let outcome = heartbeat_reload_outcome(&scheduler);
        assert!(
            outcome.contains("failed to parse"),
            "outcome should say the file failed to parse: {outcome}"
        );
    }
}

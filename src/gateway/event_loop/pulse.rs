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

/// Process all due pulses.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn handle_pulse_tick(rt: &mut GatewayRuntime) {
    use crate::pulse::executor::build_pulse_execution;
    use crate::time;

    let now = time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
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

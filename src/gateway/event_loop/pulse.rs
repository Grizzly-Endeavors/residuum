//! Pulse execution handling and scheduling in the event loop.

use crate::bus::topics;
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
        let spawn_event = build_pulse_execution(pulse);
        handle_pulse_execution(spawn_event, rt).await;
    }
}

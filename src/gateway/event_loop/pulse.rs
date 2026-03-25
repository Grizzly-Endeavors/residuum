//! Pulse execution handling and scheduling in the event loop.

use crate::bus::topics;
use crate::gateway::types::GatewayRuntime;
use crate::memory::types::Visibility;
use crate::models::Message;
use crate::pulse::executor::PulseExecution;

/// Handle a single pulse execution entry (main-turn or sub-agent).
#[tracing::instrument(skip_all)]
pub async fn handle_pulse_execution(
    execution: PulseExecution,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    match execution {
        PulseExecution::MainWakeTurn { pulse_name, prompt } => {
            tracing::info!(pulse = %pulse_name, "scheduled pulse firing as main wake turn");
            let formatted = format!("[Scheduled pulse: {pulse_name}]\n{prompt}");
            rt.agent.inject_system_message(formatted.clone());
            let msgs = [Message::system(&formatted)];
            super::turns::persist_and_maybe_observe(
                rt,
                &msgs,
                Visibility::Background,
                observe_deadline,
            )
            .await;
        }
        PulseExecution::SubAgent { spawn_event } => {
            let topic = topics::Background;
            let preset_name = spawn_event.preset.as_ref().to_string();
            if let Err(e) = rt.publisher.publish(topic, spawn_event).await {
                tracing::warn!(pulse = %preset_name, error = %e, "failed to publish pulse spawn request");
            }
        }
    }
}

/// Process all due pulses and optionally trigger a wake turn.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn handle_pulse_tick(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    use crate::pulse::executor::build_pulse_execution;
    use crate::time;

    let now = time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
    if !due.is_empty() {
        tracing::debug!(count = due.len(), "processing due pulses");
    }
    for pulse in &due {
        let exec = build_pulse_execution(pulse);
        handle_pulse_execution(exec, rt, observe_deadline).await;
    }
}

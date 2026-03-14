//! Pulse execution handling and scheduling in the event loop.

use crate::bus::{BusEvent, EventTrigger, PresetName, SpawnRequestEvent, TopicId};
use crate::gateway::types::GatewayRuntime;
use crate::memory::types::Visibility;
use crate::models::Message;
use crate::pulse::executor::PulseExecution;

/// Handle a single pulse execution entry (main-turn or sub-agent).
pub async fn handle_pulse_execution(
    execution: PulseExecution,
    pulse_name: &str,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    match execution {
        PulseExecution::MainWakeTurn {
            pulse_name: _,
            prompt,
        } => {
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
            true
        }
        PulseExecution::SubAgent {
            task,
            preset_name: Some(name),
        } => {
            let crate::background::types::Execution::SubAgent(cfg) = &task.execution;
            let crate::background::types::ResultRouting::Direct(channels) = &task.routing;
            let spawn_event = SpawnRequestEvent {
                task_name: task.task_name.clone(),
                prompt: cfg.prompt.clone(),
                context: cfg.context.clone(),
                source: EventTrigger::Pulse,
                model_tier_override: Some(cfg.model_tier),
                routing_override: Some(channels.clone()),
            };
            let topic = TopicId::AgentPreset(PresetName::from(name.as_str()));
            if let Err(e) = rt
                .publisher
                .publish(topic, BusEvent::SpawnRequest(spawn_event))
                .await
            {
                tracing::warn!(pulse = %pulse_name, error = %e, "failed to publish pulse spawn request");
            }
            false
        }
        PulseExecution::SubAgent {
            task,
            preset_name: None,
        } => {
            let crate::background::types::Execution::SubAgent(cfg) = &task.execution;
            let crate::background::types::ResultRouting::Direct(channels) = &task.routing;
            let spawn_event = SpawnRequestEvent {
                task_name: task.task_name.clone(),
                prompt: cfg.prompt.clone(),
                context: cfg.context.clone(),
                source: EventTrigger::Pulse,
                model_tier_override: Some(cfg.model_tier),
                routing_override: Some(channels.clone()),
            };
            let topic = TopicId::AgentPreset(PresetName::from("general-purpose"));
            if let Err(e) = rt
                .publisher
                .publish(topic, BusEvent::SpawnRequest(spawn_event))
                .await
            {
                tracing::warn!(pulse = %pulse_name, error = %e, "failed to publish pulse spawn request");
            }
            false
        }
    }
}

/// Process all due pulses and optionally trigger a wake turn.
pub async fn handle_pulse_tick(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<crate::gateway::types::GatewayExit> {
    use crate::pulse::executor::build_pulse_execution;
    use crate::time;

    let now = time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
    for pulse in &due {
        let name = pulse.name.clone();
        let exec = build_pulse_execution(pulse);
        handle_pulse_execution(exec, &name, rt, observe_deadline).await;
    }
    None
}

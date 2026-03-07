//! Pulse execution handling and scheduling in the event loop.

use crate::background::spawn_context::load_preset_for_spawn;
use crate::config::BackgroundModelTier;
use crate::gateway::types::GatewayRuntime;
use crate::memory::types::Visibility;
use crate::models::Message;
use crate::pulse::executor::PulseExecution;

use super::turns::run_wake_turn_handler;
use crate::background::spawn_context as spawn_helpers;

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
            super::turns::persist_and_maybe_observe(rt, &msgs, Visibility::Background, observe_deadline).await;
            true
        }
        PulseExecution::SubAgent {
            task,
            preset_name: Some(name),
        } => {
            match load_preset_for_spawn(
                &rt.layout.subagents_dir(),
                &name,
                BackgroundModelTier::Small,
            )
            .await
            {
                Ok((tier, preset)) => {
                    let preset_arg = preset.as_ref().map(|(fm, body)| (fm, body.clone()));
                    match spawn_helpers::build_spawn_resources(
                        &rt.spawn_context,
                        &tier,
                        &rt.project_state,
                        &rt.skill_state,
                        std::sync::Arc::clone(&rt.mcp_registry),
                        preset_arg,
                    )
                    .await
                    {
                        Ok(resources) => {
                            if let Err(e) = rt.background_spawner.spawn(task, Some(resources)).await
                            {
                                tracing::warn!(pulse = %pulse_name, error = %e, "failed to spawn pulse task with preset");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(pulse = %pulse_name, error = %e, "failed to build pulse resources with preset");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(pulse = %pulse_name, preset = %name, error = %e, "failed to load preset for pulse");
                }
            }
            false
        }
        PulseExecution::SubAgent {
            task,
            preset_name: None,
        } => {
            let crate::background::types::Execution::SubAgent(cfg) = &task.execution;
            let tier = cfg.model_tier;
            match spawn_helpers::build_spawn_resources(
                &rt.spawn_context,
                &tier,
                &rt.project_state,
                &rt.skill_state,
                std::sync::Arc::clone(&rt.mcp_registry),
                None,
            )
            .await
            {
                Ok(resources) => {
                    if let Err(e) = rt.background_spawner.spawn(task, Some(resources)).await {
                        tracing::warn!(pulse = %pulse_name, error = %e, "failed to spawn pulse task");
                    }
                }
                Err(e) => {
                    tracing::warn!(pulse = %pulse_name, error = %e, "failed to build pulse resources");
                }
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
    let mut wake_requested = false;
    for pulse in &due {
        let name = pulse.name.clone();
        let exec = build_pulse_execution(pulse);
        if handle_pulse_execution(exec, &name, rt, observe_deadline).await {
            wake_requested = true;
        }
    }
    if wake_requested {
        return run_wake_turn_handler(rt, observe_deadline).await;
    }
    None
}

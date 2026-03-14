//! Pulse task builder: converts a pulse definition into a spawn request or main wake turn.

use crate::bus::EventTrigger;
use crate::bus::SpawnRequestEvent;

use super::types::{PulseDef, default_channels};

/// The execution strategy for a pulse.
#[derive(Debug)]
pub enum PulseExecution {
    /// Spawn a sub-agent via the bus (default behavior, optionally with a preset).
    SubAgent {
        /// The spawn request event to publish.
        spawn_event: SpawnRequestEvent,
        /// Preset name for topic routing (defaults to `"general-purpose"`).
        preset_name: String,
    },
    /// Inject the prompt and trigger a main agent wake turn.
    MainWakeTurn {
        /// Name of the pulse (for logging/formatting).
        pulse_name: String,
        /// Combined prompt from all pulse tasks.
        prompt: String,
    },
}

/// Build a `PulseExecution` from a pulse definition.
///
/// - `agent: None` → `SubAgent` with `general-purpose` preset
/// - `agent: Some("main")` → `MainWakeTurn` with the combined prompt
/// - `agent: Some(name)` → `SubAgent` with the named preset (tier resolved later)
#[must_use]
pub fn build_pulse_execution(pulse: &PulseDef) -> PulseExecution {
    let prompt = build_pulse_prompt(pulse);
    let source_label = format!("pulse:{}", pulse.name);

    match pulse.agent.as_deref() {
        Some("main") => {
            if pulse.channels != default_channels() {
                tracing::warn!(
                    pulse = %pulse.name,
                    "channels field is ignored for main-turn pulses"
                );
            }
            PulseExecution::MainWakeTurn {
                pulse_name: pulse.name.clone(),
                prompt,
            }
        }
        Some(preset) => {
            let spawn_event = SpawnRequestEvent {
                source_label,
                prompt,
                context: None,
                source: EventTrigger::Pulse,
                model_tier_override: Some(crate::config::BackgroundModelTier::Small),
                routing_override: Some(pulse.channels.clone()),
            };
            PulseExecution::SubAgent {
                spawn_event,
                preset_name: preset.to_string(),
            }
        }
        None => {
            let spawn_event = SpawnRequestEvent {
                source_label,
                prompt,
                context: None,
                source: EventTrigger::Pulse,
                model_tier_override: Some(crate::config::BackgroundModelTier::Small),
                routing_override: Some(pulse.channels.clone()),
            };
            PulseExecution::SubAgent {
                spawn_event,
                preset_name: "general-purpose".to_string(),
            }
        }
    }
}

/// Build the prompt string for a pulse check.
fn build_pulse_prompt(pulse: &PulseDef) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "You are running a scheduled pulse check: {}",
        pulse.name
    ));

    parts.push("## Tasks".to_string());

    for task in &pulse.tasks {
        parts.push(format!("### {}\n{}", task.name, task.prompt));
    }

    parts.push(
        "Complete all tasks above. If nothing noteworthy was found across all tasks, \
         respond with exactly: HEARTBEAT_OK"
            .to_string(),
    );

    parts.join("\n\n")
}

#[cfg(test)]
#[expect(clippy::panic, reason = "test code panics on unexpected match arm")]
mod tests {
    use super::*;
    use crate::pulse::types::PulseTask;

    fn sample_pulse() -> PulseDef {
        PulseDef {
            name: "email_check".to_string(),
            enabled: true,
            schedule: "30m".to_string(),
            active_hours: None,
            agent: None,
            trigger_count: None,
            channels: vec!["inbox".to_string()],
            tasks: vec![
                PulseTask {
                    name: "check_inbox".to_string(),
                    prompt: "Check for new emails.".to_string(),
                },
                PulseTask {
                    name: "check_alerts".to_string(),
                    prompt: "Review alert dashboard.".to_string(),
                },
            ],
        }
    }

    // ── build_pulse_execution tests ─────────────────────────────────────

    #[test]
    fn execution_no_agent_returns_subagent_general_purpose() {
        let pulse = sample_pulse();
        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent {
                preset_name,
                spawn_event,
            } => {
                assert_eq!(preset_name, "general-purpose");
                assert_eq!(spawn_event.source_label, "pulse:email_check");
                assert!(spawn_event.prompt.contains("email_check"));
                assert!(spawn_event.prompt.contains("HEARTBEAT_OK"));
            }
            PulseExecution::MainWakeTurn { .. } => panic!("expected SubAgent"),
        }
    }

    #[test]
    fn execution_agent_main_returns_wake_turn() {
        let mut pulse = sample_pulse();
        pulse.agent = Some("main".to_string());
        match build_pulse_execution(&pulse) {
            PulseExecution::MainWakeTurn { pulse_name, prompt } => {
                assert_eq!(pulse_name, "email_check");
                assert!(prompt.contains("HEARTBEAT_OK"));
                assert!(prompt.contains("check_inbox"));
            }
            PulseExecution::SubAgent { .. } => panic!("expected MainWakeTurn"),
        }
    }

    #[test]
    fn execution_agent_preset_returns_subagent_with_preset() {
        let mut pulse = sample_pulse();
        pulse.agent = Some("memory-agent".to_string());
        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent {
                preset_name,
                spawn_event,
            } => {
                assert_eq!(preset_name, "memory-agent");
                assert_eq!(spawn_event.source_label, "pulse:email_check");
            }
            PulseExecution::MainWakeTurn { .. } => panic!("expected SubAgent"),
        }
    }

    #[test]
    fn prompt_contains_pulse_name_and_tasks() {
        let pulse = sample_pulse();
        let prompt = build_pulse_prompt(&pulse);

        assert!(
            prompt.contains("email_check"),
            "prompt should contain pulse name"
        );
        assert!(
            prompt.contains("check_inbox"),
            "prompt should contain task name"
        );
        assert!(
            prompt.contains("Check for new emails"),
            "prompt should contain task prompt"
        );
        assert!(
            prompt.contains("check_alerts"),
            "prompt should contain second task"
        );
    }

    #[test]
    fn prompt_ends_with_heartbeat_ok_instruction() {
        let pulse = sample_pulse();
        let prompt = build_pulse_prompt(&pulse);

        assert!(
            prompt.contains("HEARTBEAT_OK"),
            "prompt should contain HEARTBEAT_OK instruction"
        );
    }

    #[test]
    fn routing_defaults_to_inbox() {
        let pulse = sample_pulse();
        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent { spawn_event, .. } => {
                assert_eq!(
                    spawn_event.routing_override,
                    Some(vec!["inbox".to_string()]),
                    "should route to default channel"
                );
            }
            PulseExecution::MainWakeTurn { .. } => panic!("expected SubAgent"),
        }
    }

    #[test]
    fn empty_tasks_pulse_still_builds() {
        let pulse = PulseDef {
            name: "empty".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            trigger_count: None,
            channels: vec!["inbox".to_string()],
            tasks: vec![],
        };
        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent { spawn_event, .. } => {
                assert_eq!(spawn_event.source_label, "pulse:empty");
                assert!(
                    spawn_event.prompt.contains("HEARTBEAT_OK"),
                    "should still have HEARTBEAT_OK instruction"
                );
            }
            PulseExecution::MainWakeTurn { .. } => panic!("expected SubAgent"),
        }
    }
}

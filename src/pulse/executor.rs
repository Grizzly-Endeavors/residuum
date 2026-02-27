//! Pulse task builder: converts a pulse definition into a background task or main wake turn.

use crate::background::types::{BackgroundTask, Execution, ResultRouting, SubAgentConfig};
use crate::config::BackgroundModelTier;
use crate::notify::types::TaskSource;

use super::types::PulseDef;

/// The execution strategy for a pulse.
#[derive(Debug)]
pub enum PulseExecution {
    /// Spawn a sub-agent background task (default behavior, optionally with a preset).
    SubAgent {
        /// The background task to spawn.
        task: BackgroundTask,
        /// If set, load this preset to configure the sub-agent.
        preset_name: Option<String>,
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
/// - `agent: None` → `SubAgent` with `Small` tier, no preset (backward-compat)
/// - `agent: Some("main")` → `MainWakeTurn` with the combined prompt
/// - `agent: Some(name)` → `SubAgent` with the named preset (tier resolved later)
#[must_use]
pub fn build_pulse_execution(pulse: &PulseDef) -> PulseExecution {
    let prompt = build_pulse_prompt(pulse);

    match pulse.agent.as_deref() {
        Some("main") => PulseExecution::MainWakeTurn {
            pulse_name: pulse.name.clone(),
            prompt,
        },
        Some(preset) => {
            let task = build_background_task(pulse, prompt, BackgroundModelTier::Small);
            PulseExecution::SubAgent {
                task,
                preset_name: Some(preset.to_string()),
            }
        }
        None => {
            let task = build_background_task(pulse, prompt, BackgroundModelTier::Small);
            PulseExecution::SubAgent {
                task,
                preset_name: None,
            }
        }
    }
}

/// Build a `BackgroundTask` from a pulse definition (backward-compat wrapper).
///
/// Always produces a `SubAgent` at `Small` tier with `ResultRouting::Notify`.
#[must_use]
pub fn build_pulse_task(pulse: &PulseDef) -> BackgroundTask {
    let prompt = build_pulse_prompt(pulse);
    build_background_task(pulse, prompt, BackgroundModelTier::Small)
}

/// Create a `BackgroundTask` for a pulse with the given prompt and tier.
fn build_background_task(
    pulse: &PulseDef,
    prompt: String,
    tier: BackgroundModelTier,
) -> BackgroundTask {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    BackgroundTask {
        id: format!("pulse-{}-{timestamp_ms}", pulse.name),
        task_name: pulse.name.clone(),
        source: TaskSource::Pulse,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt,
            context: None,
            model_tier: tier,
        }),
        routing: ResultRouting::Notify,
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
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unreachable variants"
)]
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

    #[test]
    fn task_name_matches_pulse_name() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert_eq!(task.task_name, "email_check");
    }

    #[test]
    fn task_id_contains_pulse_name() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            task.id.starts_with("pulse-email_check-"),
            "id should start with pulse-<name>-"
        );
    }

    #[test]
    fn source_is_pulse() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            matches!(task.source, TaskSource::Pulse),
            "source should be Pulse"
        );
    }

    #[test]
    fn execution_is_subagent_small() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        match &task.execution {
            Execution::SubAgent(cfg) => {
                assert_eq!(
                    cfg.model_tier,
                    BackgroundModelTier::Small,
                    "tier should be Small"
                );
            }
            Execution::Script(_) => panic!("expected SubAgent execution"),
        }
    }

    #[test]
    fn prompt_contains_pulse_name_and_tasks() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        let prompt = match &task.execution {
            Execution::SubAgent(cfg) => &cfg.prompt,
            Execution::Script(_) => panic!("expected SubAgent"),
        };

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
        let task = build_pulse_task(&pulse);
        let prompt = match &task.execution {
            Execution::SubAgent(cfg) => &cfg.prompt,
            Execution::Script(_) => panic!("expected SubAgent"),
        };

        assert!(
            prompt.contains("HEARTBEAT_OK"),
            "prompt should contain HEARTBEAT_OK instruction"
        );
    }

    #[test]
    fn routing_is_notify() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            matches!(task.routing, ResultRouting::Notify),
            "routing should be Notify"
        );
    }

    #[test]
    fn empty_tasks_pulse_still_builds() {
        let pulse = PulseDef {
            name: "empty".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            tasks: vec![],
        };
        let task = build_pulse_task(&pulse);
        assert_eq!(task.task_name, "empty");
        let prompt = match &task.execution {
            Execution::SubAgent(cfg) => &cfg.prompt,
            Execution::Script(_) => panic!("expected SubAgent"),
        };
        assert!(
            prompt.contains("HEARTBEAT_OK"),
            "should still have HEARTBEAT_OK instruction"
        );
    }

    // ── build_pulse_execution tests ─────────────────────────────────────

    #[test]
    fn execution_no_agent_returns_subagent_no_preset() {
        let pulse = sample_pulse();
        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent { preset_name, task } => {
                assert!(preset_name.is_none(), "should have no preset");
                assert_eq!(task.task_name, "email_check");
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
            PulseExecution::SubAgent { preset_name, task } => {
                assert_eq!(preset_name.as_deref(), Some("memory-agent"));
                assert_eq!(task.task_name, "email_check");
            }
            PulseExecution::MainWakeTurn { .. } => panic!("expected SubAgent"),
        }
    }
}

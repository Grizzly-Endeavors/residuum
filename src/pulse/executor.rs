//! Pulse task builder: converts a pulse definition into a background task.

use crate::background::types::{BackgroundTask, Execution, ResultRouting, SubAgentConfig};
use crate::config::BackgroundModelTier;
use crate::notify::types::TaskSource;

use super::types::PulseDef;

/// Build a `BackgroundTask` from a pulse definition.
///
/// The task prompt mirrors the old `execute_pulse()` format: header, numbered
/// task sections, and a trailing `HEARTBEAT_OK` instruction. The task is
/// configured as a `SubAgent` at `Small` tier with `ResultRouting::Notify`.
#[must_use]
pub fn build_pulse_task(pulse: &PulseDef) -> BackgroundTask {
    let prompt = build_pulse_prompt(pulse);
    let timestamp_ms = chrono::Utc::now().timestamp_millis();

    BackgroundTask {
        id: format!("pulse-{}-{timestamp_ms}", pulse.name),
        task_name: pulse.name.clone(),
        source: TaskSource::Pulse,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt,
            context: None,
            context_files: Vec::new(),
            model_tier: BackgroundModelTier::Small,
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
}

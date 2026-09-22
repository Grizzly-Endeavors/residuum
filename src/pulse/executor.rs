//! Pulse task builder: converts a pulse definition into a spawn request.

use crate::background::registry::{MAIN_DEPTH, generate_address};
use crate::bus::{EventTrigger, HEARTBEAT_OK, HEARTBEAT_URGENT, SpawnRequestEvent};

use super::types::PulseDef;

/// Build a `SpawnRequestEvent` from a pulse definition.
///
/// Every pulse runs as a `scheduled` session: `agent: None` runs on the
/// pulse prompt alone, `agent: Some(name)` activates that skill as the
/// session's role. `agent: "main"` is rejected at load time (see
/// `validate_pulse`) and never reaches this function.
///
/// The model tier comes from the pulse's own `model_tier`, defaulting to `small`.
#[must_use]
pub fn build_pulse_execution(pulse: &PulseDef) -> SpawnRequestEvent {
    let prompt = build_pulse_prompt(pulse);
    let skill = pulse.agent.as_deref();

    tracing::debug!(
        pulse = %pulse.name,
        skill = skill.unwrap_or("none"),
        "routing pulse to scheduled session"
    );
    let model_tier = pulse
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(crate::config::BackgroundModelTier::Small);

    let trigger = EventTrigger::Pulse;
    let address = generate_address(&trigger, &pulse.name);

    SpawnRequestEvent {
        address,
        skill: skill.map(crate::bus::SkillName::from),
        source_label: format!("pulse:{}", pulse.name),
        prompt,
        context: None,
        source: trigger,
        model_tier,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
    }
}

/// Build the prompt string for a pulse check.
fn build_pulse_prompt(pulse: &PulseDef) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "You are running a scheduled pulse check: {}",
        pulse.name
    ));
    parts.push(
        "This run is autonomous — no user is present, so you cannot ask questions or request \
         clarification; act on the tasks as written. Do not create or modify pulses \
         (HEARTBEAT.yml)."
            .to_string(),
    );

    parts.push("## Tasks".to_string());

    for task in &pulse.tasks {
        parts.push(format!("### {}\n{}", task.name, task.prompt));
    }

    parts.push(format!(
        "Complete all tasks above. If nothing noteworthy was found across all tasks, \
         respond with exactly: {HEARTBEAT_OK}\n\n\
         Otherwise report what you found. Your report is filed for review. If — and \
         only if — it needs attention before the user would next check in, end your \
         report with {HEARTBEAT_URGENT} on its own line; that also pushes it to every \
         notification channel the user has configured. Judge this from what you \
         actually found, not from the topic you were asked to watch."
    ));

    parts.join("\n\n")
}

#[cfg(test)]
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
            model_tier: None,
            include_identity: None,
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
    fn execution_no_agent_has_no_skill() {
        let pulse = sample_pulse();
        let spawn_event = build_pulse_execution(&pulse);
        assert_eq!(spawn_event.skill, None);
        assert_eq!(spawn_event.source_label, "pulse:email_check");
        assert!(spawn_event.prompt.contains("email_check"));
        assert!(spawn_event.prompt.contains(HEARTBEAT_OK));
        assert!(matches!(spawn_event.source, EventTrigger::Pulse));
        assert!(matches!(
            spawn_event.model_tier,
            crate::config::BackgroundModelTier::Small
        ));
        assert!(
            spawn_event
                .address
                .as_ref()
                .starts_with("scheduled-email-check-")
        );
    }

    #[test]
    fn execution_agent_name_activates_skill() {
        let mut pulse = sample_pulse();
        pulse.agent = Some("memory-agent".to_string());
        let spawn_event = build_pulse_execution(&pulse);
        assert_eq!(
            spawn_event.skill.as_ref().map(AsRef::as_ref),
            Some("memory-agent")
        );
        assert_eq!(spawn_event.source_label, "pulse:email_check");
        assert!(matches!(spawn_event.source, EventTrigger::Pulse));
        // A pulse that names no tier runs small, whether or not it
        // names a skill — the tier is the pulse's own setting.
        assert!(matches!(
            spawn_event.model_tier,
            crate::config::BackgroundModelTier::Small
        ));
    }

    #[test]
    fn prompt_teaches_both_sentinels() {
        let prompt = build_pulse_prompt(&sample_pulse());
        assert!(
            prompt.contains(HEARTBEAT_OK),
            "session must be told how to exit silently"
        );
        assert!(
            prompt.contains(HEARTBEAT_URGENT),
            "session must be told how to escalate"
        );
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
    fn prompt_includes_autonomous_context_framing() {
        let pulse = sample_pulse();
        let prompt = build_pulse_prompt(&pulse);

        assert!(
            prompt.contains("autonomous"),
            "prompt should flag the run as autonomous"
        );
        assert!(
            prompt.contains("cannot ask questions"),
            "prompt should tell the agent it cannot ask for clarification"
        );
        assert!(
            prompt.contains("HEARTBEAT.yml"),
            "prompt should forbid modifying pulses from a pulse run"
        );
    }

    #[test]
    fn prompt_does_not_forbid_further_background_work() {
        let prompt = build_pulse_prompt(&sample_pulse());
        assert!(
            !prompt.contains("schedule further background work"),
            "scheduled sessions may spawn within the depth cap like any other session"
        );
    }

    #[test]
    fn prompt_ends_with_heartbeat_ok_instruction() {
        let pulse = sample_pulse();
        let prompt = build_pulse_prompt(&pulse);

        assert!(
            prompt.contains(HEARTBEAT_OK),
            "prompt should contain HEARTBEAT_OK instruction"
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
            model_tier: None,
            include_identity: None,
            tasks: vec![],
        };
        let spawn_event = build_pulse_execution(&pulse);
        assert_eq!(spawn_event.source_label, "pulse:empty");
        assert!(
            spawn_event.prompt.contains(HEARTBEAT_OK),
            "should still have HEARTBEAT_OK instruction"
        );
    }
}

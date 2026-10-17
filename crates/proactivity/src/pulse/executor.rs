//! Pulse task builder: converts a pulse definition into a spawn request.

use residuum_bus::session::{MAIN_DEPTH, generate_address};
use residuum_bus::{EventTrigger, HEARTBEAT_OK, HEARTBEAT_URGENT, PulseOverlap, SpawnRequestEvent};

use super::script::{ScriptRun, prompt_section};
use super::types::PulseDef;

/// A pulse's `context_from` target resolved to its stored output, for
/// injection into the firing pulse's prompt. Built by the caller (see
/// the gateway's `handle_pulse_tick`) from
/// `PulseScheduler`'s persisted `last_output` map — single-hop only, so
/// `output` is always that pulse's own stored output, never resolved
/// through a `context_from` chain.
pub struct PulseContext {
    /// Name of the pulse named by this pulse's `context_from`.
    pub source_pulse: String,
    /// The source pulse's last delivered output, or `None` if it has never
    /// delivered one (never fired, or fired but never completed with
    /// deliverable output).
    pub output: Option<String>,
}

/// Build a `SpawnRequestEvent` from a pulse definition.
///
/// Every pulse runs as a `scheduled` session: `agent: None` runs on the
/// pulse prompt alone, `agent: Some(name)` activates that skill as the
/// session's role. `agent: "main"` is rejected at load time (see
/// `validate_pulse`) and never reaches this function.
///
/// The model comes from the pulse's own `model_tier` (a tier or an explicit
/// model), defaulting to the `small` tier.
///
/// `overlap` is `Some` when the caller found this pulse's previous run still
/// live in the session registry — the new run still starts normally, this
/// only tags it so the overlap is visible in the Schedule place and on the
/// run itself in the session panel (see the gateway's pulse event loop).
///
/// `context` is `Some` when `pulse.context_from` names another pulse — see
/// [`PulseContext`] — and is folded into the prompt by `build_pulse_prompt`.
///
/// `scripts` lines up with `pulse.tasks`: each task's script run, already
/// finished, whose output (or failure) joins that task's section of the
/// prompt. Empty when the pulse has no scripts.
#[must_use]
pub fn build_pulse_execution(
    pulse: &PulseDef,
    overlap: Option<PulseOverlap>,
    context: Option<&PulseContext>,
    scripts: &[Option<ScriptRun>],
) -> SpawnRequestEvent {
    let prompt = build_pulse_prompt(pulse, context, scripts);
    let skill = pulse.agent.as_deref();

    tracing::debug!(
        pulse = %pulse.name,
        skill = skill.unwrap_or("none"),
        "routing pulse to scheduled session"
    );
    let model = pulse
        .model_tier
        .clone()
        .unwrap_or(residuum_config::ModelSelector::SMALL);

    let trigger = EventTrigger::Pulse;
    let address = generate_address(&trigger, &pulse.name);

    SpawnRequestEvent {
        address,
        skill: skill.map(residuum_bus::SkillName::from),
        source_label: format!("pulse:{}", pulse.name),
        prompt,
        context: None,
        source: trigger,
        model,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: Vec::new(),
        delivered_chat_messages: Vec::new(),
    }
}

/// Build the prompt string for a pulse check.
///
/// `context` — resolved from `pulse.context_from` by the caller — is folded
/// in as its own section naming the source pulse, whether or not it had
/// output to give: a `None` `context.output` (the source pulse doesn't
/// exist, or has never delivered output) still says so explicitly rather
/// than silently omitting the section, so the run isn't left guessing
/// whether context injection is even configured.
fn build_pulse_prompt(
    pulse: &PulseDef,
    context: Option<&PulseContext>,
    scripts: &[Option<ScriptRun>],
) -> String {
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

    if let Some(context) = context {
        parts.push(build_context_section(context));
    }

    parts.push("## Tasks".to_string());

    for (index, task) in pulse.tasks.iter().enumerate() {
        let mut section = format!("### {}", task.name);
        if !task.prompt.trim().is_empty() {
            section.push('\n');
            section.push_str(&task.prompt);
        }
        if let Some(run) = scripts.get(index).and_then(Option::as_ref) {
            section.push_str("\n\n");
            section.push_str(&prompt_section(run));
        }
        parts.push(section);
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

/// Build the `## Context from <pulse>` section for a pulse that sets
/// `context_from`, framing the injected text as another pulse's own past
/// output rather than something this run should treat as an instruction.
fn build_context_section(context: &PulseContext) -> String {
    match &context.output {
        Some(output) => format!(
            "## Context from '{name}'\n\
             The following is the most recent output pulse '{name}' delivered (not a live \
             re-run — this is what it reported last time it fired):\n\n{output}",
            name = context.source_pulse,
        ),
        None => format!(
            "## Context from '{name}'\n\
             No prior output from '{name}' is available yet (it may not exist, or has never \
             delivered a non-HEARTBEAT_OK result).",
            name = context.source_pulse,
        ),
    }
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
            context_from: None,
            include_identity: None,
            tasks: vec![
                PulseTask {
                    name: "check_inbox".to_string(),
                    prompt: "Check for new emails.".to_string(),
                    ..PulseTask::default()
                },
                PulseTask {
                    name: "check_alerts".to_string(),
                    prompt: "Review alert dashboard.".to_string(),
                    ..PulseTask::default()
                },
            ],
        }
    }

    // ── build_pulse_execution tests ─────────────────────────────────────

    #[test]
    fn execution_no_agent_has_no_skill() {
        let pulse = sample_pulse();
        let spawn_event = build_pulse_execution(&pulse, None, None, &[]);
        assert_eq!(spawn_event.skill, None);
        assert_eq!(spawn_event.source_label, "pulse:email_check");
        assert!(spawn_event.prompt.contains("email_check"));
        assert!(spawn_event.prompt.contains(HEARTBEAT_OK));
        assert!(matches!(spawn_event.source, EventTrigger::Pulse));
        assert_eq!(spawn_event.model, residuum_config::ModelSelector::SMALL);
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
        let spawn_event = build_pulse_execution(&pulse, None, None, &[]);
        assert_eq!(
            spawn_event.skill.as_ref().map(AsRef::as_ref),
            Some("memory-agent")
        );
        assert_eq!(spawn_event.source_label, "pulse:email_check");
        assert!(matches!(spawn_event.source, EventTrigger::Pulse));
        // A pulse that names no tier runs small, whether or not it
        // names a skill — the tier is the pulse's own setting.
        assert_eq!(spawn_event.model, residuum_config::ModelSelector::SMALL);
    }

    #[test]
    fn execution_runs_the_pulse_model_tier_or_explicit_model() {
        let mut pulse = sample_pulse();
        pulse.model_tier = Some(residuum_config::ModelSelector::LARGE);
        assert_eq!(
            build_pulse_execution(&pulse, None, None, &[]).model,
            residuum_config::ModelSelector::LARGE
        );

        let explicit = residuum_config::ModelSelector::Explicit("local/qwen3:8b".to_string());
        pulse.model_tier = Some(explicit.clone());
        assert_eq!(
            build_pulse_execution(&pulse, None, None, &[]).model,
            explicit
        );
    }

    #[test]
    fn prompt_teaches_both_sentinels() {
        let prompt = build_pulse_prompt(&sample_pulse(), None, &[]);
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
        let prompt = build_pulse_prompt(&pulse, None, &[]);

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
        let prompt = build_pulse_prompt(&pulse, None, &[]);

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
        let prompt = build_pulse_prompt(&sample_pulse(), None, &[]);
        assert!(
            !prompt.contains("schedule further background work"),
            "scheduled sessions may spawn within the depth cap like any other session"
        );
    }

    #[test]
    fn prompt_ends_with_heartbeat_ok_instruction() {
        let pulse = sample_pulse();
        let prompt = build_pulse_prompt(&pulse, None, &[]);

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
            context_from: None,
            include_identity: None,
            tasks: vec![],
        };
        let spawn_event = build_pulse_execution(&pulse, None, None, &[]);
        assert_eq!(spawn_event.source_label, "pulse:empty");
        assert!(
            spawn_event.prompt.contains(HEARTBEAT_OK),
            "should still have HEARTBEAT_OK instruction"
        );
    }

    // ── context_from injection ──────────────────────────────────────────

    #[test]
    fn no_context_means_no_context_section() {
        let prompt = build_pulse_prompt(&sample_pulse(), None, &[]);
        assert!(
            !prompt.contains("Context from"),
            "a pulse with no context_from should get no context section at all"
        );
    }

    #[test]
    fn context_with_output_is_injected_under_a_framing_header() {
        let context = PulseContext {
            source_pulse: "collector".to_string(),
            output: Some("Found 3 overnight alerts.".to_string()),
        };
        let prompt = build_pulse_prompt(&sample_pulse(), Some(&context), &[]);
        assert!(
            prompt.contains("collector"),
            "prompt should name the source pulse"
        );
        assert!(
            prompt.contains("Found 3 overnight alerts."),
            "prompt should contain the source pulse's stored output"
        );
    }

    #[test]
    fn missing_context_output_says_so_explicitly() {
        let context = PulseContext {
            source_pulse: "never_ran".to_string(),
            output: None,
        };
        let prompt = build_pulse_prompt(&sample_pulse(), Some(&context), &[]);
        assert!(
            prompt.contains("never_ran"),
            "prompt should name the missing source pulse"
        );
        assert!(
            prompt.contains("No prior output"),
            "prompt should say plainly that there is no prior output, not silently omit the section"
        );
    }

    #[test]
    fn build_pulse_execution_injects_context_into_the_spawned_prompt() {
        let pulse = sample_pulse();
        let context = PulseContext {
            source_pulse: "collector".to_string(),
            output: Some("Found 3 overnight alerts.".to_string()),
        };
        let spawn_event = build_pulse_execution(&pulse, None, Some(&context), &[]);
        assert!(spawn_event.prompt.contains("collector"));
        assert!(spawn_event.prompt.contains("Found 3 overnight alerts."));
    }

    // ── task scripts ────────────────────────────────────────────────────

    #[test]
    fn script_output_is_appended_to_its_own_tasks_prompt() {
        let pulse = sample_pulse();
        let scripts = vec![
            None,
            Some(ScriptRun::Succeeded {
                stdout: "2 alerts firing\n".to_string(),
            }),
        ];
        let prompt = build_pulse_execution(&pulse, None, None, &scripts).prompt;
        let alerts = prompt.find("### check_alerts").unwrap();
        let inbox = prompt.find("### check_inbox").unwrap();
        let output = prompt.find("2 alerts firing").unwrap();
        assert!(
            output > alerts && alerts > inbox,
            "the output belongs to check_alerts' section: {prompt}"
        );
        let task_prompt = prompt.find("Review alert dashboard.").unwrap();
        assert!(output > task_prompt, "the output follows the task's prompt");
        assert!(
            prompt.contains(HEARTBEAT_OK),
            "the session still gets its sentinels"
        );
    }

    #[test]
    fn a_failed_script_is_noted_in_the_prompt() {
        let pulse = sample_pulse();
        let scripts = vec![Some(ScriptRun::Failed(
            crate::pulse::script::ScriptFailure {
                script: "check-mail".to_string(),
                cause: crate::pulse::script::ScriptFailureCause::TimedOut { secs: 60 },
                stdout: String::new(),
                stderr_tail: "connection reset".to_string(),
            },
        ))];
        let prompt = build_pulse_prompt(&pulse, None, &scripts);
        assert!(prompt.contains("script failed"), "{prompt}");
        assert!(prompt.contains("60-second limit"), "{prompt}");
        assert!(prompt.contains("connection reset"), "{prompt}");
    }

    #[test]
    fn a_task_with_only_a_script_gets_its_output_under_its_name() {
        let mut pulse = sample_pulse();
        pulse.tasks = vec![PulseTask {
            name: "disk".to_string(),
            script: Some("df -h".to_string()),
            ..PulseTask::default()
        }];
        let scripts = vec![Some(ScriptRun::Succeeded {
            stdout: "/ 91%".to_string(),
        })];
        let prompt = build_pulse_prompt(&pulse, None, &scripts);
        assert!(
            prompt.contains("### disk\n\nOutput of this task's script"),
            "{prompt}"
        );
        assert!(prompt.contains("/ 91%"));
    }
}

//! Pulse execution handling and scheduling in the event loop.

use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::bus::{
    AgentResultEvent, AgentResultStatus, BusError, PulseOverlap, ResultDisposition, topics,
};
use crate::gateway::helpers::publish_notice;
use crate::gateway::types::AgentRuntime;
use crate::pulse::executor::PulseContext;
use crate::pulse::scheduler::PulseScheduler;
use crate::pulse::types::PulseDef;

/// Fork a session for a single due pulse.
#[tracing::instrument(skip_all)]
pub async fn handle_pulse_execution(
    spawn_event: crate::bus::SpawnRequestEvent,
    rt: &mut AgentRuntime,
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

/// Build the plain-language outcome of this tick's `HEARTBEAT.yml` reload,
/// for delivery to the agent whose own write caused it (see
/// `ConfigWriteWatch`). Independent of the deduped, change-only user notice
/// `take_problem_notice` returns — the agent asked about this specific
/// write, not "is there anything new to tell the owner".
fn heartbeat_reload_outcome(scheduler: &crate::pulse::scheduler::PulseScheduler) -> String {
    if let Some(err) = scheduler.last_parse_error() {
        return format!(
            "HEARTBEAT.yml failed to parse, your previous pulses are still running: {err}"
        );
    }
    let problems = scheduler.current_problems();
    if problems.is_empty() {
        "HEARTBEAT.yml reloaded cleanly — no problems found".to_string()
    } else {
        crate::pulse::types::heartbeat_problems_notice(problems)
    }
}

/// Resolve a pulse's `context_from` target to a [`PulseContext`], if it sets
/// one — looking up the named pulse's last delivered output in `scheduler`.
/// `output` is `None` when the named pulse has never delivered one (it
/// doesn't exist, has never fired, or has never completed with anything but
/// `HEARTBEAT_OK`); `build_pulse_prompt` turns that into an explicit "no
/// prior output" note rather than silently omitting the section.
fn resolve_pulse_context(pulse: &PulseDef, scheduler: &PulseScheduler) -> Option<PulseContext> {
    let source_pulse = pulse.context_from.clone()?;
    let output = scheduler.last_output(&source_pulse).map(str::to_string);
    Some(PulseContext {
        source_pulse,
        output,
    })
}

/// Extract `(pulse_name, summary)` from a pulse result eligible to become
/// that pulse's `last_output`, or `None` if it isn't. Split out as a pure
/// function (rather than inlined into [`handle_pulse_result_event`]) so the
/// eligibility rule is testable without a full `AgentRuntime`.
///
/// Only a `Completed` run with a non-empty summary whose disposition isn't
/// `Silent` (a `HEARTBEAT_OK` result) counts as "delivered" — a failed or
/// cancelled run never updates the stored output. A result whose
/// `source_label` doesn't start with `pulse:` (a spawned session, an action,
/// a webhook) isn't a pulse result and is ignored.
fn delivered_pulse_output(event: &AgentResultEvent) -> Option<(&str, &str)> {
    let pulse_name = event.source_label.strip_prefix("pulse:")?;
    if !matches!(event.status, AgentResultStatus::Completed) {
        return None;
    }
    if matches!(event.disposition, ResultDisposition::Silent) || event.summary.is_empty() {
        return None;
    }
    Some((pulse_name, event.summary.as_str()))
}

/// Record a completed pulse run's delivered output into `pulse_scheduler`,
/// for a later `context_from` pulse to inject at fire time — see
/// [`delivered_pulse_output`] for which results count as "delivered".
/// `Ok(None)` (subscriber closed) and `Err` (lagged/mismatched) are dropped
/// silently, same as the gateway's other best-effort bus subscriptions (see
/// `error_subscriber` in `run_event_loop`) — this is a secondary enrichment
/// of pulse state, not something worth failing the loop over.
pub fn handle_pulse_result_event(
    event: Result<Option<AgentResultEvent>, BusError>,
    rt: &mut AgentRuntime,
) {
    let Ok(Some(event)) = event else {
        return;
    };
    if let Some((pulse_name, summary)) = delivered_pulse_output(&event) {
        rt.pulse_scheduler
            .record_pulse_output(pulse_name, summary.to_string());
    }
}

/// Process all due pulses.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn handle_pulse_tick(rt: &mut AgentRuntime) {
    use crate::pulse::executor::build_pulse_execution;
    use crate::time;

    // Consumed once, up front: whether this tick is the first one after the
    // agent's own write to HEARTBEAT.yml — see `ConfigWriteWatch`. Unlike
    // config.toml/mcp.json, HEARTBEAT.yml has no discrete `ReloadSignal`; it
    // hot-reloads on every scheduler tick, so "the reload this write
    // triggered" is simply the very next tick.
    let deliver_to_agent = rt
        .config_reload_tracker
        .take_if_matches(crate::tools::config_reload_tracker::ConfigReloadKind::Heartbeat);

    let now = time::now_local(rt.tz);
    let due = rt
        .pulse_scheduler
        .due_pulses(now, &rt.layout.heartbeat_yml());
    if deliver_to_agent {
        let outcome = heartbeat_reload_outcome(&rt.pulse_scheduler);
        rt.agent.inject_system_message(outcome);
    }
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
        let context = resolve_pulse_context(pulse, &rt.pulse_scheduler);
        let spawn_event = build_pulse_execution(pulse, overlap, context.as_ref());
        handle_pulse_execution(spawn_event, rt).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::scheduler::PulseScheduler;

    #[test]
    fn a_clean_heartbeat_reports_reloaded_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "pulses: []\n").unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);

        assert_eq!(
            heartbeat_reload_outcome(&scheduler),
            "HEARTBEAT.yml reloaded cleanly — no problems found"
        );
    }

    #[test]
    fn a_heartbeat_with_problems_reports_them_regardless_of_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        let yaml = "pulses:\n  - name: dup\n    schedule: \"1h\"\n    tasks: []\n  - name: dup\n    schedule: \"2h\"\n    tasks: []\n";
        std::fs::write(&path, yaml).unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);
        // Drain the deduped owner notice — the agent-facing outcome must not
        // depend on whether anything is left for the owner to be told.
        let _notice = scheduler.take_problem_notice();

        let outcome = heartbeat_reload_outcome(&scheduler);
        assert!(
            outcome.contains("dup"),
            "outcome should name the problem even with no pending owner notice: {outcome}"
        );
    }

    #[test]
    fn an_unparseable_heartbeat_reports_the_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(&path, "not: valid: yaml: [[[").unwrap();

        let mut scheduler = PulseScheduler::new();
        let _due = scheduler.due_pulses(chrono::Local::now().naive_local(), &path);

        let outcome = heartbeat_reload_outcome(&scheduler);
        assert!(
            outcome.contains("failed to parse"),
            "outcome should say the file failed to parse: {outcome}"
        );
    }

    // ── context_from resolution ────────────────────────────────────────

    fn sample_pulse_def(name: &str, context_from: Option<&str>) -> crate::pulse::types::PulseDef {
        crate::pulse::types::PulseDef {
            name: name.to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            context_from: context_from.map(str::to_string),
            include_identity: None,
            tasks: vec![],
        }
    }

    #[test]
    fn resolve_pulse_context_none_when_pulse_sets_no_context_from() {
        let pulse = sample_pulse_def("downstream", None);
        let scheduler = PulseScheduler::new();
        assert!(resolve_pulse_context(&pulse, &scheduler).is_none());
    }

    #[test]
    fn resolve_pulse_context_finds_recorded_output() {
        let pulse = sample_pulse_def("downstream", Some("collector"));
        let mut scheduler = PulseScheduler::new();
        scheduler.record_pulse_output("collector", "found 3 items".to_string());
        let context = resolve_pulse_context(&pulse, &scheduler).unwrap();
        assert_eq!(context.source_pulse, "collector");
        assert_eq!(context.output.as_deref(), Some("found 3 items"));
    }

    #[test]
    fn resolve_pulse_context_missing_source_gives_none_output_not_none_context() {
        let pulse = sample_pulse_def("downstream", Some("never_ran"));
        let scheduler = PulseScheduler::new();
        let context = resolve_pulse_context(&pulse, &scheduler).unwrap();
        assert_eq!(context.source_pulse, "never_ran");
        assert_eq!(context.output, None);
    }

    // ── delivered_pulse_output eligibility ───────────────────────────────

    fn sample_result_event(
        source_label: &str,
        status: AgentResultStatus,
        disposition: ResultDisposition,
        summary: &str,
    ) -> AgentResultEvent {
        AgentResultEvent {
            session_address: crate::bus::SessionAddress::from("scheduled-test-0001"),
            run_id: "t1".into(),
            source_label: source_label.to_string(),
            agent_skill: None,
            source: crate::bus::EventTrigger::Pulse,
            disposition,
            status,
            summary: summary.to_string(),
            transcript_path: None,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        }
    }

    #[test]
    fn delivered_pulse_output_accepts_a_normal_completed_pulse_result() {
        let event = sample_result_event(
            "pulse:collector",
            AgentResultStatus::Completed,
            ResultDisposition::Normal,
            "found 3 items",
        );
        assert_eq!(
            delivered_pulse_output(&event),
            Some(("collector", "found 3 items"))
        );
    }

    #[test]
    fn delivered_pulse_output_accepts_urgent_disposition() {
        let event = sample_result_event(
            "pulse:collector",
            AgentResultStatus::Completed,
            ResultDisposition::Urgent,
            "urgent finding",
        );
        assert_eq!(
            delivered_pulse_output(&event),
            Some(("collector", "urgent finding"))
        );
    }

    #[test]
    fn delivered_pulse_output_rejects_silent_heartbeat_ok() {
        let event = sample_result_event(
            "pulse:collector",
            AgentResultStatus::Completed,
            ResultDisposition::Silent,
            "HEARTBEAT_OK",
        );
        assert_eq!(delivered_pulse_output(&event), None);
    }

    #[test]
    fn delivered_pulse_output_rejects_failed_runs() {
        let event = sample_result_event(
            "pulse:collector",
            AgentResultStatus::Failed {
                error: "boom".to_string(),
                details: None,
            },
            ResultDisposition::Normal,
            "partial output before failing",
        );
        assert_eq!(delivered_pulse_output(&event), None);
    }

    #[test]
    fn delivered_pulse_output_rejects_cancelled_runs() {
        let event = sample_result_event(
            "pulse:collector",
            AgentResultStatus::Cancelled,
            ResultDisposition::Normal,
            "",
        );
        assert_eq!(delivered_pulse_output(&event), None);
    }

    #[test]
    fn delivered_pulse_output_ignores_non_pulse_source_labels() {
        let event = sample_result_event(
            "action:deploy",
            AgentResultStatus::Completed,
            ResultDisposition::Normal,
            "deployed",
        );
        assert_eq!(delivered_pulse_output(&event), None);
    }
}

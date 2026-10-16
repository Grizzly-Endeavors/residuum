//! Pulse execution handling and scheduling in the event loop.

use tokio_util::sync::CancellationToken;

use crate::background::registry::SessionRegistry;
use crate::bus::session::SessionCategory;
use crate::bus::{
    AgentResultEvent, AgentResultStatus, BusError, EventTrigger, NotificationEvent, Publisher,
    PulseOverlap, ResultDisposition, SpawnRequestEvent, topics,
};
use crate::gateway::helpers::publish_notice;
use crate::gateway::types::AgentRuntime;
use crate::pulse::executor::{PulseContext, build_pulse_execution};
use crate::pulse::scheduler::PulseScheduler;
use crate::pulse::script::{ScriptEnv, ScriptRun, run_pulse_scripts, script_only_result};
use crate::pulse::types::PulseDef;

/// Publish a due pulse's spawn request, which forks its session.
async fn publish_pulse_spawn(publisher: &Publisher, spawn_event: SpawnRequestEvent) {
    let source_label = spawn_event.source_label.clone();
    if let Err(e) = publisher.publish(topics::Background, spawn_event).await {
        tracing::warn!(pulse = %source_label, error = %e, "failed to publish pulse spawn request");
    }
}

/// What a pulse with task scripts needs to run them and deliver what they
/// produced, away from the event loop.
#[derive(Clone)]
struct ScriptedPulseDeps {
    publisher: Publisher,
    env: ScriptEnv,
    tz: chrono_tz::Tz,
    /// Cancelled when the agent stops (`AgentRuntime::pulse_scripts_stop`).
    stop: CancellationToken,
}

impl ScriptedPulseDeps {
    fn from_runtime(rt: &AgentRuntime) -> Self {
        Self {
            publisher: rt.publisher.clone(),
            env: ScriptEnv {
                workspace_root: rt.layout.root().to_path_buf(),
                tools_path: std::sync::Arc::clone(&rt.tools_path),
                agent_keys: std::sync::Arc::clone(&rt.agent_keys),
            },
            tz: rt.tz,
            stop: rt.pulse_scripts_stop.clone(),
        }
    }
}

/// Fire a pulse whose tasks run scripts: run them in task order, then
/// either deliver a `script_only` pulse's result straight to the
/// notification router, or start the pulse's session with each script's
/// output in its task's prompt.
///
/// Runs on its own task (see [`handle_pulse_tick`]) so a slow script never
/// holds up the agent's event loop. In a pulse that runs a session, a
/// failed script is reported to the owner here (see
/// [`report_script_failures`]) and the session still runs, told the script
/// failed; a `script_only` pulse's failures travel in its result instead.
/// Delivers nothing when the agent stops mid-script.
async fn run_scripted_pulse(
    pulse: PulseDef,
    overlap: Option<PulseOverlap>,
    context: Option<PulseContext>,
    deps: ScriptedPulseDeps,
) {
    let Some(runs) = run_pulse_scripts(&pulse, &deps.env, &deps.stop).await else {
        return;
    };
    deliver_scripted_pulse(&pulse, &runs, overlap, context.as_ref(), &deps).await;
}

/// Deliver what a scripted pulse's finished scripts produced: a
/// `script_only` pulse's result, or the session spawn (after reporting any
/// failed script). Delivers nothing once the agent has begun stopping, so a
/// script that finished during shutdown never starts a session or files a
/// result into an agent that is going away.
async fn deliver_scripted_pulse(
    pulse: &PulseDef,
    runs: &[Option<ScriptRun>],
    overlap: Option<PulseOverlap>,
    context: Option<&PulseContext>,
    deps: &ScriptedPulseDeps,
) {
    if deps.stop.is_cancelled() {
        tracing::info!(
            pulse = %pulse.name,
            "pulse scripts finished while the agent is stopping; delivering nothing for this fire"
        );
        return;
    }
    if pulse.is_script_only() {
        let event = script_only_result(pulse, runs, deps.tz);
        tracing::debug!(
            pulse = %pulse.name,
            disposition = ?event.disposition,
            status = %event.status,
            "script-only pulse finished"
        );
        if let Err(e) = deps.publisher.publish(topics::Background, event).await {
            tracing::warn!(pulse = %pulse.name, error = %e, "failed to publish script-only pulse result");
        }
        return;
    }
    report_script_failures(pulse, runs, deps).await;
    let spawn_event = build_pulse_execution(pulse, overlap, context, runs);
    publish_pulse_spawn(&deps.publisher, spawn_event).await;
}

/// Tell the owner about each failed script in a pulse that still runs its
/// session, on the same two surfaces the notification router uses for a
/// failed pulse: an inbox item carrying the command, how it ended and the
/// tail of its stderr, and an owner notice naming the pulse, the task and
/// the reason.
async fn report_script_failures(
    pulse: &PulseDef,
    runs: &[Option<ScriptRun>],
    deps: &ScriptedPulseDeps,
) {
    for (task, run) in pulse.tasks.iter().zip(runs) {
        let Some(ScriptRun::Failed(failure)) = run else {
            continue;
        };
        let message = format!(
            "The script for task '{}' in pulse '{}' {}. The pulse still ran its check, without \
             that script's output.",
            task.name,
            pulse.name,
            failure.reason()
        );
        let content = format!("{message}\n\n{}", failure.details());
        let item = NotificationEvent {
            title: format!("pulse:{}", pulse.name),
            content,
            source: EventTrigger::Pulse,
            urgent: false,
            timestamp: crate::time::now_local(deps.tz),
        };
        if let Err(e) = deps.publisher.publish(topics::Inbox, item).await {
            tracing::warn!(pulse = %pulse.name, task = %task.name, error = %e, "failed to file pulse script failure to the inbox");
        }
        publish_notice(&deps.publisher, message).await;
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
///
/// A pulse whose tasks run scripts fires on its own task (see
/// [`run_scripted_pulse`]), so the loop never waits on a script; every
/// other pulse starts its session straight away.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn handle_pulse_tick(rt: &mut AgentRuntime) {
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
    let due = {
        let models = rt.spawn_context.model_catalog.read().await;
        rt.pulse_scheduler
            .due_pulses(now, &rt.layout.heartbeat_yml(), &models)
    };
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
        if pulse.has_scripts() {
            let deps = ScriptedPulseDeps::from_runtime(rt);
            crate::util::spawn_in_span(run_scripted_pulse(pulse.clone(), overlap, context, deps));
        } else {
            let spawn_event = build_pulse_execution(pulse, overlap, context.as_ref(), &[]);
            publish_pulse_spawn(&rt.publisher, spawn_event).await;
        }
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
        let _due = scheduler.due_pulses(
            chrono::Local::now().naive_local(),
            &path,
            &crate::config::ModelCatalog::default(),
        );

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
        let _due = scheduler.due_pulses(
            chrono::Local::now().naive_local(),
            &path,
            &crate::config::ModelCatalog::default(),
        );
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
        let _due = scheduler.due_pulses(
            chrono::Local::now().naive_local(),
            &path,
            &crate::config::ModelCatalog::default(),
        );

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

    // ── scripted pulses ──────────────────────────────────────────────────

    use crate::bus::{BusHandle, NotifyName, SYSTEM_CHANNEL, Subscriber};
    use crate::pulse::types::PulseTask;
    use crate::testing::bus_wait;

    fn scripted_deps(bus: &BusHandle, dir: &std::path::Path) -> ScriptedPulseDeps {
        ScriptedPulseDeps {
            publisher: bus.publisher(),
            env: crate::pulse::script::tests::env(dir),
            tz: chrono_tz::UTC,
            stop: CancellationToken::new(),
        }
    }

    fn scripted_pulse(script: &str, script_only: bool) -> PulseDef {
        let mut pulse = sample_pulse_def("watch", None);
        pulse.tasks = vec![PulseTask {
            name: "check".to_string(),
            prompt: if script_only {
                String::new()
            } else {
                "Say whether anything changed.".to_string()
            },
            script: Some(script.to_string()),
            script_only,
            ..PulseTask::default()
        }];
        pulse
    }

    /// A broker with the notification router running on it, as in a live
    /// agent, so a script-only result is followed all the way to the inbox.
    async fn bus_with_router() -> BusHandle {
        let bus = crate::bus::spawn_broker();
        let router = crate::notify::router::spawn_notification_router(
            &bus,
            crate::bus::EndpointRegistry::from_entries([]),
            bus.publisher(),
        )
        .await;
        assert!(router.is_some(), "the router should subscribe");
        bus
    }

    // `exit 0` with no output works the same under `sh -c` and `cmd /C`.
    const SILENT_SCRIPT: &str = "exit 0";

    #[tokio::test]
    async fn script_only_pulse_with_empty_output_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();

        run_scripted_pulse(
            scripted_pulse(SILENT_SCRIPT, true),
            None,
            None,
            scripted_deps(&bus, dir.path()),
        )
        .await;

        let result = bus_wait::next_event("the script-only result", &mut results).await;
        assert_eq!(result.disposition, ResultDisposition::Silent);
        assert!(matches!(result.status, AgentResultStatus::Completed));
        assert_eq!(result.source_label, "pulse:watch");
        bus_wait::bus_barrier(&bus).await;
        assert!(
            spawns.drain().is_empty(),
            "a script-only pulse never starts a session"
        );
    }

    #[tokio::test]
    async fn script_only_pulse_output_is_routed_to_the_inbox_as_printed() {
        let dir = tempfile::tempdir().unwrap();
        let bus = bus_with_router().await;
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();

        run_scripted_pulse(
            scripted_pulse("echo site changed", true),
            None,
            None,
            scripted_deps(&bus, dir.path()),
        )
        .await;

        let item = bus_wait::next_event("the inbox item", &mut inbox).await;
        assert_eq!(item.title, "pulse:watch");
        assert_eq!(item.content.trim(), "site changed");
        assert!(!item.urgent, "no urgency unless the output asks for it");
    }

    #[tokio::test]
    async fn script_only_pulse_failure_reaches_the_inbox_and_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let bus = bus_with_router().await;
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let mut notices: Subscriber<crate::bus::NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        run_scripted_pulse(
            scripted_pulse("echo feed unreachable 1>&2 && exit 4", true),
            None,
            None,
            scripted_deps(&bus, dir.path()),
        )
        .await;

        let item = bus_wait::next_event("the inbox item", &mut inbox).await;
        assert!(
            item.content.contains("exited with code 4"),
            "{}",
            item.content
        );
        assert!(
            item.content.contains("feed unreachable"),
            "{}",
            item.content
        );
        let notice = bus_wait::next_event("the failed-pulse notice", &mut notices).await;
        assert!(notice.message.contains("watch"), "{}", notice.message);
        assert!(
            notice.message.contains("exited with code 4"),
            "{}",
            notice.message
        );
    }

    #[tokio::test]
    async fn prompt_mode_appends_script_output_to_the_session_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();

        run_scripted_pulse(
            scripted_pulse("echo 3 new posts", false),
            None,
            None,
            scripted_deps(&bus, dir.path()),
        )
        .await;

        let spawn = bus_wait::next_event("the pulse's spawn request", &mut spawns).await;
        assert_eq!(spawn.source_label, "pulse:watch");
        assert!(spawn.prompt.contains("Say whether anything changed."));
        assert!(spawn.prompt.contains("3 new posts"), "{}", spawn.prompt);
    }

    #[tokio::test]
    async fn prompt_mode_failure_is_reported_and_the_session_still_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let mut notices: Subscriber<crate::bus::NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        run_scripted_pulse(
            scripted_pulse("echo token expired 1>&2 && exit 2", false),
            None,
            None,
            scripted_deps(&bus, dir.path()),
        )
        .await;

        let item = bus_wait::next_event("the inbox item", &mut inbox).await;
        assert!(item.content.contains("'check'"), "{}", item.content);
        assert!(
            item.content.contains("exited with code 2"),
            "{}",
            item.content
        );
        assert!(item.content.contains("exit code: 2"), "{}", item.content);
        assert!(item.content.contains("token expired"), "{}", item.content);
        let notice = bus_wait::next_event("the owner notice", &mut notices).await;
        assert!(notice.message.contains("'watch'"), "{}", notice.message);
        assert!(
            notice.message.contains("exited with code 2"),
            "{}",
            notice.message
        );
        let spawn = bus_wait::next_event("the pulse's spawn request", &mut spawns).await;
        assert!(spawn.prompt.contains("script failed"), "{}", spawn.prompt);
    }

    #[tokio::test]
    async fn a_stopped_agent_delivers_nothing_for_a_scripted_pulse() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let deps = scripted_deps(&bus, dir.path());
        deps.stop.cancel();

        run_scripted_pulse(scripted_pulse("echo hi", true), None, None, deps.clone()).await;
        run_scripted_pulse(scripted_pulse("echo hi", false), None, None, deps).await;

        bus_wait::bus_barrier(&bus).await;
        assert!(results.drain().is_empty());
        assert!(spawns.drain().is_empty());
    }

    #[tokio::test]
    async fn scripts_that_finish_after_the_agent_starts_stopping_deliver_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let deps = scripted_deps(&bus, dir.path());
        // The scripts have already run; the stop lands before delivery.
        let output = Some(ScriptRun::Succeeded {
            stdout: "changed".to_string(),
        });
        let failed = Some(ScriptRun::Failed(crate::pulse::script::ScriptFailure {
            script: "check".to_string(),
            cause: crate::pulse::script::ScriptFailureCause::Exited { code: Some(1) },
            stdout: String::new(),
            stderr_tail: String::new(),
        }));
        deps.stop.cancel();

        let script_only = scripted_pulse("check", true);
        deliver_scripted_pulse(&script_only, &[output], None, None, &deps).await;
        let with_session = scripted_pulse("check", false);
        deliver_scripted_pulse(&with_session, &[failed], None, None, &deps).await;

        bus_wait::bus_barrier(&bus).await;
        assert!(results.drain().is_empty(), "no result is filed");
        assert!(spawns.drain().is_empty(), "no session is started");
        assert!(inbox.drain().is_empty(), "no failure is filed");
    }
}

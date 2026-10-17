//! One pulse fire, run on its own task: the pulse's task scripts, then
//! either a `script_only` pulse's result or the pulse's session, followed to
//! its end so the Schedule place can show the fire as running, in which
//! phase, and how it ended (see [`crate::pulse_fires`]).

use std::sync::Arc;

use chrono::Utc;

use crate::pulse_fires::{PulseFire, PulseFires};
use crate::types::AgentRuntime;
use residuum_background::registry::SessionRegistry;
use residuum_bus::notice::publish_notice;
use residuum_bus::{
    AgentResultStatus, BusHandle, EventTrigger, NotificationEvent, Publisher, PulseFireChange,
    PulseFireOutcome, PulseFirePhase, PulseFireStatus, SessionEvent, SessionEventKind,
    SessionStartFailedEvent, SpawnRequestEvent, Subscriber, topics,
};
use residuum_proactivity::pulse::executor::{PulseContext, build_pulse_execution};
use residuum_proactivity::pulse::script::{
    ScriptEnv, ScriptRun, run_pulse_scripts, script_only_result,
};
use residuum_proactivity::pulse::types::PulseDef;

/// What a pulse fire needs to run away from the event loop.
#[derive(Clone)]
pub(super) struct FireDeps {
    pub publisher: Publisher,
    pub bus: BusHandle,
    pub env: ScriptEnv,
    pub tz: chrono_tz::Tz,
    pub registry: Arc<SessionRegistry>,
    pub fires: Arc<PulseFires>,
}

impl FireDeps {
    pub(super) fn from_runtime(rt: &AgentRuntime) -> Self {
        Self {
            publisher: rt.publisher.clone(),
            bus: rt.bus_handle.clone(),
            env: ScriptEnv {
                workspace_root: rt.layout.root().to_path_buf(),
                tools_path: Arc::clone(&rt.tools_path),
                agent_keys: Arc::clone(&rt.agent_keys),
            },
            tz: rt.tz,
            registry: Arc::clone(&rt.session_registry),
            fires: Arc::clone(&rt.pulse_fires),
        }
    }
}

/// Register a fire of `pulse` and run it on its own task, so a slow script
/// or a long session never holds up the agent's event loop.
///
/// The fire is flagged when an earlier fire of the same pulse is still in
/// flight. That never blocks or skips it — the owner decided pulses that
/// overlap their own previous run start anyway — it only makes the overlap
/// visible.
pub(super) fn start_pulse_fire(pulse: PulseDef, context: Option<PulseContext>, deps: FireDeps) {
    let overlap = deps.fires.overlap(&pulse.name, &deps.registry);
    if let Some(ov) = &overlap {
        let still_going_secs = (Utc::now() - ov.previous_started_at).num_seconds().max(0);
        tracing::info!(
            pulse = %pulse.name,
            previous_fire = ov.previous_fire_id.as_deref().unwrap_or("none"),
            previous_run_id = ov.previous_run_id.as_deref().unwrap_or("none"),
            previous_running_for_secs = still_going_secs,
            "pulse fired again while its previous run is still going; starting the new run \
             anyway and flagging the overlap"
        );
    }
    let phase = if pulse.has_scripts() {
        PulseFirePhase::Script
    } else {
        PulseFirePhase::Session
    };
    let fire = deps.fires.begin(
        &pulse.name,
        phase,
        overlap.clone(),
        deps.publisher.clone(),
        Arc::clone(&deps.registry),
    );
    residuum_core::util::spawn_in_span(run_pulse_fire(pulse, overlap, context, deps, fire));
}

/// Run one fire: its task scripts in task order, then a `script_only`
/// pulse's result straight to the notification router, or the pulse's
/// session with each script's output in its task's prompt, followed until
/// it ends.
///
/// In a pulse that runs a session, a failed script is reported to the
/// owner here (see [`report_script_failures`]) and the session still runs,
/// told the script failed; a `script_only` pulse's failures travel in its
/// result instead. A fire the owner stops delivers nothing more and is
/// recorded as stopped; a fire the agent's stop catches delivers and
/// records nothing.
async fn run_pulse_fire(
    pulse: PulseDef,
    overlap: Option<residuum_bus::PulseOverlap>,
    context: Option<PulseContext>,
    deps: FireDeps,
    fire: PulseFire,
) {
    if !pulse.has_scripts() {
        let spawn = build_pulse_execution(&pulse, overlap, context.as_ref(), &[]);
        fire.request_session(spawn.address.clone());
        fire.announce(PulseFireChange::Started).await;
        run_session_phase(fire, spawn, &deps).await;
        return;
    }

    fire.announce(PulseFireChange::Started).await;
    let Some(runs) = run_pulse_scripts(&pulse, &deps.env, fire.stop_token()).await else {
        end_stopped_fire(fire).await;
        return;
    };
    deliver_scripted_fire(fire, &pulse, &runs, overlap, context.as_ref(), &deps).await;
}

/// Deliver what a fire's finished scripts produced: a `script_only`
/// pulse's result, or the session (after reporting any failed script).
///
/// A stop that landed after the scripts finished still keeps the fire from
/// delivering anything: a script that finished during shutdown never starts
/// a session or files a result into an agent that is going away, and a fire
/// the owner stopped stays stopped.
async fn deliver_scripted_fire(
    fire: PulseFire,
    pulse: &PulseDef,
    runs: &[Option<ScriptRun>],
    overlap: Option<residuum_bus::PulseOverlap>,
    context: Option<&PulseContext>,
    deps: &FireDeps,
) {
    if fire.stop_token().is_cancelled() {
        end_stopped_fire(fire).await;
        return;
    }
    if pulse.is_script_only() {
        deliver_script_only(fire, pulse, runs, deps).await;
        return;
    }
    report_script_failures(pulse, runs, deps).await;
    let spawn = build_pulse_execution(pulse, overlap, context, runs);
    fire.request_session(spawn.address.clone());
    fire.announce(PulseFireChange::SessionRequested).await;
    run_session_phase(fire, spawn, deps).await;
}

/// End a fire whose stop token fired before it delivered anything.
async fn end_stopped_fire(fire: PulseFire) {
    if fire.agent_stopping() {
        tracing::info!(
            pulse = %fire.pulse(),
            fire = %fire.id(),
            "the agent is stopping; delivering nothing for this pulse fire"
        );
        fire.end().await;
        return;
    }
    tracing::info!(pulse = %fire.pulse(), fire = %fire.id(), "pulse fire stopped by the owner before it delivered anything");
    fire.finish(
        PulseFireOutcome {
            status: PulseFireStatus::Stopped,
            at: Utc::now(),
            error: None,
        },
        None,
    )
    .await;
}

/// Deliver a `script_only` pulse's result to the notification router and
/// finish the fire with its outcome (a silent result included).
async fn deliver_script_only(
    fire: PulseFire,
    pulse: &PulseDef,
    runs: &[Option<ScriptRun>],
    deps: &FireDeps,
) {
    let event = script_only_result(pulse, runs, deps.tz);
    tracing::debug!(
        pulse = %pulse.name,
        disposition = ?event.disposition,
        status = %event.status,
        "script-only pulse finished"
    );
    let outcome = fire_outcome(&event.status);
    if let Err(e) = deps.publisher.publish(topics::Background, event).await {
        tracing::warn!(pulse = %pulse.name, error = %e, "failed to publish script-only pulse result");
    }
    fire.finish(outcome, None).await;
}

/// A fire's outcome from the status of the run that ended it.
fn fire_outcome(status: &AgentResultStatus) -> PulseFireOutcome {
    let (status, error) = match status {
        AgentResultStatus::Completed => (PulseFireStatus::Ok, None),
        AgentResultStatus::Cancelled => (PulseFireStatus::Stopped, None),
        AgentResultStatus::Failed { error, .. } => (PulseFireStatus::Failed, Some(error.clone())),
    };
    PulseFireOutcome {
        status,
        at: Utc::now(),
        error,
    }
}

/// Ask for the fire's session and follow it until it ends: until the
/// session completes, or the spawn listener says it couldn't start.
///
/// The fire offers its stop token to the session it asks for (see
/// [`residuum_background::fire_stops`]), so a stop stops that session
/// wherever it is: one that lands before the spawn listener forks it means
/// it is never forked, and one that lands later has already cancelled the
/// session's own stop token, a child of the fire's.
async fn run_session_phase(fire: PulseFire, mut spawn: SpawnRequestEvent, deps: &FireDeps) {
    let address = spawn.address.clone();
    let pulse = spawn.source_label.clone();
    // Subscribed before the spawn is published, so neither the session's
    // start nor its end can be missed.
    let subscriptions = async {
        let sessions: Subscriber<SessionEvent> = deps.bus.subscribe(topics::Sessions).await?;
        let failures: Subscriber<SessionStartFailedEvent> =
            deps.bus.subscribe(topics::Sessions).await?;
        Ok::<_, residuum_bus::BusError>((sessions, failures))
    }
    .await;
    // A stop that landed while the fire was subscribing (the owner's, or the
    // agent's during shutdown) keeps the session from being asked for at all.
    if fire.stop_token().is_cancelled() {
        end_stopped_fire(fire).await;
        return;
    }
    let (mut sessions, mut failures) = match subscriptions {
        Ok(subscriptions) => subscriptions,
        Err(e) => {
            tracing::warn!(pulse = %pulse, error = %e, "couldn't follow a pulse fire's session; it won't show as running");
            publish_pulse_spawn(&deps.publisher, spawn).await;
            fire.end().await;
            return;
        }
    };
    spawn.fire_id = Some(fire.id().to_string());
    deps.registry
        .fire_stops()
        .offer(fire.id(), fire.stop_token().clone());
    if !publish_pulse_spawn(&deps.publisher, spawn).await {
        end_unrequested_fire(fire).await;
        return;
    }

    let mut run_id: Option<String> = None;
    let mut stop_requested = false;
    loop {
        tokio::select! {
            () = fire.stop_token().cancelled(), if !stop_requested => {
                stop_requested = true;
                if !deps.registry.fire_stops().withdraw(fire.id()) {
                    // The listener hadn't forked the session, and now never
                    // will.
                    end_stopped_fire(fire).await;
                    return;
                }
                tracing::info!(pulse = %pulse, fire = %fire.id(), "stopping a pulse fire's session");
            }
            event = sessions.recv() => match event {
                Ok(Some(event)) if event.address == address => match event.kind {
                    SessionEventKind::Started(_) if run_id.is_none() => {
                        fire.session_registered(event.run_id.clone());
                        run_id = Some(event.run_id);
                        fire.announce(PulseFireChange::SessionStarted).await;
                    }
                    // A run id the fire doesn't know yet is still its own:
                    // a request whose address was already live is delivered
                    // into that run instead of starting a new one.
                    SessionEventKind::Completed { status, .. }
                        if run_id.as_ref().is_none_or(|id| *id == event.run_id) =>
                    {
                        fire.finish(fire_outcome(&status), Some(event.run_id)).await;
                        return;
                    }
                    SessionEventKind::Started(_)
                    | SessionEventKind::Completed { .. }
                    | SessionEventKind::StateChanged(_)
                    | SessionEventKind::TurnStarted { .. }
                    | SessionEventKind::TurnEnded { .. }
                    | SessionEventKind::ToolCall(_)
                    | SessionEventKind::ToolResult(_)
                    | SessionEventKind::Intermediate { .. }
                    | SessionEventKind::TurnUsage { .. }
                    | SessionEventKind::Response { .. }
                    | SessionEventKind::Error { .. } => {}
                },
                Ok(Some(_)) | Err(_) => {}
                Ok(None) => {
                    fire.end().await;
                    return;
                }
            },
            failed = failures.recv() => match failed {
                Ok(Some(failed)) if failed.address == address => {
                    end_unstarted_fire(fire, failed, stop_requested).await;
                    return;
                }
                Ok(Some(_)) | Err(_) => {}
                Ok(None) => {
                    fire.end().await;
                    return;
                }
            },
        }
    }
}

/// End a fire whose session the spawn listener couldn't fork: as stopped
/// when the fire had been stopped by then, else as failed with the
/// listener's plain-language reason, the full cause going to the log.
async fn end_unstarted_fire(fire: PulseFire, failed: SessionStartFailedEvent, stopped: bool) {
    if stopped {
        end_stopped_fire(fire).await;
        return;
    }
    tracing::warn!(
        pulse = %fire.pulse(),
        fire = %fire.id(),
        error = failed.error_details.as_deref().unwrap_or(&failed.error),
        "a pulse fire's session couldn't start"
    );
    fire.finish(
        PulseFireOutcome {
            status: PulseFireStatus::Failed,
            at: Utc::now(),
            error: Some(failed.error),
        },
        None,
    )
    .await;
}

/// Publish a due pulse's spawn request, which forks its session. Returns
/// whether it was published.
async fn publish_pulse_spawn(publisher: &Publisher, spawn_event: SpawnRequestEvent) -> bool {
    let source_label = spawn_event.source_label.clone();
    match publisher.publish(topics::Background, spawn_event).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(pulse = %source_label, error = %e, "failed to publish pulse spawn request");
            false
        }
    }
}

/// End a fire whose spawn request couldn't be published, so no session will
/// ever start for it: as failed, unless the agent is stopping, which is
/// usually why the bus refused it.
async fn end_unrequested_fire(fire: PulseFire) {
    if fire.agent_stopping() {
        fire.end().await;
        return;
    }
    let error = format!(
        "The pulse \"{}\" couldn't ask for its session. It tries again at its next run; if it \
         keeps failing, the agent's logs have the details.",
        fire.pulse()
    );
    fire.finish(
        PulseFireOutcome {
            status: PulseFireStatus::Failed,
            at: Utc::now(),
            error: Some(error),
        },
        None,
    )
    .await;
}

/// Tell the owner about each failed script in a pulse that still runs its
/// session, on the same two surfaces the notification router uses for a
/// failed pulse: an inbox item carrying the command, how it ended and the
/// tail of its stderr, and an owner notice naming the pulse, the task and
/// the reason.
async fn report_script_failures(pulse: &PulseDef, runs: &[Option<ScriptRun>], deps: &FireDeps) {
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
            timestamp: residuum_core::time::now_local(deps.tz),
        };
        if let Err(e) = deps.publisher.publish(topics::Inbox, item).await {
            tracing::warn!(pulse = %pulse.name, task = %task.name, error = %e, "failed to file pulse script failure to the inbox");
        }
        publish_notice(&deps.publisher, message).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::session::{MAIN_DEPTH, SessionCategory, SessionInfo, SessionState};
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_bus::{
        AgentResultEvent, NoticeEvent, NotifyName, PulseFireEvent, ResultDisposition,
        SYSTEM_CHANNEL, SessionAddress,
    };
    use residuum_core::testing::wait;
    use residuum_proactivity::pulse::types::PulseTask;
    use tokio_util::sync::CancellationToken;

    fn fire_deps(bus: &BusHandle, dir: &std::path::Path) -> FireDeps {
        FireDeps {
            publisher: bus.publisher(),
            bus: bus.clone(),
            env: residuum_proactivity::pulse::testing::script_env(dir),
            tz: chrono_tz::UTC,
            registry: Arc::new(SessionRegistry::new()),
            fires: Arc::new(PulseFires::default()),
        }
    }

    fn plain_pulse() -> PulseDef {
        PulseDef {
            name: "watch".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: vec![PulseTask {
                name: "check".to_string(),
                prompt: "Say whether anything changed.".to_string(),
                ..PulseTask::default()
            }],
        }
    }

    fn scripted_pulse(script: &str, script_only: bool) -> PulseDef {
        let mut pulse = plain_pulse();
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

    /// A script that marks `started` once it runs, then runs until it is
    /// killed.
    fn blocking_script(started: &std::path::Path) -> String {
        if cfg!(windows) {
            format!(
                "echo x> \"{}\" && powershell -NoProfile -Command \"Start-Sleep -Seconds 600\"",
                started.display()
            )
        } else {
            format!("touch '{}'; sleep 600", started.display())
        }
    }

    fn begin(deps: &FireDeps, pulse: &PulseDef) -> PulseFire {
        let phase = if pulse.has_scripts() {
            PulseFirePhase::Script
        } else {
            PulseFirePhase::Session
        };
        let overlap = deps.fires.overlap(&pulse.name, &deps.registry);
        deps.fires.begin(
            &pulse.name,
            phase,
            overlap,
            deps.publisher.clone(),
            Arc::clone(&deps.registry),
        )
    }

    /// Run one fire of `pulse` to its end.
    async fn run_fire(pulse: PulseDef, deps: &FireDeps) {
        let fire = begin(deps, &pulse);
        run_pulse_fire(pulse, None, None, deps.clone(), fire).await;
    }

    async fn fire_events(bus: &BusHandle) -> Subscriber<PulseFireEvent> {
        bus.subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap()
    }

    /// The next change the fire publishes that isn't its start.
    async fn next_after_start(events: &mut Subscriber<PulseFireEvent>) -> PulseFireEvent {
        let started = next_event("the fire's start", events).await;
        assert_eq!(started.change, PulseFireChange::Started);
        next_event("the fire's next change", events).await
    }

    /// The next change the fire publishes that `matches`.
    async fn next_change_where(
        events: &mut Subscriber<PulseFireEvent>,
        matches: impl Fn(&PulseFireChange) -> bool,
    ) -> PulseFireEvent {
        loop {
            let event = next_event("the fire's changes", events).await;
            if matches(&event.change) {
                return event;
            }
        }
    }

    fn finished_outcome(event: &PulseFireEvent) -> (PulseFireOutcome, Option<String>) {
        let PulseFireChange::Finished {
            outcome,
            session_run_id,
        } = &event.change
        else {
            panic!("expected the fire to finish, got {event:?}");
        };
        (outcome.clone(), session_run_id.clone())
    }

    fn live_session(address: &SessionAddress, run_id: &str) -> SessionInfo {
        SessionInfo {
            address: address.clone(),
            run_id: run_id.to_string(),
            category: SessionCategory::Scheduled,
            trigger: EventTrigger::Pulse,
            source_label: "pulse:watch".to_string(),
            state: SessionState::Forking,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "check".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::SMALL,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    /// Stand in for the spawn listener: claim the stop token the fire
    /// offered for `spawn`'s session, as `fork_and_spawn` does. `None` when
    /// the listener would fork nothing.
    fn claim_session_stop(deps: &FireDeps, spawn: &SpawnRequestEvent) -> Option<CancellationToken> {
        let fire_id = spawn.fire_id.as_deref().expect("a fire's request names it");
        deps.registry.fire_stops().claim(fire_id)
    }

    /// Stand in for the session runtime: register the session at `address`
    /// with `stop` as its stop token and announce its start, as
    /// `SessionRuntime::spawn` does.
    async fn register_session(
        deps: &FireDeps,
        address: &SessionAddress,
        run_id: &str,
        stop: CancellationToken,
    ) {
        let info = live_session(address, run_id);
        deps.registry.register(info.clone(), stop).unwrap();
        deps.publisher
            .publish(
                topics::Sessions,
                SessionEvent {
                    address: address.clone(),
                    run_id: run_id.to_string(),
                    kind: SessionEventKind::Started(Box::new(info)),
                },
            )
            .await
            .unwrap();
    }

    /// Stand in for the spawn listener and the session runtime together:
    /// fork `spawn`'s session with the stop token claimed from its fire, and
    /// announce its start. Returns the session's stop token.
    async fn start_session(
        deps: &FireDeps,
        spawn: &SpawnRequestEvent,
        run_id: &str,
    ) -> CancellationToken {
        let stop = claim_session_stop(deps, spawn).expect("the fire is still going");
        register_session(deps, &spawn.address, run_id, stop.clone()).await;
        stop
    }

    /// End the session at `address` with `status`, as the session runtime
    /// does: out of the registry, then `Completed`.
    async fn complete_session(
        deps: &FireDeps,
        address: &SessionAddress,
        run_id: &str,
        status: AgentResultStatus,
    ) {
        deps.registry.remove(address, run_id);
        deps.publisher
            .publish(
                topics::Sessions,
                SessionEvent {
                    address: address.clone(),
                    run_id: run_id.to_string(),
                    kind: SessionEventKind::Completed {
                        status,
                        episode_id: None,
                    },
                },
            )
            .await
            .unwrap();
    }

    /// A broker with the notification router running on it, as in a live
    /// agent, so a script-only result is followed all the way to the inbox.
    async fn bus_with_router() -> BusHandle {
        let bus = residuum_bus::spawn_broker();
        let router = residuum_notify::router::spawn_notification_router(
            &bus,
            residuum_bus::EndpointRegistry::from_entries([]),
            bus.publisher(),
        )
        .await;
        assert!(router.is_some(), "the router should subscribe");
        bus
    }

    // `exit 0` with no output works the same under `sh -c` and `cmd /C`.
    const SILENT_SCRIPT: &str = "exit 0";

    // ── script-only fires ────────────────────────────────────────────────

    #[tokio::test]
    async fn a_silent_script_only_fire_finishes_ok_without_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        run_fire(scripted_pulse(SILENT_SCRIPT, true), &deps).await;

        let result = next_event("the script-only result", &mut results).await;
        assert_eq!(result.disposition, ResultDisposition::Silent);
        assert!(matches!(result.status, AgentResultStatus::Completed));
        assert_eq!(result.source_label, "pulse:watch");
        let finished = next_after_start(&mut events).await;
        let (outcome, session_run_id) = finished_outcome(&finished);
        assert_eq!(outcome.status, PulseFireStatus::Ok);
        assert_eq!(outcome.error, None);
        assert_eq!(
            session_run_id, None,
            "the event loop records a script-only fire"
        );
        assert_eq!(
            finished.current_run, None,
            "nothing of the pulse is running"
        );
        bus_barrier(&bus).await;
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

        run_fire(
            scripted_pulse("echo site changed", true),
            &fire_deps(&bus, dir.path()),
        )
        .await;

        let item = next_event("the inbox item", &mut inbox).await;
        assert_eq!(item.title, "pulse:watch");
        assert_eq!(item.content.trim(), "site changed");
        assert!(!item.urgent, "no urgency unless the output asks for it");
    }

    #[tokio::test]
    async fn a_failed_script_only_fire_reaches_the_inbox_and_the_owner_and_records_why() {
        let dir = tempfile::tempdir().unwrap();
        let bus = bus_with_router().await;
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let mut notices: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut events = fire_events(&bus).await;

        run_fire(
            scripted_pulse("echo feed unreachable 1>&2 && exit 4", true),
            &fire_deps(&bus, dir.path()),
        )
        .await;

        let item = next_event("the inbox item", &mut inbox).await;
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
        let notice = next_event("the failed-pulse notice", &mut notices).await;
        assert!(notice.message.contains("watch"), "{}", notice.message);
        assert!(
            notice.message.contains("exited with code 4"),
            "{}",
            notice.message
        );
        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Failed);
        assert_eq!(
            outcome.error.as_deref(),
            Some("the script for task 'check' exited with code 4")
        );
        assert_eq!(session_run_id, None);
    }

    // ── fires that run a session ─────────────────────────────────────────

    #[tokio::test]
    async fn a_fire_moves_from_its_scripts_to_its_session_and_finishes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());
        let started = dir.path().join("started");
        let release = dir.path().join("release");
        let script = if cfg!(windows) {
            format!(
                "echo x> \"{}\" && powershell -NoProfile -Command \"while (-not (Test-Path '{}')) {{ Start-Sleep -Milliseconds 20 }}; 'fresh news'\"",
                started.display(),
                release.display()
            )
        } else {
            format!(
                "touch '{}'; while [ ! -e '{}' ]; do sleep 0.02; done; echo fresh news",
                started.display(),
                release.display()
            )
        };

        let pulse = scripted_pulse(&script, false);
        let fire = begin(&deps, &pulse);
        let fire_id = fire.id().to_string();
        let task = tokio::spawn(run_pulse_fire(pulse, None, None, deps.clone(), fire));

        let start = next_event("the fire's start", &mut events).await;
        assert_eq!(start.change, PulseFireChange::Started);
        let at_start = start.current_run.expect("the fire is running");
        assert_eq!(at_start.fire_id.as_deref(), Some(fire_id.as_str()));
        assert_eq!(at_start.phase, PulseFirePhase::Script);
        wait::until_true("the script to start", || started.exists()).await;
        let while_scripting = deps.fires.current_run("watch", &deps.registry).unwrap();
        assert_eq!(
            while_scripting.phase,
            PulseFirePhase::Script,
            "still in its scripts"
        );
        std::fs::write(&release, "").unwrap();

        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        assert!(spawn.prompt.contains("fresh news"), "{}", spawn.prompt);
        let requested = next_event("the move to the session", &mut events).await;
        assert_eq!(requested.change, PulseFireChange::SessionRequested);
        let in_session = requested.current_run.unwrap();
        assert_eq!(in_session.phase, PulseFirePhase::Session);
        assert_eq!(in_session.address.as_ref(), Some(&spawn.address));
        assert_eq!(in_session.run_id, None);

        let _stop = start_session(&deps, &spawn, "run-1").await;
        let registered = next_event("the session's start", &mut events).await;
        assert_eq!(registered.change, PulseFireChange::SessionStarted);
        assert_eq!(
            registered.current_run.unwrap().run_id.as_deref(),
            Some("run-1")
        );

        complete_session(
            &deps,
            &spawn.address,
            "run-1",
            AgentResultStatus::Failed {
                error: "the model call timed out".to_string(),
                details: None,
            },
        )
        .await;
        let finished = next_event("the fire's finish", &mut events).await;
        let (outcome, session_run_id) = finished_outcome(&finished);
        assert_eq!(outcome.status, PulseFireStatus::Failed);
        assert_eq!(outcome.error.as_deref(), Some("the model call timed out"));
        assert_eq!(
            session_run_id.as_deref(),
            Some("run-1"),
            "the session store records this one"
        );
        assert_eq!(finished.current_run, None);
        task.await.unwrap();
        assert_eq!(deps.fires.current_run("watch", &deps.registry), None);
    }

    #[tokio::test]
    async fn a_pulse_without_scripts_starts_in_its_session_phase() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let task = tokio::spawn({
            let deps = deps.clone();
            async move { run_fire(plain_pulse(), &deps).await }
        });

        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        let start = next_event("the fire's start", &mut events).await;
        assert_eq!(start.change, PulseFireChange::Started);
        let run = start.current_run.unwrap();
        assert_eq!(run.phase, PulseFirePhase::Session);
        assert_eq!(run.address.as_ref(), Some(&spawn.address));

        start_session(&deps, &spawn, "run-1").await;
        complete_session(&deps, &spawn.address, "run-1", AgentResultStatus::Completed).await;
        let finished = next_change_where(&mut events, |change| {
            matches!(change, PulseFireChange::Finished { .. })
        })
        .await;
        let (outcome, session_run_id) = finished_outcome(&finished);
        assert_eq!(outcome.status, PulseFireStatus::Ok);
        assert_eq!(session_run_id.as_deref(), Some("run-1"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_session_that_cannot_start_fails_the_fire() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let task = tokio::spawn({
            let deps = deps.clone();
            async move { run_fire(plain_pulse(), &deps).await }
        });
        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        deps.publisher
            .publish(
                topics::Sessions,
                SessionStartFailedEvent {
                    address: spawn.address,
                    error: "The pulse \"watch\" couldn't start its session because the skill \
                            \"ghost\" couldn't be loaded. Check that the skill is still installed."
                        .to_string(),
                    error_details: Some(
                        "failed to activate skill 'ghost' for sub-agent: not found".to_string(),
                    ),
                    owner_start: false,
                },
            )
            .await
            .unwrap();

        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Failed);
        assert_eq!(
            outcome.error.as_deref(),
            Some(
                "The pulse \"watch\" couldn't start its session because the skill \"ghost\" \
                 couldn't be loaded. Check that the skill is still installed."
            ),
            "the plain sentence, as the listener words it"
        );
        assert_eq!(
            session_run_id, None,
            "no session run records it, so the event loop does"
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_spawn_request_that_cannot_be_published_fails_the_fire() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut events = fire_events(&bus).await;
        let deps = FireDeps {
            publisher: Publisher::noop(),
            ..fire_deps(&bus, dir.path())
        };
        let pulse = plain_pulse();
        // The fire's own changes still reach the bus; only the spawn request
        // is refused.
        let fire = deps.fires.begin(
            &pulse.name,
            PulseFirePhase::Session,
            None,
            bus.publisher(),
            Arc::clone(&deps.registry),
        );
        let fire_id = fire.id().to_string();

        run_pulse_fire(pulse, None, None, deps.clone(), fire).await;

        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Failed);
        assert_eq!(
            outcome.error.as_deref(),
            Some(
                "The pulse \"watch\" couldn't ask for its session. It tries again at its next \
                 run; if it keeps failing, the agent's logs have the details."
            )
        );
        assert_eq!(session_run_id, None);
        assert!(
            deps.registry.fire_stops().claim(&fire_id).is_none(),
            "the ended fire withdrew its stop token"
        );
    }

    #[tokio::test]
    async fn prompt_mode_failure_is_reported_and_the_session_still_runs() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let mut notices: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let deps = fire_deps(&bus, dir.path());

        let task = tokio::spawn({
            let deps = deps.clone();
            async move {
                run_fire(
                    scripted_pulse("echo token expired 1>&2 && exit 2", false),
                    &deps,
                )
                .await;
            }
        });

        let item = next_event("the inbox item", &mut inbox).await;
        assert!(item.content.contains("'check'"), "{}", item.content);
        assert!(
            item.content.contains("exited with code 2"),
            "{}",
            item.content
        );
        assert!(item.content.contains("exit code: 2"), "{}", item.content);
        assert!(item.content.contains("token expired"), "{}", item.content);
        let notice = next_event("the owner notice", &mut notices).await;
        assert!(notice.message.contains("'watch'"), "{}", notice.message);
        assert!(
            notice.message.contains("exited with code 2"),
            "{}",
            notice.message
        );
        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        assert!(spawn.prompt.contains("script failed"), "{}", spawn.prompt);
        task.abort();
    }

    // ── stopping a fire ──────────────────────────────────────────────────

    #[tokio::test]
    async fn stopping_a_fire_in_its_scripts_kills_the_script_and_starts_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());
        let started = dir.path().join("started");

        let pulse = scripted_pulse(&blocking_script(&started), false);
        let fire = begin(&deps, &pulse);
        let fire_id = fire.id().to_string();
        let task = tokio::spawn(run_pulse_fire(pulse, None, None, deps.clone(), fire));
        wait::until_true("the script to start", || started.exists()).await;

        assert!(deps.fires.stop("watch", &fire_id));

        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Stopped);
        assert_eq!(session_run_id, None);
        task.await.unwrap();
        bus_barrier(&bus).await;
        assert!(
            spawns.drain().is_empty(),
            "a stopped fire starts no session"
        );
        assert!(!deps.fires.stop("watch", &fire_id), "the fire is gone");
    }

    #[tokio::test]
    async fn stopping_a_fire_in_its_session_stops_that_session() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let pulse = plain_pulse();
        let fire = begin(&deps, &pulse);
        let fire_id = fire.id().to_string();
        let task = tokio::spawn(run_pulse_fire(pulse, None, None, deps.clone(), fire));
        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        let session_stop = start_session(&deps, &spawn, "run-1").await;
        next_change_where(&mut events, |change| {
            *change == PulseFireChange::SessionStarted
        })
        .await;

        assert!(deps.fires.stop("watch", &fire_id));
        wait::guarded("the session to be stopped", session_stop.cancelled()).await;
        complete_session(&deps, &spawn.address, "run-1", AgentResultStatus::Cancelled).await;

        let finished = next_event("the fire's finish", &mut events).await;
        let (outcome, session_run_id) = finished_outcome(&finished);
        assert_eq!(outcome.status, PulseFireStatus::Stopped);
        assert_eq!(session_run_id.as_deref(), Some("run-1"));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_before_the_listener_forks_the_session_forks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let pulse = plain_pulse();
        let fire = begin(&deps, &pulse);
        let fire_id = fire.id().to_string();
        let task = tokio::spawn(run_pulse_fire(pulse, None, None, deps.clone(), fire));
        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        assert_eq!(spawn.fire_id.as_deref(), Some(fire_id.as_str()));
        assert!(deps.fires.stop("watch", &fire_id));

        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Stopped);
        assert_eq!(
            session_run_id, None,
            "no session ran, so the event loop records it"
        );
        task.await.unwrap();
        assert!(
            claim_session_stop(&deps, &spawn).is_none(),
            "the listener forks nothing for a fire stopped before the fork"
        );
    }

    #[tokio::test]
    async fn a_stop_after_the_fork_has_cancelled_the_session_before_it_registers() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let pulse = plain_pulse();
        let fire = begin(&deps, &pulse);
        let fire_id = fire.id().to_string();
        let task = tokio::spawn(run_pulse_fire(pulse, None, None, deps.clone(), fire));
        let spawn = next_event("the pulse's spawn request", &mut spawns).await;
        let session_stop = claim_session_stop(&deps, &spawn).expect("the fire is still going");
        assert!(deps.fires.stop("watch", &fire_id));
        assert!(
            session_stop.is_cancelled(),
            "the session's stop token is cancelled before it registers"
        );

        register_session(&deps, &spawn.address, "run-1", session_stop).await;
        complete_session(&deps, &spawn.address, "run-1", AgentResultStatus::Cancelled).await;
        let finished = next_change_where(&mut events, |change| {
            matches!(change, PulseFireChange::Finished { .. })
        })
        .await;
        let (outcome, session_run_id) = finished_outcome(&finished);
        assert_eq!(outcome.status, PulseFireStatus::Stopped);
        assert_eq!(
            session_run_id.as_deref(),
            Some("run-1"),
            "the session store records the cancelled run"
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_fire_stopped_before_it_asks_for_its_session_starts_none() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());

        let pulse = plain_pulse();
        let fire = begin(&deps, &pulse);
        assert!(deps.fires.stop("watch", fire.id()));
        run_pulse_fire(pulse, None, None, deps.clone(), fire).await;

        let (outcome, session_run_id) = finished_outcome(&next_after_start(&mut events).await);
        assert_eq!(outcome.status, PulseFireStatus::Stopped);
        assert_eq!(
            session_run_id, None,
            "no session ran, so the event loop records it"
        );
        bus_barrier(&bus).await;
        assert!(
            spawns.drain().is_empty(),
            "a fire stopped before its spawn request starts no session"
        );
    }

    #[tokio::test]
    async fn a_stopped_agent_delivers_nothing_for_any_pulse() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());
        deps.fires.stop_all();

        run_fire(scripted_pulse("echo hi", true), &deps).await;
        run_fire(scripted_pulse("echo hi", false), &deps).await;
        run_fire(plain_pulse(), &deps).await;

        for _ in 0..3 {
            let ended = next_after_start(&mut events).await;
            assert_eq!(ended.change, PulseFireChange::Ended, "nothing is recorded");
        }
        bus_barrier(&bus).await;
        assert!(results.drain().is_empty());
        assert!(spawns.drain().is_empty());
    }

    #[tokio::test]
    async fn scripts_that_finish_after_a_stop_deliver_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut results: Subscriber<AgentResultEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut inbox: Subscriber<NotificationEvent> = bus.subscribe(topics::Inbox).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());
        // The scripts have already run; the stop lands before delivery.
        let output = Some(ScriptRun::Succeeded {
            stdout: "changed".to_string(),
        });
        let failed = Some(ScriptRun::Failed(
            residuum_proactivity::pulse::script::ScriptFailure {
                script: "check".to_string(),
                cause: residuum_proactivity::pulse::script::ScriptFailureCause::Exited {
                    code: Some(1),
                },
                stdout: String::new(),
                stderr_tail: String::new(),
            },
        ));

        let script_only = scripted_pulse("check", true);
        let owner_stopped = begin(&deps, &script_only);
        assert!(
            deps.fires.stop("watch", owner_stopped.id()),
            "the owner's stop"
        );
        deliver_scripted_fire(owner_stopped, &script_only, &[output], None, None, &deps).await;
        let stopped = next_event("the stopped fire", &mut events).await;
        assert_eq!(
            finished_outcome(&stopped).0.status,
            PulseFireStatus::Stopped
        );

        deps.fires.stop_all();
        let with_session = scripted_pulse("check", false);
        let agent_stopped = begin(&deps, &with_session);
        deliver_scripted_fire(agent_stopped, &with_session, &[failed], None, None, &deps).await;
        let ended = next_event("the fire the agent's stop caught", &mut events).await;
        assert_eq!(ended.change, PulseFireChange::Ended);

        bus_barrier(&bus).await;
        assert!(results.drain().is_empty(), "no result is filed");
        assert!(spawns.drain().is_empty(), "no session is started");
        assert!(inbox.drain().is_empty(), "no failure is filed");
    }

    // ── overlap ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn two_fires_overlapping_in_their_scripts_flag_the_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let bus = residuum_bus::spawn_broker();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus.subscribe(topics::Background).await.unwrap();
        let mut events = fire_events(&bus).await;
        let deps = fire_deps(&bus, dir.path());
        let first_started = dir.path().join("first");
        let mut first_pulse = scripted_pulse(&blocking_script(&first_started), false);
        for task in &mut first_pulse.tasks {
            task.script_timeout_secs = Some(600);
        }

        start_pulse_fire(first_pulse, None, deps.clone());
        wait::until_true("the first fire's script to start", || {
            first_started.exists()
        })
        .await;
        let first_id = deps
            .fires
            .current_run("watch", &deps.registry)
            .and_then(|run| run.fire_id)
            .unwrap();

        start_pulse_fire(scripted_pulse("echo second", false), None, deps.clone());
        let spawn = next_event("the second fire's spawn request", &mut spawns).await;
        let overlap = spawn.overlap.expect("the second fire overlaps the first");
        assert_eq!(overlap.previous_fire_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(
            overlap.previous_run_id, None,
            "the first is still in its scripts"
        );

        // Both fires end before the test does, so the first one's script
        // (and the `sleep` it started) has been killed by then.
        deps.fires.stop_all();
        for _ in 0..2 {
            next_change_where(&mut events, |change| *change == PulseFireChange::Ended).await;
        }
        assert_eq!(deps.fires.current_run("watch", &deps.registry), None);
    }
}

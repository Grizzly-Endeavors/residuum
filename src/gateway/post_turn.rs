//! Background memory and post-turn work: the observer/reflector cycle (the
//! automatic one and the `/observe` and `/reflect` commands) and the
//! end-of-turn subconscious evaluation, run off the event loop so their LLM
//! calls never block the next inbound message, a shutdown signal, or a stop
//! request from being handled.
//!
//! Each kind (memory, subconscious) runs through its own worker: at most
//! one cycle in flight at a time, and any trigger that arrives while one is
//! running collapses into exactly one follow-up run rather than stacking —
//! see [`Coalescer`]. The slow part (the LLM call, plus any file persistence
//! that doesn't need `Agent`) runs entirely in the background task. Whatever
//! *does* need `&mut Agent` — reloading observations/recent-context after an
//! observe, or injecting a subconscious note — can't be touched off the
//! event loop, so it's reported back over a result channel and applied on
//! the main loop the next time it drains one (see
//! `crate::gateway::event_loop::run_loop`'s `post_turn_result_rx` branch).
//! That's also why memory can be "one step stale": a turn that starts before
//! a still-in-flight cycle's result is applied sees the pre-cycle context.
//!
//! Each trigger call carries a fresh snapshot of whatever `Arc`/`Clone`
//! runtime state the cycle needs (observer, merge writer, subconscious,
//! learning state, ...), taken from `AgentRuntime` at the moment of the
//! call. A coalesced run always uses the *latest* trigger's snapshot, so a
//! reload that swaps a component in place between two coalesced triggers is
//! still picked up — nothing here holds a stale clone from before the swap.
//!
//! Shutdown cancels both workers: an observation still waiting on its
//! extraction call, or a subconscious evaluation still waiting on its model,
//! is dropped with nothing written. An observation already merging its
//! episode gets a bounded grace period to finish its (atomic) writes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::bus::origin::MessageOrigin;
use crate::bus::{MessageEvent, PostTurnActivityEvent, PostTurnActivityKind, Publisher, topics};
use crate::gateway::memory::{self, MemorySubsystems};
use crate::inference::Message;
use crate::subconscious::{
    EvalPhase, Finding, LearnSignal, LearningState, Severity, Subconscious, TurnScratch,
};

/// What idle transition step to run once a background observe cycle
/// triggered by [`MemoryWorker::trigger_for_idle`] finishes, carried back
/// on [`PostTurnResult::IdleObservationReady`].
pub(crate) struct IdleContinuation {
    pub timeout_mins: u64,
    pub total_skills: usize,
    /// Where agent-started turns go from the transition on, named in the
    /// continuity message.
    pub idle_endpoint: crate::bus::EndpointName,
}

/// What the main loop needs to apply from a finished background cycle.
pub(crate) enum PostTurnResult {
    /// A (non-idle) memory cycle produced a merge — rotate the agent's
    /// history and reload its observations/recent-context views.
    ObservationReady,
    /// A `/reflect` rewrote the observation log and nothing was merged —
    /// reload the agent's observations only.
    ReflectionReady,
    /// The idle-triggered observe cycle finished; `reload_needed` says
    /// whether it changed memory (an idle transition still runs
    /// its continuation either way — clearing messages, switching the
    /// interface, injecting the continuity note — since those don't depend
    /// on whether there was anything to observe).
    IdleObservationReady {
        reload_needed: bool,
        continuation: IdleContinuation,
    },
    /// A subconscious evaluation finished (successfully or not); each
    /// string is a note to inject into the agent's context for its next
    /// turn. Corrections were already published directly from the
    /// background task — see [`publish_correction_turn`] — since that only
    /// needs the publisher, not `&mut Agent`.
    SubconsciousNotes(Vec<String>),
}

/// Coalescing bookkeeping: whether a cycle is currently running, and
/// whether a trigger arrived during that run and so owes exactly one
/// follow-up. The running-or-not decision and the pending-or-not decision
/// are always read and written together under this one lock — including at
/// the tail of a cycle, where "is anyone else about to trigger me" and
/// "should I loop again" must agree, or a trigger that lands in the gap
/// between them would be silently lost.
struct RunState {
    running: bool,
    pending: bool,
    handle: Option<JoinHandle<()>>,
}

/// The run/coalesce state machine and shutdown handling both workers share.
struct Coalescer {
    state: Mutex<RunState>,
    shutdown: CancellationToken,
}

impl Coalescer {
    fn new() -> Self {
        Self {
            state: Mutex::new(RunState {
                running: false,
                pending: false,
                handle: None,
            }),
            shutdown: CancellationToken::new(),
        }
    }

    /// Record a trigger. Spawns a run loop through `spawn` when nothing is
    /// running; otherwise marks one follow-up as owed. After shutdown has
    /// begun, triggers are ignored.
    fn trigger(&self, spawn: impl FnOnce() -> JoinHandle<()>) {
        let mut state = lock(&self.state);
        if self.shutdown.is_cancelled() {
            return;
        }
        if state.running {
            state.pending = true;
            return;
        }
        state.running = true;
        state.pending = false;
        state.handle = Some(spawn());
    }

    /// Called by the run loop after each cycle: whether a trigger arrived
    /// during it and so owes one more cycle. Returning `false` marks the
    /// worker idle under the same lock, so a trigger racing this call
    /// either sees `running` and sets `pending` (answered `true` here) or
    /// sees idle and spawns a fresh loop — never neither.
    fn owes_another_cycle(&self) -> bool {
        let mut state = lock(&self.state);
        if state.pending && !self.shutdown.is_cancelled() {
            state.pending = false;
            return true;
        }
        state.pending = false;
        state.running = false;
        false
    }

    /// Cancel the in-flight cycle's cancellable phase, then wait up to
    /// `grace` for the rest of it; past that, abort it. Every write a cycle
    /// makes is atomic (temp file + rename), so an aborted cycle leaves what
    /// it already wrote intact and discards the rest.
    async fn shutdown(&self, grace: Duration, kind: &'static str) {
        self.shutdown.cancel();
        let handle = lock(&self.state).handle.take();
        let Some(mut handle) = handle else { return };
        if tokio::time::timeout(grace, &mut handle).await.is_err() {
            tracing::warn!(
                kind,
                grace_secs = grace.as_secs(),
                "post-turn background cycle still running past the shutdown grace period, aborting it"
            );
            handle.abort();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Tell the web UI a background post-turn cycle started or finished, for
/// the quiet "updating memory…" / "reviewing turn…" indicator.
async fn publish_activity(publisher: Option<&Publisher>, kind: PostTurnActivityKind, active: bool) {
    let Some(publisher) = publisher else { return };
    if let Err(e) = publisher
        .publish(
            topics::Notification(crate::bus::NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
            PostTurnActivityEvent { kind, active },
        )
        .await
    {
        tracing::debug!(error = %e, ?kind, active, "failed to publish post-turn activity signal");
    }
}

/// How a queued observation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObserveMode {
    /// A threshold crossing or idle transition: backs off after a failure
    /// and stays quiet unless a failure streak starts or clears.
    Automatic,
    /// `/observe`: always attempts and tells the user how it went.
    Forced,
}

/// What the next memory cycle runs, accumulated across every trigger that
/// coalesces into it.
#[derive(Default)]
struct QueuedMemoryWork {
    /// The snapshot the next cycle should run with — always the most recent
    /// trigger's, overwritten on every call regardless of coalescing.
    mem: Option<MemorySubsystems>,
    /// A forced observation outranks an automatic one queued with it.
    observe: Option<ObserveMode>,
    /// Whether `/reflect` asked for a reflection.
    reflect: bool,
}

/// Background worker for the main agent's memory cycle: automatic
/// observation, and the `/observe` and `/reflect` commands. Sharing one
/// worker means no two of them ever load, observe, and remove the same
/// recent messages twice, or show the busy indicator over each other.
pub(crate) struct MemoryWorker {
    coalescer: Coalescer,
    queued: Mutex<QueuedMemoryWork>,
    /// Set only by [`Self::trigger_for_idle`] and never cleared by a plain
    /// [`Self::trigger`] — an idle transition queued behind a plain trigger
    /// must still run its continuation once a cycle finally completes.
    idle_continuation: Mutex<Option<IdleContinuation>>,
    result_tx: mpsc::UnboundedSender<PostTurnResult>,
}

impl MemoryWorker {
    #[must_use]
    pub(crate) fn new(result_tx: mpsc::UnboundedSender<PostTurnResult>) -> Arc<Self> {
        Arc::new(Self {
            coalescer: Coalescer::new(),
            queued: Mutex::new(QueuedMemoryWork::default()),
            idle_continuation: Mutex::new(None),
            result_tx,
        })
    }

    /// Trigger a plain (non-idle) automatic observation cycle.
    pub(crate) fn trigger(self: &Arc<Self>, mem: MemorySubsystems) {
        self.queue(mem, |work| {
            work.observe.get_or_insert(ObserveMode::Automatic);
        });
    }

    /// Trigger an observation cycle that, once it finishes, should also run
    /// the idle transition's remaining steps (see `crate::gateway::idle`).
    pub(crate) fn trigger_for_idle(
        self: &Arc<Self>,
        mem: MemorySubsystems,
        continuation: IdleContinuation,
    ) {
        *lock(&self.idle_continuation) = Some(continuation);
        self.trigger(mem);
    }

    /// Queue `/observe`: an observation that ignores the automatic backoff
    /// and reports its outcome. Waits behind a cycle already running.
    pub(crate) fn force_observe(self: &Arc<Self>, mem: MemorySubsystems) {
        self.queue(mem, |work| work.observe = Some(ObserveMode::Forced));
    }

    /// Queue `/reflect`: compress the observation log regardless of its
    /// size, and report the outcome. Waits behind a cycle already running.
    pub(crate) fn force_reflect(self: &Arc<Self>, mem: MemorySubsystems) {
        self.queue(mem, |work| work.reflect = true);
    }

    fn queue(self: &Arc<Self>, mem: MemorySubsystems, add: impl FnOnce(&mut QueuedMemoryWork)) {
        {
            let mut queued = lock(&self.queued);
            queued.mem = Some(mem);
            add(&mut queued);
        }
        let this = Arc::clone(self);
        self.coalescer
            .trigger(|| crate::util::spawn_in_span(async move { this.run_loop().await }));
    }

    async fn run_loop(&self) {
        let publisher = lock(&self.queued).mem.as_ref().map(|m| m.publisher.clone());
        publish_activity(publisher.as_ref(), PostTurnActivityKind::Memory, true).await;
        loop {
            self.run_one_cycle().await;
            if !self.coalescer.owes_another_cycle() {
                break;
            }
        }
        publish_activity(publisher.as_ref(), PostTurnActivityKind::Memory, false).await;
    }

    async fn run_one_cycle(&self) {
        let (mem, observe, reflect) = {
            let mut queued = lock(&self.queued);
            // Every trigger sets this before spawning or marking a
            // follow-up, and only a cycle takes it, so a run loop always
            // finds one.
            let Some(mem) = queued.mem.take() else {
                return;
            };
            (
                mem,
                queued.observe.take(),
                std::mem::take(&mut queued.reflect),
            )
        };
        let shutdown = &self.coalescer.shutdown;
        let observed = match observe {
            Some(ObserveMode::Automatic) => memory::execute_observation(&mem, shutdown).await,
            Some(ObserveMode::Forced) => memory::execute_forced_observation(&mem, shutdown).await,
            None => false,
        };
        let reflected = reflect && memory::execute_forced_reflection(&mem, shutdown).await;
        let continuation = lock(&self.idle_continuation).take();
        let result = if let Some(continuation) = continuation {
            PostTurnResult::IdleObservationReady {
                reload_needed: observed || reflected,
                continuation,
            }
        } else if observed {
            PostTurnResult::ObservationReady
        } else if reflected {
            PostTurnResult::ReflectionReady
        } else {
            return;
        };
        if self.result_tx.send(result).is_err() {
            tracing::debug!(
                "post-turn result channel closed, dropping memory cycle result (shutting down)"
            );
        }
    }

    /// Cancel a cycle still waiting on its model call, give one already
    /// merging up to `grace` to finish, and ignore later triggers.
    pub(crate) async fn shutdown(&self, grace: Duration) {
        self.coalescer.shutdown(grace, "memory").await;
    }
}

/// Everything one subconscious evaluation cycle needs, snapshotted fresh
/// from `AgentRuntime` by the caller at trigger time.
pub(crate) struct SubconsciousTrigger {
    pub subconscious: Arc<Subconscious>,
    pub learning_state: Arc<Mutex<LearningState>>,
    pub publisher: Publisher,
    pub tz: chrono_tz::Tz,
    pub learning_cooldown: Duration,
    pub new_messages: Vec<Message>,
    pub correlation_id: String,
    pub scratch: TurnScratch,
}

impl SubconsciousTrigger {
    /// Fold a newer turn's trigger into this still-unevaluated one, so the
    /// follow-up evaluation covers both turns: their messages in order, and
    /// both turns' mid-turn corrections and queued notes. The newer
    /// trigger's runtime snapshot and correlation id win.
    fn absorb(&mut self, newer: Self) {
        let mut messages = std::mem::take(&mut self.new_messages);
        messages.extend(newer.new_messages);
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch
            .applied_corrections
            .extend(newer.scratch.applied_corrections);
        scratch.queued_notes.extend(newer.scratch.queued_notes);
        *self = Self {
            new_messages: messages,
            scratch,
            ..newer
        };
    }
}

/// Background worker for the end-of-turn subconscious evaluation.
pub(crate) struct SubconsciousWorker {
    coalescer: Coalescer,
    /// The turn(s) the next cycle evaluates. A trigger arriving while an
    /// earlier one is still waiting here is folded into it (see
    /// [`SubconsciousTrigger::absorb`]) rather than replacing it, so no
    /// turn goes unevaluated and no queued mid-turn note is dropped.
    latest: Mutex<Option<SubconsciousTrigger>>,
    result_tx: mpsc::UnboundedSender<PostTurnResult>,
}

impl SubconsciousWorker {
    #[must_use]
    pub(crate) fn new(result_tx: mpsc::UnboundedSender<PostTurnResult>) -> Arc<Self> {
        Arc::new(Self {
            coalescer: Coalescer::new(),
            latest: Mutex::new(None),
            result_tx,
        })
    }

    pub(crate) fn trigger(self: &Arc<Self>, payload: SubconsciousTrigger) {
        {
            let mut latest = lock(&self.latest);
            match latest.as_mut() {
                Some(waiting) => waiting.absorb(payload),
                None => *latest = Some(payload),
            }
        }
        let this = Arc::clone(self);
        self.coalescer
            .trigger(|| crate::util::spawn_in_span(async move { this.run_loop().await }));
    }

    async fn run_loop(&self) {
        let publisher = lock(&self.latest).as_ref().map(|t| t.publisher.clone());
        publish_activity(publisher.as_ref(), PostTurnActivityKind::Subconscious, true).await;
        loop {
            self.run_one_cycle().await;
            if !self.coalescer.owes_another_cycle() {
                break;
            }
        }
        publish_activity(
            publisher.as_ref(),
            PostTurnActivityKind::Subconscious,
            false,
        )
        .await;
    }

    async fn run_one_cycle(&self) {
        let Some(trigger) = lock(&self.latest).take() else {
            return;
        };
        // A turn with no assistant output (e.g. a hard error) has nothing
        // to judge.
        if trigger.new_messages.len() < 2 {
            return;
        }

        let evaluation = tokio::select! {
            biased;
            () = self.coalescer.shutdown.cancelled() => {
                tracing::debug!("shutting down, dropping in-flight subconscious evaluation");
                return;
            }
            evaluation = trigger.subconscious.evaluate(
                &trigger.new_messages,
                EvalPhase::EndOfTurn,
                Some(&trigger.scratch),
            ) => evaluation,
        };

        let notes = match evaluation {
            Ok(outcome) => {
                let mut notes = Vec::new();
                for delivery in plan_delivery(outcome.findings) {
                    match delivery {
                        Delivery::Note(instruction) => {
                            tracing::info!("subconscious note queued for next turn");
                            notes.push(instruction);
                        }
                        Delivery::Correction(instruction) => {
                            tracing::info!("subconscious act finding triggering correction turn");
                            publish_correction_turn(
                                &trigger.publisher,
                                trigger.tz,
                                &instruction,
                                &trigger.correlation_id,
                            )
                            .await;
                        }
                    }
                }
                maybe_spawn_learner(
                    &trigger.publisher,
                    &trigger.learning_state,
                    trigger.learning_cooldown,
                    &outcome.learnings,
                )
                .await;
                notes
            }
            Err(e) => {
                tracing::warn!(error = %e, "subconscious end-of-turn evaluation failed");
                // Triage is the only delivery path for queued mid-turn
                // notes, so a failed evaluation would silently drop them —
                // surface them raw instead of losing them.
                fallback_notes(&trigger.scratch)
            }
        };

        if !notes.is_empty()
            && self
                .result_tx
                .send(PostTurnResult::SubconsciousNotes(notes))
                .is_err()
        {
            tracing::debug!(
                "post-turn result channel closed, dropping subconscious notes (shutting down)"
            );
        }
    }

    /// Drop an evaluation still waiting on its model, give anything past
    /// that point up to `grace`, and ignore later triggers.
    pub(crate) async fn shutdown(&self, grace: Duration) {
        self.coalescer.shutdown(grace, "subconscious").await;
    }
}

/// A planned delivery for one finding, decided before touching the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Delivery {
    /// Inject as a passive system note for the next turn.
    Note(String),
    /// Trigger an immediate correction turn.
    Correction(String),
}

/// Decide how each finding is delivered.
///
/// The first `act` finding becomes a correction turn; every other finding
/// (including later `act` findings) degrades to a note, so a single turn never
/// spawns more than one correction.
fn plan_delivery(findings: Vec<Finding>) -> Vec<Delivery> {
    let mut plan = Vec::with_capacity(findings.len());
    let mut correction_used = false;
    for finding in findings {
        if finding.severity == Severity::Act && !correction_used {
            correction_used = true;
            plan.push(Delivery::Correction(finding.instruction));
        } else {
            plan.push(Delivery::Note(finding.instruction));
        }
    }
    plan
}

/// Queued mid-turn notes to deliver directly when the triage evaluation
/// failed — the lower-urgency findings the mid-turn watch observed but did
/// not steer on; without the triage pass to fold them in, they're injected
/// as-is so the agent still sees them next turn.
fn fallback_notes(prior: &TurnScratch) -> Vec<String> {
    if prior.queued_notes.is_empty() {
        return Vec::new();
    }
    tracing::info!(
        count = prior.queued_notes.len(),
        "delivering queued mid-turn notes after triage failure"
    );
    prior
        .queued_notes
        .iter()
        .map(|note| note.instruction.clone())
        .collect()
}

/// Build the background-origin `MessageEvent` that starts a correction turn.
///
/// The `background` origin endpoint is what keeps the correction turn from
/// being re-evaluated by the subconscious (the end-of-turn hook skips
/// background turns), bounding the feedback loop.
fn build_correction_event(
    instruction: &str,
    correlation_id: &str,
    timestamp: chrono::NaiveDateTime,
) -> MessageEvent {
    let content = format!(
        "[Subconscious] A background check of your last turn found a problem to correct now:\n\
         {instruction}\n\
         Take the corrective action (e.g. update a wiki page). Only message the user if they \
         need to know something new."
    );

    MessageEvent {
        id: format!("subconscious-{correlation_id}"),
        content,
        origin: MessageOrigin {
            endpoint: "background".to_string(),
            sender: None,
            conversation: None,
            agent_sender: None,
        },
        timestamp,
        images: vec![],
        context: None,
    }
}

/// Publish a correction turn onto the user-message topic. Needs only the
/// publisher and timezone, never `&mut Agent`, so it runs directly from the
/// background task rather than being deferred to the main loop.
async fn publish_correction_turn(
    publisher: &Publisher,
    tz: chrono_tz::Tz,
    instruction: &str,
    correlation_id: &str,
) {
    let msg_event = build_correction_event(instruction, correlation_id, crate::time::now_local(tz));
    if let Err(e) = publisher.publish(topics::UserMessage, msg_event).await {
        tracing::warn!(error = %e, "failed to publish subconscious correction turn");
    }
}

/// Spawn the `learner` sub-agent when the triage surfaced learnable signals.
///
/// Cooldown-gated via the shared `LearningState`; an empty signal list or a
/// live cooldown is a no-op. Decisions are logged inside `LearningState`.
async fn maybe_spawn_learner(
    publisher: &Publisher,
    learning_state: &Mutex<LearningState>,
    cooldown: Duration,
    learnings: &[LearnSignal],
) {
    let Some(spawn) =
        lock(learning_state).on_learn_signals(learnings, cooldown, std::time::Instant::now())
    else {
        return;
    };
    if let Err(e) = publisher.publish(topics::Background, spawn).await {
        tracing::warn!(error = %e, "failed to publish learner spawn request");
    } else {
        tracing::info!(
            signals = learnings.len(),
            "learner sub-agent spawn requested"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subconscious::FindingKind;

    fn finding(severity: Severity, instruction: &str) -> Finding {
        Finding {
            kind: FindingKind::Omission,
            severity,
            instruction: instruction.to_string(),
        }
    }

    #[test]
    fn only_first_act_becomes_a_correction() {
        let plan = plan_delivery(vec![
            finding(Severity::Note, "note one"),
            finding(Severity::Act, "first act"),
            finding(Severity::Act, "second act"),
        ]);

        assert_eq!(
            plan,
            vec![
                Delivery::Note("note one".to_string()),
                Delivery::Correction("first act".to_string()),
                Delivery::Note("second act".to_string()),
            ],
            "exactly one correction per turn; later acts degrade to notes"
        );
    }

    #[test]
    fn all_notes_produce_no_correction() {
        let plan = plan_delivery(vec![
            finding(Severity::Note, "a"),
            finding(Severity::Note, "b"),
        ]);
        assert!(
            plan.iter().all(|d| matches!(d, Delivery::Note(_))),
            "note-only findings never trigger a correction turn"
        );
    }

    #[test]
    fn empty_findings_produce_empty_plan() {
        let actual = &plan_delivery(vec![]);
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }

    #[test]
    fn correction_event_has_background_origin() {
        let ts = chrono::NaiveDate::from_ymd_opt(2026, 2, 22)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let event = build_correction_event("save the preference", "corr-123", ts);

        // The background origin is the loop-prevention guarantee: the end-of-turn
        // hook skips background turns, so this correction is never re-evaluated.
        assert_eq!(
            event.origin.endpoint, "background",
            "correction must use the background origin to avoid re-evaluation"
        );
        assert_eq!(event.id, "subconscious-corr-123");
        assert!(
            event.content.contains("save the preference"),
            "instruction must reach the agent"
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::bus::testing::next_event;
    use crate::bus::{NotifyName, SYSTEM_CHANNEL, Subscriber};
    use crate::config::WorkspaceLayout;
    use crate::inference::testing::model::GatedProvider;
    use crate::memory::recent_messages::{append_recent_messages, load_recent_messages};
    use crate::memory::types::Visibility;
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;

    const OBSERVER_RESPONSE: &str = r#"{
        "observations": [
            {"content": "the user likes short answers", "timestamp": "2026-02-21T14:30", "visibility": "user"}
        ],
        "narrative": ""
    }"#;

    struct ObserveHarness {
        _dir: tempfile::TempDir,
        layout: WorkspaceLayout,
        mem: MemorySubsystems,
        /// Holds each extraction call until the test releases it.
        gate: Gate,
        worker: Arc<MemoryWorker>,
        results: mpsc::UnboundedReceiver<PostTurnResult>,
        activity: Subscriber<PostTurnActivityEvent>,
    }

    async fn observe_harness() -> ObserveHarness {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        for d in layout.required_dirs() {
            tokio::fs::create_dir_all(&d).await.unwrap();
        }
        let gate = Gate::closed("observer");
        let observer = Arc::new(crate::memory::observer::Observer::new(
            Box::new(GatedProvider::new(&gate, OBSERVER_RESPONSE)),
            crate::memory::observer::ObserverConfig::default(),
        ));
        let search_index = Arc::new(
            crate::memory::search::MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap(),
        );
        let merge_writer = Arc::new(crate::memory::merge_writer::MemoryMergeWriter::new(
            crate::memory::reflector::Reflector::disabled(chrono_tz::UTC),
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let bus = crate::bus::spawn_broker();
        // Subscribed before any trigger, so no activity signal can be missed.
        let activity: Subscriber<PostTurnActivityEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mem = MemorySubsystems {
            observer,
            merge_writer,
            layout: layout.clone(),
            tz: chrono_tz::UTC,
            publisher: bus.publisher(),
        };
        let (result_tx, results) = mpsc::unbounded_channel();
        ObserveHarness {
            _dir: dir,
            layout,
            mem,
            gate,
            worker: MemoryWorker::new(result_tx),
            results,
            activity,
        }
    }

    async fn append_message(layout: &WorkspaceLayout, content: &str) {
        append_recent_messages(
            &layout.recent_messages_json(),
            &[Message::user(content)],
            Visibility::User,
            chrono_tz::UTC,
            None,
        )
        .await
        .unwrap();
    }

    /// Waits until the memory cycle's busy signal reaches `active`. Activity
    /// from the other post-turn cycle is skipped.
    async fn memory_activity_reaches(
        activity: &mut Subscriber<PostTurnActivityEvent>,
        active: bool,
    ) {
        loop {
            let event = next_event("a memory cycle activity signal", activity).await;
            if event.kind == PostTurnActivityKind::Memory && event.active == active {
                return;
            }
        }
    }

    #[test]
    fn coalescer_owes_exactly_one_follow_up_for_triggers_mid_cycle() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _enter = rt.enter();
        let coalescer = Coalescer::new();
        let spawned = AtomicUsize::new(0);
        let spawn = || {
            spawned.fetch_add(1, Ordering::SeqCst);
            crate::util::spawn_in_span(async {})
        };

        coalescer.trigger(spawn);
        coalescer.trigger(spawn);
        coalescer.trigger(spawn);
        assert_eq!(spawned.load(Ordering::SeqCst), 1, "one run loop at a time");
        assert!(
            coalescer.owes_another_cycle(),
            "two mid-cycle triggers owe one follow-up"
        );
        assert!(!coalescer.owes_another_cycle(), "and only one");

        coalescer.trigger(spawn);
        assert_eq!(
            spawned.load(Ordering::SeqCst),
            2,
            "a trigger after going idle starts a fresh run loop"
        );
    }

    #[tokio::test]
    async fn coalescer_ignores_triggers_after_shutdown() {
        let coalescer = Coalescer::new();
        coalescer.shutdown(Duration::from_secs(1), "test").await;
        let spawned = AtomicUsize::new(0);
        coalescer.trigger(|| {
            spawned.fetch_add(1, Ordering::SeqCst);
            crate::util::spawn_in_span(async {})
        });
        assert_eq!(spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn memory_worker_runs_off_loop_and_coalesces_into_one_follow_up() {
        let mut h = observe_harness().await;
        append_message(&h.layout, "first turn").await;

        // The trigger returns immediately while the cycle waits on its
        // extraction call: the work is off the caller's path.
        h.worker.trigger(h.mem.clone());
        h.gate.until_held(1).await;
        memory_activity_reaches(&mut h.activity, true).await;

        // Two more turns end while the first cycle is still extracting.
        append_message(&h.layout, "second turn").await;
        h.worker.trigger(h.mem.clone());
        h.worker.trigger(h.mem.clone());

        h.gate.open_all();
        for _ in 0..2 {
            let result = wait::next("an observation result", &mut h.results).await;
            assert!(matches!(result, PostTurnResult::ObservationReady));
        }
        // The busy signal goes idle only after the run loop has exited, so
        // the arrival count read below is final.
        memory_activity_reaches(&mut h.activity, false).await;
        assert_eq!(
            h.gate.arrived(),
            2,
            "three triggers make one cycle plus exactly one follow-up"
        );
        assert!(
            load_recent_messages(&h.layout.recent_messages_json())
                .await
                .unwrap()
                .is_empty(),
            "the follow-up observed the message the first cycle never saw"
        );
    }

    #[tokio::test]
    async fn memory_worker_keeps_messages_that_arrive_during_a_cycle() {
        let mut h = observe_harness().await;
        append_message(&h.layout, "observed").await;
        h.worker.trigger(h.mem.clone());
        h.gate.until_held(1).await;

        // A turn ends mid-cycle but its trigger hasn't fired yet (the
        // observe threshold wasn't crossed).
        append_message(&h.layout, "arrived mid-cycle").await;
        h.gate.release(1);
        wait::next("the observation result", &mut h.results).await;

        let remaining = load_recent_messages(&h.layout.recent_messages_json())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining.first().unwrap().message.content,
            "arrived mid-cycle"
        );
    }

    #[tokio::test]
    async fn shutdown_cancels_an_in_flight_extraction_without_writing() {
        let mut h = observe_harness().await;
        append_message(&h.layout, "unobserved").await;
        h.worker.trigger(h.mem.clone());
        h.gate.until_held(1).await;

        // The gate is never opened: only cancellation can end this cycle. The
        // grace period is longer than the hang guard, so a shutdown that waited
        // it out would fail the guard rather than return.
        wait::guarded(
            "shutdown to cancel the in-flight extraction",
            h.worker.shutdown(wait::HANG_GUARD * 2),
        )
        .await;

        assert!(
            h.results.try_recv().is_err(),
            "a cancelled cycle reports nothing"
        );
        assert_eq!(
            load_recent_messages(&h.layout.recent_messages_json())
                .await
                .unwrap()
                .len(),
            1,
            "a cancelled cycle leaves the unobserved messages for the next boot"
        );
        assert!(
            tokio::fs::read_dir(h.layout.episodes_dir())
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none(),
            "no episode was written"
        );

        // A trigger after shutdown decides synchronously and spawns nothing, so
        // the run state read right after it is final.
        h.worker.trigger(h.mem.clone());
        assert!(
            !lock(&h.worker.coalescer.state).running,
            "triggers after shutdown start nothing"
        );
        assert_eq!(h.gate.arrived(), 1, "triggers after shutdown start nothing");
    }

    #[tokio::test]
    async fn forced_observe_runs_in_the_background_despite_backoff() {
        let mut h = observe_harness().await;
        append_message(&h.layout, "hello").await;
        h.mem.observer.automatic_failure_tracker().record_failure();

        // Returns at once: the extraction waits at the gate in the worker,
        // off the caller's path, even though automatic cycles are backing off.
        h.worker.force_observe(h.mem.clone());
        h.gate.until_held(1).await;
        memory_activity_reaches(&mut h.activity, true).await;

        h.gate.release(1);
        let result = wait::next("the forced observation result", &mut h.results).await;
        assert!(matches!(result, PostTurnResult::ObservationReady));
        assert!(
            load_recent_messages(&h.layout.recent_messages_json())
                .await
                .unwrap()
                .is_empty(),
            "the forced observation removed what it observed"
        );
    }

    #[tokio::test]
    async fn forced_observe_queues_behind_a_running_cycle() {
        let mut h = observe_harness().await;
        append_message(&h.layout, "first turn").await;
        h.worker.trigger(h.mem.clone());
        h.gate.until_held(1).await;

        append_message(&h.layout, "second turn").await;
        h.worker.force_observe(h.mem.clone());
        assert_eq!(
            h.gate.arrived(),
            1,
            "the forced observation waits for the running cycle"
        );

        h.gate.open_all();
        for _ in 0..2 {
            let result = wait::next("an observation result", &mut h.results).await;
            assert!(matches!(result, PostTurnResult::ObservationReady));
        }
        memory_activity_reaches(&mut h.activity, false).await;
        assert_eq!(h.gate.arrived(), 2, "one cycle, then the forced one");
    }

    #[tokio::test]
    async fn forced_reflect_runs_in_the_background_and_reloads_observations() {
        let mut h = observe_harness().await;
        let reflector_gate = Gate::closed("reflector");
        let search_index = Arc::new(
            crate::memory::search::MemoryIndex::open_or_create(&h.layout.search_index_dir())
                .unwrap(),
        );
        let mem = MemorySubsystems {
            merge_writer: Arc::new(crate::memory::merge_writer::MemoryMergeWriter::new(
                crate::memory::reflector::Reflector::new(
                    Box::new(GatedProvider::new(
                        &reflector_gate,
                        r#"{"observations": [{"content": "compressed", "timestamp": "2026-02-21T14:30", "visibility": "user"}]}"#,
                    )),
                    crate::memory::reflector::ReflectorConfig::default(),
                ),
                h.layout.clone(),
                search_index,
                None,
                None,
            )),
            ..h.mem.clone()
        };
        crate::memory::log_store::append_observations(
            &h.layout.observations_json(),
            vec![crate::memory::types::Observation {
                timestamp: chrono::Utc::now().naive_utc(),
                source_episodes: Some("ep-001".to_string()),
                visibility: Visibility::User,
                content: "something worth keeping".to_string(),
                source: crate::memory::types::SourceTag::main(),
            }],
        )
        .await
        .unwrap();

        h.worker.force_reflect(mem);
        reflector_gate.until_held(1).await;
        memory_activity_reaches(&mut h.activity, true).await;

        reflector_gate.release(1);
        let result = wait::next("the forced reflection result", &mut h.results).await;
        assert!(
            matches!(result, PostTurnResult::ReflectionReady),
            "a reflection alone reloads observations without rotating history"
        );
        assert_eq!(h.gate.arrived(), 0, "a reflection runs no observation");
    }

    fn scratch_with_note(note: &str) -> TurnScratch {
        TurnScratch {
            applied_corrections: vec![],
            queued_notes: vec![finding(Severity::Note, note)],
        }
    }

    fn subconscious_trigger(
        subconscious: Subconscious,
        messages: Vec<Message>,
        scratch: TurnScratch,
    ) -> SubconsciousTrigger {
        SubconsciousTrigger {
            subconscious: Arc::new(subconscious),
            learning_state: Arc::new(Mutex::new(LearningState::default())),
            publisher: crate::bus::spawn_broker().publisher(),
            tz: chrono_tz::UTC,
            learning_cooldown: Duration::from_secs(60),
            new_messages: messages,
            correlation_id: "corr-1".to_string(),
            scratch,
        }
    }

    fn turn(user: &str) -> Vec<Message> {
        vec![
            Message::user(user),
            Message::assistant("ok".to_string(), None),
        ]
    }

    #[tokio::test]
    async fn absorbing_a_newer_trigger_keeps_both_turns_and_their_notes() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let mut waiting = subconscious_trigger(
            Subconscious::disabled(layout.clone()),
            turn("first"),
            scratch_with_note("note from turn one"),
        );
        let mut newer = subconscious_trigger(
            Subconscious::disabled(layout),
            turn("second"),
            scratch_with_note("note from turn two"),
        );
        newer.correlation_id = "corr-2".to_string();

        waiting.absorb(newer);

        let contents: Vec<&str> = waiting
            .new_messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(contents, ["first", "ok", "second", "ok"]);
        let notes: Vec<&str> = waiting
            .scratch
            .queued_notes
            .iter()
            .map(|f| f.instruction.as_str())
            .collect();
        assert_eq!(notes, ["note from turn one", "note from turn two"]);
        assert_eq!(waiting.correlation_id, "corr-2");
    }

    #[tokio::test]
    async fn subconscious_notes_reach_the_main_loop_for_the_next_turn() {
        const NOTE_RESPONSE: &str = r#"{
            "findings": [
                {"kind": "omission", "severity": "note", "instruction": "mention the deadline next time"}
            ]
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let subconscious = Subconscious::new(
            Box::new(crate::memory::testing::MockMemoryProvider::new(
                NOTE_RESPONSE,
            )),
            crate::subconscious::SubconsciousConfig {
                enabled: true,
                ..crate::subconscious::SubconsciousConfig::default()
            },
            WorkspaceLayout::new(dir.path()),
        );
        let (result_tx, mut results) = mpsc::unbounded_channel();
        let worker = SubconsciousWorker::new(result_tx);

        worker.trigger(subconscious_trigger(
            subconscious,
            turn("when is it due?"),
            TurnScratch::default(),
        ));

        let result = wait::next("the subconscious result", &mut results).await;
        let PostTurnResult::SubconsciousNotes(notes) = result else {
            panic!("expected subconscious notes");
        };
        assert_eq!(notes, ["mention the deadline next time"]);
    }

    #[tokio::test]
    async fn failed_subconscious_evaluation_still_delivers_queued_notes() {
        let dir = tempfile::tempdir().unwrap();
        let subconscious = Subconscious::new(
            Box::new(crate::inference::NullProvider),
            crate::subconscious::SubconsciousConfig {
                enabled: true,
                ..crate::subconscious::SubconsciousConfig::default()
            },
            WorkspaceLayout::new(dir.path()),
        );
        let (result_tx, mut results) = mpsc::unbounded_channel();
        let worker = SubconsciousWorker::new(result_tx);

        worker.trigger(subconscious_trigger(
            subconscious,
            turn("hi"),
            scratch_with_note("queued mid-turn"),
        ));

        let result = wait::next("the subconscious result", &mut results).await;
        let PostTurnResult::SubconsciousNotes(notes) = result else {
            panic!("expected the queued note as a fallback");
        };
        assert_eq!(notes, ["queued mid-turn"]);
    }
}

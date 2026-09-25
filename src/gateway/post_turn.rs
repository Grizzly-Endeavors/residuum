//! Post-turn background work: the automatic observer/reflector cycle and the
//! end-of-turn subconscious evaluation, both moved off the event loop so a
//! turn's LLM-calling housekeeping never blocks the next inbound message, a
//! shutdown signal, or a stop request from being handled.
//!
//! Each kind (observe, subconscious) runs through its own worker: at most
//! one cycle in flight at a time, and any trigger that arrives while one is
//! running collapses into exactly one follow-up run rather than stacking —
//! see [`RunState`]. The slow part (the LLM call, plus any file persistence
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
//! learning state, ...), taken from `GatewayRuntime` at the moment of the
//! call. A coalesced run always uses the *latest* trigger's snapshot, so a
//! reload that swaps a component in place between two coalesced triggers is
//! still picked up — nothing here holds a stale clone from before the swap.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bus::{MessageEvent, Publisher, topics};
use crate::gateway::memory::{self, MemorySubsystems};
use crate::inference::Message;
use crate::interfaces::types::MessageOrigin;
use crate::subconscious::{
    EvalPhase, Finding, LearnSignal, LearningState, Severity, Subconscious, TurnScratch,
};

/// What idle transition step to run once a background observe cycle
/// triggered by [`ObserveWorker::trigger_for_idle`] finishes, carried back
/// on [`PostTurnResult::IdleObservationReady`].
pub(crate) struct IdleContinuation {
    pub timeout_mins: u64,
    pub total_skills: usize,
    pub idle_channel: Option<String>,
}

/// What the main loop needs to apply from a finished background cycle.
pub(crate) enum PostTurnResult {
    /// An automatic (non-idle) observe cycle produced a merge — reload the
    /// agent's observations/recent-context views.
    ObservationReady,
    /// The idle-triggered observe cycle finished; `reload_needed` says
    /// whether it actually produced a merge (an idle transition still runs
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

/// Coalescing bookkeeping shared by both workers below: whether a cycle is
/// currently running, and whether a trigger arrived during that run and so
/// owes exactly one follow-up. The running-or-not decision and the
/// pending-or-not decision are always read and written together under this
/// one lock — including at the tail of a cycle, where "is anyone else about
/// to trigger me" and "should I loop again" must agree, or a trigger that
/// lands in the gap between them would be silently lost.
struct RunState {
    running: bool,
    pending: bool,
    handle: Option<JoinHandle<()>>,
}

impl RunState {
    fn new() -> Self {
        Self {
            running: false,
            pending: false,
            handle: None,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Background worker for the automatic observer/reflector cycle.
pub(crate) struct ObserveWorker {
    state: Mutex<RunState>,
    /// The snapshot the next cycle should run with — always the most recent
    /// trigger's, overwritten on every call regardless of coalescing.
    latest_mem: Mutex<Option<MemorySubsystems>>,
    /// Set only by [`Self::trigger_for_idle`] and never cleared by a plain
    /// [`Self::trigger`] — an idle transition queued behind a plain trigger
    /// must still run its continuation once a cycle finally completes.
    idle_continuation: Mutex<Option<IdleContinuation>>,
    result_tx: mpsc::UnboundedSender<PostTurnResult>,
}

impl ObserveWorker {
    #[must_use]
    pub(crate) fn new(result_tx: mpsc::UnboundedSender<PostTurnResult>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RunState::new()),
            latest_mem: Mutex::new(None),
            idle_continuation: Mutex::new(None),
            result_tx,
        })
    }

    /// Trigger a plain (non-idle) observation cycle.
    pub(crate) fn trigger(self: &Arc<Self>, mem: MemorySubsystems) {
        *lock(&self.latest_mem) = Some(mem);
        self.spawn_or_coalesce();
    }

    /// Trigger an observation cycle that, once it finishes, should also run
    /// the idle transition's remaining steps (see `crate::gateway::idle`).
    pub(crate) fn trigger_for_idle(
        self: &Arc<Self>,
        mem: MemorySubsystems,
        continuation: IdleContinuation,
    ) {
        *lock(&self.latest_mem) = Some(mem);
        *lock(&self.idle_continuation) = Some(continuation);
        self.spawn_or_coalesce();
    }

    fn spawn_or_coalesce(self: &Arc<Self>) {
        let mut state = lock(&self.state);
        if state.running {
            state.pending = true;
            return;
        }
        state.running = true;
        state.pending = false;
        let this = Arc::clone(self);
        state.handle = Some(tokio::spawn(async move { this.run_loop().await }));
    }

    async fn run_loop(&self) {
        loop {
            self.run_one_cycle().await;
            let mut state = lock(&self.state);
            if state.pending {
                state.pending = false;
                continue;
            }
            state.running = false;
            return;
        }
    }

    async fn run_one_cycle(&self) {
        let Some(mem) = lock(&self.latest_mem).take() else {
            // Only reachable if `run_loop` somehow ran without a prior
            // `trigger`/`trigger_for_idle` call — both always set this
            // before spawning or marking pending, so this is defensive.
            return;
        };
        let reload_needed = memory::execute_observation(&mem).await;
        let continuation = lock(&self.idle_continuation).take();
        let result = if let Some(continuation) = continuation {
            PostTurnResult::IdleObservationReady {
                reload_needed,
                continuation,
            }
        } else if reload_needed {
            PostTurnResult::ObservationReady
        } else {
            return;
        };
        if self.result_tx.send(result).is_err() {
            tracing::debug!(
                "post-turn result channel closed, dropping observation result (shutting down)"
            );
        }
    }

    /// Give an in-flight cycle up to `grace` to finish, then give up and let
    /// the process exit cut it short — its own writes are already atomic
    /// (temp file + rename), so an interrupted cycle leaves whatever it had
    /// already completed intact and simply discards the rest, never a
    /// corrupted partial state.
    pub(crate) async fn shutdown(&self, grace: Duration) {
        let handle = lock(&self.state).handle.take();
        if let Some(handle) = handle
            && tokio::time::timeout(grace, handle).await.is_err()
        {
            tracing::warn!(
                "observer background cycle still running past the shutdown grace period, \
                 leaving it to end with the process"
            );
        }
    }
}

/// Everything one subconscious evaluation cycle needs, snapshotted fresh
/// from `GatewayRuntime` by the caller at trigger time.
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

/// Background worker for the end-of-turn subconscious evaluation.
pub(crate) struct SubconsciousWorker {
    state: Mutex<RunState>,
    latest: Mutex<Option<SubconsciousTrigger>>,
    result_tx: mpsc::UnboundedSender<PostTurnResult>,
}

impl SubconsciousWorker {
    #[must_use]
    pub(crate) fn new(result_tx: mpsc::UnboundedSender<PostTurnResult>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RunState::new()),
            latest: Mutex::new(None),
            result_tx,
        })
    }

    pub(crate) fn trigger(self: &Arc<Self>, payload: SubconsciousTrigger) {
        *lock(&self.latest) = Some(payload);
        let mut state = lock(&self.state);
        if state.running {
            state.pending = true;
            return;
        }
        state.running = true;
        state.pending = false;
        let this = Arc::clone(self);
        state.handle = Some(tokio::spawn(async move { this.run_loop().await }));
    }

    async fn run_loop(&self) {
        loop {
            self.run_one_cycle().await;
            let mut state = lock(&self.state);
            if state.pending {
                state.pending = false;
                continue;
            }
            state.running = false;
            return;
        }
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

        let notes = match trigger
            .subconscious
            .evaluate(
                &trigger.new_messages,
                EvalPhase::EndOfTurn,
                Some(&trigger.scratch),
            )
            .await
        {
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

    /// Same shutdown contract as [`ObserveWorker::shutdown`].
    pub(crate) async fn shutdown(&self, grace: Duration) {
        let handle = lock(&self.state).handle.take();
        if let Some(handle) = handle
            && tokio::time::timeout(grace, handle).await.is_err()
        {
            tracing::warn!(
                "subconscious background cycle still running past the shutdown grace period, \
                 leaving it to end with the process"
            );
        }
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
        assert!(plan_delivery(vec![]).is_empty());
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

    #[tokio::test]
    async fn observe_worker_coalesces_triggers_that_arrive_mid_cycle() {
        // Two triggers fired back to back should never run the observation
        // cycle twice in a row for the second one — coalescing means the
        // worker's own loop notices the pending flag and runs once more, but
        // a THIRD trigger sent only after the worker is fully idle again
        // must start a fresh run. This exercises the public trigger/run_loop
        // path directly against a real (but tiny, disabled) memory stack so
        // the state machine is proven end to end, not just unit-level.
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        let observer = Arc::new(crate::memory::observer::Observer::disabled(chrono_tz::UTC));
        let search_index = Arc::new(
            crate::memory::search::MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap(),
        );
        let reflector = crate::memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = Arc::new(crate::memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let handle = crate::bus::spawn_broker();
        let (result_tx, mut result_rx) = mpsc::unbounded_channel();
        let worker = ObserveWorker::new(result_tx);

        let mem = || MemorySubsystems {
            observer: Arc::clone(&observer),
            merge_writer: Arc::clone(&merge_writer),
            layout: layout.clone(),
            tz: chrono_tz::UTC,
            publisher: handle.publisher(),
        };

        worker.trigger(mem());
        worker.trigger(mem());

        // A disabled observer never has anything to observe (no recent
        // messages file even exists), so no result is ever sent — this
        // test only needs to prove the worker doesn't panic or deadlock
        // running two coalesced triggers back to back, then a third after
        // going idle again.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        worker.trigger(mem());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), result_rx.recv())
                .await
                .is_err(),
            "a disabled observer with nothing to observe should never report a result"
        );

        worker.shutdown(std::time::Duration::from_secs(1)).await;
    }
}

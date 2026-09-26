//! Mid-turn subconscious watch: spawns concurrent evaluations during the
//! agent's tool loop and injects corrections via the interrupt channel.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{EvalPhase, Finding, Severity, Subconscious, TurnScratch};
use crate::agent::interrupt::Interrupt;
use crate::inference::Message;

/// Record a correction the watch actually delivered to the agent this turn.
fn record_applied(scratch: &Mutex<TurnScratch>, instruction: String) {
    if let Ok(mut s) = scratch.lock() {
        s.applied_corrections.push(instruction);
    }
}

/// Record a finding the watch observed but did not deliver as a live steer.
fn record_note(scratch: &Mutex<TurnScratch>, finding: Finding) {
    if let Ok(mut s) = scratch.lock() {
        s.queued_notes.push(finding);
    }
}

/// Per-turn handle that gates and spawns mid-turn evaluations.
///
/// Evaluations run in detached tasks so the tool loop never waits on the
/// classifier; a correction lands at the next interrupt drain. At most one
/// evaluation is in flight at a time. Every `act`-severity finding across
/// the turn is delivered as a correction — there is no per-turn cap — and
/// each one carries a running count of how many corrections this turn has
/// delivered so far, so the user can see mid-turn steering piling up rather
/// than it happening invisibly.
///
/// As it runs it records what it did into `scratch`, which the end-of-turn
/// pass reads to triage rather than re-classify the same turn.
pub struct SubconsciousWatch {
    subconscious: Arc<Subconscious>,
    interrupt_tx: mpsc::UnboundedSender<Interrupt>,
    in_flight: Arc<AtomicBool>,
    /// Count of corrections delivered so far this turn; included in each
    /// delivered correction's text as the only surface where subconscious
    /// mid-turn activity is currently shown to the user.
    corrections_delivered: Arc<AtomicUsize>,
    scratch: Arc<Mutex<TurnScratch>>,
}

impl SubconsciousWatch {
    /// Create a watch for one turn, holding a sender into that turn's
    /// interrupt channel.
    #[must_use]
    pub fn new(
        subconscious: Arc<Subconscious>,
        interrupt_tx: mpsc::UnboundedSender<Interrupt>,
    ) -> Self {
        Self {
            subconscious,
            interrupt_tx,
            in_flight: Arc::new(AtomicBool::new(false)),
            corrections_delivered: Arc::new(AtomicUsize::new(0)),
            scratch: Arc::new(Mutex::new(TurnScratch::default())),
        }
    }

    /// Shared handle to the steering recorded so far this turn.
    ///
    /// The end-of-turn pass reads this after the tool loop exits. Best-effort:
    /// an evaluation still in flight when the turn ends may not be reflected.
    #[must_use]
    pub fn scratch(&self) -> Arc<Mutex<TurnScratch>> {
        Arc::clone(&self.scratch)
    }

    /// Spawn a mid-turn evaluation of `transcript` if the gates allow it.
    ///
    /// Gates: mid-turn enabled, cadence (`every_n_iterations`), no evaluation
    /// already in flight. There is no cap on how many corrections a turn may
    /// receive — every `act` finding is delivered. `transcript` should be
    /// the messages of the current turn so far.
    pub fn maybe_spawn(&self, iteration: usize, transcript: Vec<Message>) {
        if !self.subconscious.mid_turn_enabled() {
            return;
        }
        let every_n = self.subconscious.every_n_iterations();
        if iteration % every_n != every_n - 1 {
            return;
        }
        // Only one evaluation in flight: a slow classifier must not pile up.
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let subconscious = Arc::clone(&self.subconscious);
        let interrupt_tx = self.interrupt_tx.clone();
        let in_flight = Arc::clone(&self.in_flight);
        let corrections_delivered = Arc::clone(&self.corrections_delivered);
        let scratch = Arc::clone(&self.scratch);

        crate::util::spawn_monitored("subconscious-mid-turn", async move {
            match subconscious
                .evaluate(&transcript, EvalPhase::MidTurn, None)
                .await
            {
                Ok(outcome) => {
                    // Deliver every `act` finding as a live correction; `note`
                    // findings are recorded for the end-of-turn triage instead.
                    // Learn signals are never emitted mid-turn, so
                    // `outcome.learnings` is empty here.
                    for finding in outcome.findings {
                        if finding.severity != Severity::Act {
                            record_note(&scratch, finding);
                            continue;
                        }

                        let count = corrections_delivered.fetch_add(1, Ordering::AcqRel) + 1;
                        let content = format!(
                            "[Subconscious] Course correction #{count} this turn:\n{}",
                            finding.instruction
                        );
                        if interrupt_tx.send(Interrupt::Subconscious(content)).is_ok() {
                            tracing::debug!(
                                correction_number = count,
                                "delivered subconscious mid-turn correction"
                            );
                            record_applied(&scratch, finding.instruction);
                        } else {
                            // The turn already ended; the correction never
                            // reached the agent. Hand it to the end-of-turn
                            // pass as a queued note instead of losing it.
                            tracing::debug!(
                                "turn ended before subconscious correction could be injected"
                            );
                            record_note(&scratch, finding);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "subconscious mid-turn evaluation failed");
                }
            }
            in_flight.store(false, Ordering::Release);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::test_helpers::MockMemoryProvider;
    use crate::subconscious::SubconsciousConfig;
    use crate::workspace::layout::WorkspaceLayout;

    const ACT_RESPONSE: &str = r#"{
        "findings": [
            {"kind": "violation", "severity": "act", "instruction": "Stop and re-read AGENTS.md."}
        ]
    }"#;
    const EMPTY_RESPONSE: &str = r#"{"findings": []}"#;

    fn make_watch(
        response: &str,
        config: SubconsciousConfig,
    ) -> (SubconsciousWatch, mpsc::UnboundedReceiver<Interrupt>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sub = Arc::new(Subconscious::new(
            Box::new(MockMemoryProvider::new(response)),
            config,
            WorkspaceLayout::new("/tmp/ws"),
        ));
        (SubconsciousWatch::new(sub, tx), rx)
    }

    fn enabled_config() -> SubconsciousConfig {
        SubconsciousConfig {
            enabled: true,
            mid_turn: true,
            every_n_iterations: 1,
            ..SubconsciousConfig::default()
        }
    }

    fn transcript() -> Vec<Message> {
        vec![
            Message::user("do the thing"),
            Message::assistant("working on it".to_string(), None),
        ]
    }

    #[tokio::test]
    async fn act_finding_lands_in_interrupt_channel() {
        let (watch, mut rx) = make_watch(ACT_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());

        let interrupt = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match interrupt {
            Interrupt::Subconscious(content) => {
                assert!(
                    content.contains("re-read AGENTS.md"),
                    "correction should carry the instruction"
                );
            }
            Interrupt::UserMessage(_) | Interrupt::AgentMessage(_) | Interrupt::Stopped => {
                unreachable!("expected a subconscious interrupt")
            }
        }
    }

    #[tokio::test]
    async fn empty_findings_send_nothing() {
        let (watch, mut rx) = make_watch(EMPTY_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err(), "no finding, no interrupt");
    }

    #[tokio::test]
    async fn cadence_gates_iterations() {
        let (watch, mut rx) = make_watch(
            ACT_RESPONSE,
            SubconsciousConfig {
                every_n_iterations: 3,
                ..enabled_config()
            },
        );

        // Iterations 0 and 1 don't match the every-3 cadence (fires at 2, 5, ...).
        watch.maybe_spawn(0, transcript());
        watch.maybe_spawn(1, transcript());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            rx.try_recv().is_err(),
            "off-cadence iterations must not fire"
        );

        watch.maybe_spawn(2, transcript());
        let interrupt = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap();
        assert!(interrupt.is_some(), "on-cadence iteration should fire");
    }

    #[tokio::test]
    async fn disabled_mid_turn_never_spawns() {
        let (watch, mut rx) = make_watch(
            ACT_RESPONSE,
            SubconsciousConfig {
                mid_turn: false,
                ..enabled_config()
            },
        );
        watch.maybe_spawn(0, transcript());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err(), "mid_turn off must not evaluate");
    }

    #[tokio::test]
    async fn every_correction_is_delivered_with_no_cap() {
        // There is no per-turn cap: a second evaluation's `act` finding is
        // still delivered live, not demoted to a note.
        let (watch, mut rx) = make_watch(ACT_RESPONSE, enabled_config());

        watch.maybe_spawn(0, transcript());
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Interrupt::Subconscious(first_content) = first else {
            unreachable!("expected a subconscious interrupt")
        };
        assert!(
            first_content.contains("#1"),
            "first correction should be numbered #1, got: {first_content}"
        );

        // Wait for in_flight to clear, then evaluate again.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        watch.maybe_spawn(1, transcript());
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Interrupt::Subconscious(second_content) = second else {
            unreachable!("expected a subconscious interrupt")
        };
        assert!(
            second_content.contains("#2"),
            "second correction should be delivered and numbered #2, not capped, got: {second_content}"
        );

        let scratch = watch.scratch();
        let s = scratch.lock().unwrap();
        assert_eq!(
            s.applied_corrections.len(),
            2,
            "both corrections should be recorded as applied"
        );
    }

    #[tokio::test]
    async fn applied_correction_is_recorded_in_scratch() {
        let (watch, mut rx) = make_watch(ACT_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());
        // Drain the interrupt so we know the eval task ran to completion.
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let scratch = watch.scratch();
        let s = scratch.lock().unwrap();
        assert_eq!(s.applied_corrections.len(), 1, "delivered act recorded");
        assert!(
            s.applied_corrections
                .first()
                .is_some_and(|c| c.contains("re-read AGENTS.md"))
        );
        assert!(
            s.queued_notes.is_empty(),
            "the act was delivered, not queued"
        );
    }

    #[tokio::test]
    async fn note_findings_are_queued_not_injected() {
        const NOTE_RESPONSE: &str = r#"{
            "findings": [
                {"kind": "omission", "severity": "note", "instruction": "Consider saving the preference."}
            ]
        }"#;
        let (watch, mut rx) = make_watch(NOTE_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert!(
            rx.try_recv().is_err(),
            "notes must not be injected mid-turn"
        );
        let scratch = watch.scratch();
        let s = scratch.lock().unwrap();
        assert_eq!(
            s.queued_notes.len(),
            1,
            "note queued for end-of-turn triage"
        );
        assert!(s.applied_corrections.is_empty());
    }

    #[tokio::test]
    async fn dropped_receiver_does_not_panic() {
        let (watch, rx) = make_watch(ACT_RESPONSE, enabled_config());
        drop(rx);
        watch.maybe_spawn(0, transcript());
        // Give the spawned task time to run; spawn_monitored would log a panic,
        // and the test harness would surface an abort if send panicked.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

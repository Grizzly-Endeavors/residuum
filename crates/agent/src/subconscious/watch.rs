use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{EvalPhase, Finding, Severity, Subconscious, TurnScratch};
use residuum_bus::interrupt::Interrupt;
use residuum_inference::Message;

fn record_applied(scratch: &Mutex<TurnScratch>, instruction: String) {
    if let Ok(mut s) = scratch.lock() {
        s.applied_corrections.push(instruction);
    }
}

fn record_note(scratch: &Mutex<TurnScratch>, finding: Finding) {
    if let Ok(mut s) = scratch.lock() {
        s.queued_notes.push(finding);
    }
}

pub struct SubconsciousWatch {
    subconscious: Arc<Subconscious>,
    interrupt_tx: mpsc::UnboundedSender<Interrupt>,
    in_flight: Arc<AtomicBool>,
    corrections_delivered: Arc<AtomicUsize>,
    scratch: Arc<Mutex<TurnScratch>>,
}

impl SubconsciousWatch {
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

    #[must_use]
    pub fn scratch(&self) -> Arc<Mutex<TurnScratch>> {
        Arc::clone(&self.scratch)
    }

    pub fn maybe_spawn(&self, iteration: usize, transcript: Vec<Message>) {
        if !self.subconscious.mid_turn_enabled() {
            return;
        }
        let every_n = self.subconscious.every_n_iterations();
        if iteration % every_n != every_n - 1 {
            return;
        }
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

        residuum_core::util::spawn_monitored("subconscious-mid-turn", async move {
            match subconscious
                .evaluate(&transcript, EvalPhase::MidTurn, None)
                .await
            {
                Ok(outcome) => {
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
    use crate::subconscious::SubconsciousConfig;
    use residuum_config::WorkspaceLayout;
    use residuum_core::testing::wait;
    use residuum_memory::testing::MockMemoryProvider;

    const ACT_RESPONSE: &str = r#"{
        "findings": [
            {"kind": "violation", "severity": "act", "instruction": "Stop and re-read AGENTS.md."}
        ]
    }"#;
    const EMPTY_RESPONSE: &str = r#"{"findings": []}"#;

    async fn wait_until_idle(watch: &SubconsciousWatch) {
        wait::until_true("the mid-turn evaluation to finish", || {
            !watch.in_flight.load(Ordering::Acquire)
        })
        .await;
    }

    fn make_watch(
        response: &str,
        config: SubconsciousConfig,
    ) -> (
        SubconsciousWatch,
        mpsc::UnboundedReceiver<Interrupt>,
        tempfile::TempDir,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        let sub = Arc::new(Subconscious::new(
            Box::new(MockMemoryProvider::new(response)),
            config,
            WorkspaceLayout::new(dir.path()),
        ));
        (SubconsciousWatch::new(sub, tx), rx, dir)
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
        let (watch, mut rx, _dir) = make_watch(ACT_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());

        let interrupt = wait::guarded("a subconscious interrupt", rx.recv())
            .await
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
        let (watch, mut rx, _dir) = make_watch(EMPTY_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());

        wait_until_idle(&watch).await;
        assert!(rx.try_recv().is_err(), "no finding, no interrupt");
    }

    #[tokio::test]
    async fn cadence_gates_iterations() {
        let (watch, mut rx, _dir) = make_watch(
            ACT_RESPONSE,
            SubconsciousConfig {
                every_n_iterations: 3,
                ..enabled_config()
            },
        );

        watch.maybe_spawn(0, transcript());
        watch.maybe_spawn(1, transcript());
        wait_until_idle(&watch).await;
        assert!(
            rx.try_recv().is_err(),
            "off-cadence iterations must not fire"
        );

        watch.maybe_spawn(2, transcript());
        let interrupt = wait::guarded("a subconscious interrupt", rx.recv()).await;
        assert!(interrupt.is_some(), "on-cadence iteration should fire");
    }

    #[tokio::test]
    async fn disabled_mid_turn_never_spawns() {
        let (watch, mut rx, _dir) = make_watch(
            ACT_RESPONSE,
            SubconsciousConfig {
                mid_turn: false,
                ..enabled_config()
            },
        );
        watch.maybe_spawn(0, transcript());
        wait_until_idle(&watch).await;
        assert!(rx.try_recv().is_err(), "mid_turn off must not evaluate");
    }

    #[tokio::test]
    async fn every_correction_is_delivered_with_no_cap() {
        let (watch, mut rx, _dir) = make_watch(ACT_RESPONSE, enabled_config());

        watch.maybe_spawn(0, transcript());
        let first = wait::guarded("the first subconscious interrupt", rx.recv())
            .await
            .unwrap();
        let Interrupt::Subconscious(first_content) = first else {
            unreachable!("expected a subconscious interrupt")
        };
        assert!(
            first_content.contains("#1"),
            "first correction should be numbered #1, got: {first_content}"
        );

        wait_until_idle(&watch).await;
        watch.maybe_spawn(1, transcript());
        let second = wait::guarded("the second subconscious interrupt", rx.recv())
            .await
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
        let (watch, mut rx, _dir) = make_watch(ACT_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());
        wait::guarded("a subconscious interrupt", rx.recv()).await;
        wait_until_idle(&watch).await;

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
        let (watch, mut rx, _dir) = make_watch(NOTE_RESPONSE, enabled_config());
        watch.maybe_spawn(0, transcript());
        wait_until_idle(&watch).await;

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
        assert!(
            s.applied_corrections.is_empty(),
            "expected {:?} to be empty",
            s.applied_corrections
        );
    }

    #[tokio::test]
    async fn dropped_receiver_does_not_panic() {
        let (watch, rx, _dir) = make_watch(ACT_RESPONSE, enabled_config());
        drop(rx);
        watch.maybe_spawn(0, transcript());
        wait_until_idle(&watch).await;
    }
}

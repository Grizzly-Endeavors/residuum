//! Mid-turn subconscious watch: spawns concurrent evaluations during the
//! agent's tool loop and injects corrections via the interrupt channel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::sync::mpsc;

use super::{EvalPhase, Severity, Subconscious};
use crate::agent::interrupt::Interrupt;
use crate::models::Message;

/// Per-turn handle that gates and spawns mid-turn evaluations.
///
/// Evaluations run in detached tasks so the tool loop never waits on the
/// classifier; a correction lands at the next interrupt drain. At most one
/// evaluation is in flight at a time and at most
/// `max_interventions_per_turn` corrections are injected per turn.
pub struct SubconsciousWatch {
    subconscious: Arc<Subconscious>,
    interrupt_tx: mpsc::Sender<Interrupt>,
    in_flight: Arc<AtomicBool>,
    interventions: Arc<AtomicUsize>,
}

impl SubconsciousWatch {
    /// Create a watch for one turn, holding a sender into that turn's
    /// interrupt channel.
    #[must_use]
    pub fn new(subconscious: Arc<Subconscious>, interrupt_tx: mpsc::Sender<Interrupt>) -> Self {
        Self {
            subconscious,
            interrupt_tx,
            in_flight: Arc::new(AtomicBool::new(false)),
            interventions: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Spawn a mid-turn evaluation of `transcript` if the gates allow it.
    ///
    /// Gates: mid-turn enabled, cadence (`every_n_iterations`), no evaluation
    /// already in flight, intervention cap not reached. `transcript` should be
    /// the messages of the current turn so far.
    pub fn maybe_spawn(&self, iteration: usize, transcript: Vec<Message>) {
        if !self.subconscious.mid_turn_enabled() {
            return;
        }
        let every_n = self.subconscious.every_n_iterations();
        if iteration % every_n != every_n - 1 {
            return;
        }
        if self.interventions.load(Ordering::Relaxed)
            >= self.subconscious.max_interventions_per_turn()
        {
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
        let interventions = Arc::clone(&self.interventions);
        let max_interventions = self.subconscious.max_interventions_per_turn();

        crate::util::spawn_monitored("subconscious-mid-turn", async move {
            match subconscious.evaluate(&transcript, EvalPhase::MidTurn).await {
                Ok(findings) => {
                    if let Some(finding) =
                        findings.into_iter().find(|f| f.severity == Severity::Act)
                        && interventions.fetch_add(1, Ordering::AcqRel) < max_interventions
                    {
                        let content = format!(
                            "[Subconscious] Course correction for the work in progress:\n{}",
                            finding.instruction
                        );
                        // A failed send means the turn already ended; the
                        // end-of-turn pass will catch anything persistent.
                        if interrupt_tx
                            .try_send(Interrupt::Subconscious(content))
                            .is_err()
                        {
                            tracing::debug!(
                                "turn ended before subconscious correction could be injected"
                            );
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
    ) -> (SubconsciousWatch, mpsc::Receiver<Interrupt>) {
        let (tx, rx) = mpsc::channel(8);
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
            Interrupt::UserMessage(_) | Interrupt::BackgroundResult(_) => {
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
    async fn intervention_cap_limits_corrections() {
        let (watch, mut rx) = make_watch(
            ACT_RESPONSE,
            SubconsciousConfig {
                max_interventions_per_turn: 1,
                ..enabled_config()
            },
        );

        watch.maybe_spawn(0, transcript());
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap();
        assert!(first.is_some(), "first correction should be injected");

        // Wait for in_flight to clear, then try again — the cap must block it.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        watch.maybe_spawn(1, transcript());
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            rx.try_recv().is_err(),
            "intervention cap should block the second correction"
        );
    }

    #[tokio::test]
    async fn dropped_receiver_does_not_panic() {
        let (watch, rx) = make_watch(ACT_RESPONSE, enabled_config());
        drop(rx);
        watch.maybe_spawn(0, transcript());
        // Give the spawned task time to run; spawn_monitored would log a panic,
        // and the test harness would surface an abort if try_send panicked.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

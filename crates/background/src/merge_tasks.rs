use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::time::Instant;
use tokio_util::task::TaskTracker;

#[derive(Clone, Default)]
pub(crate) struct MergeTasks {
    tracker: TaskTracker,
    pending: Arc<Mutex<BTreeSet<String>>>,
}

impl MergeTasks {
    pub(crate) fn spawn<F>(&self, run_id: String, merge: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        lock(&self.pending).insert(run_id.clone());
        let guard = PendingGuard {
            pending: Arc::clone(&self.pending),
            run_id,
        };
        residuum_core::util::spawn_in_span(self.tracker.track_future(async move {
            merge.await;
            drop(guard);
        }));
    }

    pub(crate) async fn drain(&self, deadline: Instant) {
        self.tracker.close();
        if self.tracker.is_empty() {
            return;
        }
        tracing::info!(
            merges = self.tracker.len(),
            "waiting for memory merges to finish for shutdown"
        );
        if tokio::time::timeout_at(deadline, self.tracker.wait())
            .await
            .is_ok()
        {
            tracing::info!("all memory merges finished for shutdown");
            return;
        }
        for run_id in lock(&self.pending).iter() {
            tracing::warn!(
                run_id = %run_id,
                "shutdown timed out with this run's memory merge still running; startup recovery finishes it if the process exits"
            );
        }
    }
}

struct PendingGuard {
    pending: Arc<Mutex<BTreeSet<String>>>,
    run_id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        lock(&self.pending).remove(&self.run_id);
    }
}

fn lock(pending: &Mutex<BTreeSet<String>>) -> MutexGuard<'_, BTreeSet<String>> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;

    use super::*;

    #[tokio::test]
    async fn drain_waits_for_a_running_merge_and_returns_once_it_ends() {
        let tasks = MergeTasks::default();
        let gate = Gate::closed("merge");
        let entry = gate.entry();
        tasks.spawn("run-a".to_string(), async move {
            entry.pass().await;
        });
        gate.until_held(1).await;

        let drain = tasks.drain(Instant::now() + wait::HANG_GUARD * 2);
        tokio::pin!(drain);
        assert!(
            futures_util::FutureExt::now_or_never(drain.as_mut()).is_none(),
            "drain must not return while the merge is held"
        );

        gate.open_all();
        wait::guarded("drain to return once the merge ends", drain).await;
        assert!(lock(&tasks.pending).is_empty());
    }

    #[tokio::test]
    async fn drain_with_no_merges_returns_at_once() {
        let tasks = MergeTasks::default();
        let drain = tasks.drain(Instant::now() + wait::HANG_GUARD * 2);
        tokio::pin!(drain);
        assert!(futures_util::FutureExt::now_or_never(drain.as_mut()).is_some());
    }
}

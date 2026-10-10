//! Discord, Telegram, and Teams tasks the event loop supervises together.
//!
//! Each running adapter is a name, a task handle, and a shutdown signal.
//! Reload and shutdown address an adapter by name instead of a dedicated
//! field pair on the gateway runtime.

use std::future::Future;
use std::time::Duration;

use futures_util::future::FutureExt;
use tokio::task::JoinHandle;

/// One chat adapter the gateway started.
struct ChatAdapter {
    name: &'static str,
    handle: Option<JoinHandle<()>>,
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

/// The chat adapters that are running.
pub(crate) struct ChatAdapters {
    tasks: Vec<ChatAdapter>,
}

impl ChatAdapters {
    /// No adapters running.
    pub(crate) fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Record an adapter that was just spawned.
    pub(crate) fn insert(
        &mut self,
        name: &'static str,
        handle: JoinHandle<()>,
        shutdown: tokio::sync::watch::Sender<bool>,
    ) {
        self.tasks.push(ChatAdapter {
            name,
            handle: Some(handle),
            shutdown: Some(shutdown),
        });
    }

    /// Stop every running adapter and wait for them to finish, each for up to
    /// five seconds. A restart of the same agent starts its adapters again
    /// right after this, so two copies of one bot never run at once.
    pub(crate) async fn shutdown_all(&mut self) {
        let stops = self.tasks.iter_mut().map(|task| {
            let name = task.name;
            shutdown_adapter(&mut task.shutdown, &mut task.handle, name)
        });
        futures_util::future::join_all(stops).await;
    }

    /// Stop the named adapter if it is running, then start `build` when the
    /// new config still has one.
    pub(crate) async fn reload<F, Fut>(&mut self, name: &'static str, build: Option<F>)
    where
        F: FnOnce(tokio::sync::watch::Receiver<bool>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let position = self.tasks.iter().position(|task| task.name == name);
        if let Some(index) = position {
            let mut task = self.tasks.remove(index);
            shutdown_adapter(&mut task.shutdown, &mut task.handle, name).await;
            if let Some(build) = build {
                spawn_into(&mut task, build);
                self.tasks.push(task);
                tracing::info!(adapter = %name, "adapter restarted with new config");
            } else {
                tracing::info!(adapter = %name, "adapter removed from config");
            }
        } else if let Some(build) = build {
            let mut task = ChatAdapter {
                name,
                handle: None,
                shutdown: None,
            };
            spawn_into(&mut task, build);
            self.tasks.push(task);
            tracing::info!(adapter = %name, "adapter restarted with new config");
        }
    }

    /// Resolves when the first running adapter's task ends, and forgets that
    /// task so it is not reported again. Pends while none are running.
    pub(crate) async fn next_exit(&mut self) -> (&'static str, Result<(), tokio::task::JoinError>) {
        if self.tasks.is_empty() {
            return std::future::pending().await;
        }
        let futures = self
            .tasks
            .iter_mut()
            .map(|task| poll_task(task).boxed())
            .collect::<Vec<_>>();
        let ((name, result), _index, _rest) = futures_util::future::select_all(futures).await;
        (name, result)
    }
}

impl Drop for ChatAdapters {
    /// Adapters must not outlive the agent that owns them: dropping the set
    /// (a graceful shutdown already stopped them; a crashed event loop did
    /// not) signals and aborts whatever is still running.
    fn drop(&mut self) {
        for task in &mut self.tasks {
            if let Some(tx) = task.shutdown.take() {
                tx.send(true).ok();
            }
            if let Some(handle) = task.handle.take() {
                handle.abort();
            }
        }
    }
}

fn spawn_into<F, Fut>(task: &mut ChatAdapter, build: F)
where
    F: FnOnce(tokio::sync::watch::Receiver<bool>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::watch::channel(false);
    task.handle = Some(crate::util::spawn_monitored(task.name, build(rx)));
    task.shutdown = Some(tx);
}

/// Await a task if it is still running, or pend forever if it is not.
///
/// On completion the slot is cleared so a later poll does not await the
/// same handle again.
async fn poll_task(task: &mut ChatAdapter) -> (&'static str, Result<(), tokio::task::JoinError>) {
    let result = match &mut task.handle {
        Some(handle) => handle.await,
        None => std::future::pending().await,
    };
    task.handle = None;
    (task.name, result)
}

/// Stop an adapter task and wait up to five seconds for it to finish.
pub(crate) async fn shutdown_adapter(
    shutdown_tx: &mut Option<tokio::sync::watch::Sender<bool>>,
    handle: &mut Option<JoinHandle<()>>,
    name: &str,
) {
    if let Some(tx) = shutdown_tx.take() {
        tx.send(true).ok();
    }
    if let Some(handle) = handle.take() {
        if tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .is_ok()
        {
            tracing::info!(adapter = %name, "adapter stopped");
        } else {
            tracing::warn!(adapter = %name, "adapter shutdown timed out after 5s");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::wait;

    #[tokio::test]
    async fn next_exit_reports_a_finished_adapter_once() {
        let mut adapters = ChatAdapters::new();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        adapters.insert("discord", crate::util::spawn_in_span(async {}), tx);

        let (name, result) = adapters.next_exit().await;
        assert_eq!(name, "discord");
        assert!(result.is_ok(), "finished adapter task");

        // Polled once without waiting: the exit above was taken from a finished
        // adapter, so a second report would be ready on this first poll.
        let reported_again = tokio::select! {
            biased;
            _ = adapters.next_exit() => true,
            () = std::future::ready(()) => false,
        };
        assert!(!reported_again, "a finished adapter is not reported again");
    }

    #[tokio::test]
    async fn reload_stops_the_running_adapter_and_starts_the_replacement() {
        let mut adapters = ChatAdapters::new();
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let handle = crate::util::spawn_in_span(async move {
            let _changed = rx.changed().await;
        });
        adapters.insert("telegram", handle, tx);

        adapters.reload("telegram", Some(|_rx| async {})).await;

        let (name, result) = adapters.next_exit().await;
        assert_eq!(name, "telegram");
        assert!(result.is_ok(), "replacement adapter task");
    }

    #[tokio::test]
    async fn dropping_the_adapters_stops_their_tasks() {
        let (alive_tx, alive_rx) = tokio::sync::oneshot::channel::<()>();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let mut adapters = ChatAdapters::new();
        adapters.insert(
            "discord",
            crate::util::spawn_in_span(async move {
                // Holding the sender until the task ends lets the test see it end.
                let _alive = alive_tx;
                std::future::pending::<()>().await;
            }),
            tx,
        );

        drop(adapters);

        wait::guarded("the adapter task to end", alive_rx)
            .await
            .expect_err("the task dropped its sender without sending");
    }

    #[tokio::test]
    async fn shutdown_all_waits_for_every_adapter_to_finish() {
        let mut adapters = ChatAdapters::new();
        for name in ["discord", "telegram"] {
            let (tx, mut rx) = tokio::sync::watch::channel(false);
            let handle = crate::util::spawn_in_span(async move {
                let _changed = rx.changed().await;
            });
            adapters.insert(name, handle, tx);
        }

        adapters.shutdown_all().await;

        for task in &adapters.tasks {
            assert!(task.handle.is_none(), "{} was awaited", task.name);
            assert!(task.shutdown.is_none(), "{} was signalled", task.name);
        }
    }
}

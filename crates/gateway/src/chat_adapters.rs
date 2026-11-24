use std::future::Future;
use std::time::Duration;

use futures_util::future::FutureExt;
use tokio::task::JoinHandle;

struct ChatAdapter {
    name: &'static str,
    handle: Option<JoinHandle<()>>,
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

pub(crate) struct ChatAdapters {
    tasks: Vec<ChatAdapter>,
}

impl ChatAdapters {
    pub(crate) fn new() -> Self {
        Self { tasks: Vec::new() }
    }

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

    pub(crate) async fn shutdown_all(&mut self) {
        let stops = self.tasks.iter_mut().map(|task| {
            let name = task.name;
            shutdown_adapter(&mut task.shutdown, &mut task.handle, name)
        });
        futures_util::future::join_all(stops).await;
    }

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
    task.handle = Some(residuum_core::util::spawn_monitored(task.name, build(rx)));
    task.shutdown = Some(tx);
}

async fn poll_task(task: &mut ChatAdapter) -> (&'static str, Result<(), tokio::task::JoinError>) {
    let result = match &mut task.handle {
        Some(handle) => handle.await,
        None => std::future::pending().await,
    };
    task.handle = None;
    (task.name, result)
}

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
    use residuum_core::testing::wait;

    #[tokio::test]
    async fn next_exit_reports_a_finished_adapter_once() {
        let mut adapters = ChatAdapters::new();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        adapters.insert("discord", residuum_core::util::spawn_in_span(async {}), tx);

        let (name, result) = adapters.next_exit().await;
        assert_eq!(name, "discord");
        assert!(result.is_ok(), "finished adapter task");

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
        let handle = residuum_core::util::spawn_in_span(async move {
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
            residuum_core::util::spawn_in_span(async move {
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
            let handle = residuum_core::util::spawn_in_span(async move {
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

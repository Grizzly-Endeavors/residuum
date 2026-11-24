use std::future::Future;

use futures_util::FutureExt;
use tokio::task::JoinHandle;
use tracing::Instrument;

#[must_use]
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>")
}

pub fn spawn_in_span<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future.in_current_span())
}

pub fn spawn_blocking_in_span<F, R>(work: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || span.in_scope(work))
}

pub fn spawn_monitored<F>(name: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let span = tracing::info_span!("monitored_task", task = name);
    tokio::spawn(
        async move {
            tracing::debug!("task started");
            match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                Ok(()) => {
                    tracing::debug!("task exited (returned normally)");
                }
                Err(e) => {
                    tracing::error!(panic = panic_message(&*e), "task panicked");
                }
            }
        }
        .instrument(span),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn monitored_normal_completion() {
        let handle = spawn_monitored("test-normal", async {});
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn monitored_panic_is_swallowed() {
        let handle = spawn_monitored("test-panic", async { panic!("intentional panic") });
        handle.await.unwrap();
    }
}

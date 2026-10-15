//! Task spawning utilities: monitored (fire-and-forget with panic logging).

use std::future::Future;

use futures_util::FutureExt;
use tokio::task::JoinHandle;
use tracing::Instrument;

/// Extract a human-readable message from a caught panic payload.
///
/// Handles the common `panic!("...")` and `panic!(String)` payload shapes;
/// falls back to a placeholder for anything else (e.g. a panic carrying a
/// custom struct via `std::panic::panic_any`).
#[must_use]
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>")
}

/// Spawn `future` as a task that keeps the caller's current tracing span.
///
/// A bare `tokio::spawn` starts the task with no span, so every log line it
/// writes loses the `agent` field of the agent that started it. Everything
/// an agent's runtime spawns goes through here (or [`spawn_blocking_in_span`])
/// so the span opened at the agent's root reaches all of its tasks; a
/// source-scan test in `crate::testing` fails if a bare spawn appears
/// elsewhere in the crate.
pub fn spawn_in_span<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future.in_current_span())
}

/// Run `work` on the blocking pool inside the caller's current tracing span.
///
/// The blocking counterpart of [`spawn_in_span`].
pub fn spawn_blocking_in_span<F, R>(work: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || span.in_scope(work))
}

/// Spawn a monitored task that catches panics and logs them.
///
/// Use this for long-lived tasks (adapters, tunnel) where a silent panic
/// would leave the system in a degraded state. Short-lived / fire-and-forget
/// tasks can continue using bare `tokio::spawn`.
pub fn spawn_monitored<F>(name: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let span = tracing::info_span!("monitored_task", task = name);
    tokio::spawn(
        async move {
            tracing::debug!("task started");
            // catch_unwind here so a panicking task doesn't silently vanish — the JoinHandle still resolves normally.
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

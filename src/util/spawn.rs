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

//! Monitored task spawning utility.
//!
//! Wraps `tokio::spawn` with `catch_unwind` so that panics in spawned tasks
//! are logged with the task name rather than silently aborting the runtime.

use std::future::Future;

use futures_util::FutureExt;
use tracing::Instrument;

/// Spawn a monitored task that catches panics and logs them.
///
/// Use this for long-lived tasks (adapters, tunnel) where a silent panic
/// would leave the system in a degraded state. Short-lived / fire-and-forget
/// tasks can continue using bare `tokio::spawn`.
pub fn spawn_monitored<F>(name: &'static str, future: F) -> tokio::task::JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let span = tracing::info_span!("monitored_task", task = name);
    tokio::spawn(
        async move {
            tracing::debug!("task started");
            // SAFETY: The wrapped future is dropped on panic; no inconsistent
            // state escapes. Callers receive only () from the JoinHandle.
            match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                Ok(()) => {
                    tracing::debug!("task exited (returned normally)");
                }
                Err(e) => {
                    let msg = e
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| e.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("<non-string panic payload>");
                    tracing::error!(panic = msg, "task panicked");
                }
            }
        }
        .instrument(span),
    )
}

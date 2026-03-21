//! Monitored task spawning utility.
//!
//! Wraps `tokio::spawn` with `catch_unwind` so that panics in spawned tasks
//! are logged with the task name rather than silently aborting the runtime.

use std::future::Future;

use futures_util::FutureExt;

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
    tokio::spawn(async move {
        let _guard = span.enter();
        tracing::debug!(task = name, "task started");
        // SAFETY: We do not observe the future's state after a panic — we only
        // log the panic payload and discard the result. This is safe as long as
        // callers don't rely on internal task state after this function returns,
        // which they can't since the JoinHandle output is ().
        match std::panic::AssertUnwindSafe(future).catch_unwind().await {
            Ok(()) => {
                tracing::warn!(task = name, "task exited (returned normally)");
            }
            Err(e) => {
                let msg = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("<non-string panic payload>");
                tracing::error!(task = name, panic = msg, "task panicked");
            }
        }
    })
}

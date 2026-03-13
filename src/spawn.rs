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
    tokio::spawn(async move {
        match std::panic::AssertUnwindSafe(future).catch_unwind().await {
            Ok(()) => {
                tracing::warn!(task = name, "task exited unexpectedly");
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

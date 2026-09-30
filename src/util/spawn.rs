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
/// so the span opened at the agent's root reaches all of its tasks; a test in
/// this module fails if a bare spawn appears elsewhere in the crate.
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

    /// Files that hold only test code, exempt from the bare-spawn scan.
    fn is_test_only_file(path: &std::path::Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("tests.rs"))
    }

    fn rust_files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files_under(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// An agent's tasks must carry its `agent` log field, and `tokio::spawn`
    /// starts a task with no tracing span. Everything in the crate spawns
    /// through the helpers above; this fails when a bare spawn slips back in.
    #[test]
    fn no_bare_spawns_outside_the_span_preserving_helpers() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files_under(&src, &mut files);
        let mut offenders = Vec::new();
        for path in files {
            if path.ends_with(std::path::Path::new("util").join("spawn.rs"))
                || is_test_only_file(&path)
            {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap();
            let production = content.split("#[cfg(test)]").next().unwrap_or_default();
            for (idx, line) in production.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                if code.contains("tokio::spawn(")
                    || code.contains("tokio::task::spawn(")
                    || code.contains("tokio::task::spawn_blocking(")
                    || code.contains("std::thread::spawn(")
                {
                    offenders.push(format!("{}:{}", path.display(), idx + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use crate::util::spawn_in_span / spawn_blocking_in_span so spawned tasks keep the agent span: {offenders:?}"
        );
    }
}

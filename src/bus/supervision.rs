//! Supervision loop for bus infrastructure tasks.
//!
//! Wraps a task factory in a restart loop with exponential backoff.
//! If the task exits (normally or via panic), it is restarted up to
//! `max_failures` consecutive times before the supervisor gives up.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;

/// Maximum backoff duration between restart attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Minimum runtime before resetting the consecutive failure counter.
/// If a task runs for longer than this, the next exit is treated as a
/// fresh first failure rather than a consecutive one.
const HEALTHY_THRESHOLD: Duration = Duration::from_secs(60);

/// Spawn a supervised task that auto-restarts on exit with exponential backoff.
///
/// - `name`: human-readable label for logging.
/// - `factory`: called on each (re)start to produce the task future.
/// - `max_failures`: consecutive failures before the supervisor gives up.
/// - `base_backoff`: initial delay between restarts (doubled each time).
pub fn spawn_supervised<F, Fut>(
    name: &'static str,
    factory: F,
    max_failures: u32,
    base_backoff: Duration,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut consecutive_failures: u32 = 0;

        loop {
            let start = tokio::time::Instant::now();

            tracing::info!(task = name, "starting supervised task");

            // Run the task and catch panics.
            let result = tokio::spawn(factory()).await;

            let elapsed = start.elapsed();

            match result {
                Ok(()) => {
                    tracing::warn!(
                        task = name,
                        elapsed_secs = elapsed.as_secs(),
                        "supervised task exited cleanly"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        task = name,
                        error = %e,
                        elapsed_secs = elapsed.as_secs(),
                        "supervised task panicked"
                    );
                }
            }

            consecutive_failures = if elapsed >= HEALTHY_THRESHOLD {
                0
            } else {
                consecutive_failures + 1
            };

            if consecutive_failures > max_failures {
                tracing::error!(
                    task = name,
                    consecutive_failures,
                    "supervised task exceeded max failures, giving up"
                );
                return;
            }

            let backoff =
                (base_backoff * 2_u32.saturating_pow(consecutive_failures)).min(MAX_BACKOFF);

            tracing::warn!(
                task = name,
                attempt = consecutive_failures,
                backoff_ms = backoff.as_millis(),
                "restarting supervised task after backoff"
            );

            tokio::time::sleep(backoff).await;
        }
    })
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "test code ignores timeout results"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[tokio::test]
    async fn restarts_after_exit() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);

        let handle = spawn_supervised(
            "test-restart",
            move || {
                let c = Arc::clone(&count_clone);
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        // Exit immediately on first two runs to simulate failure.
                        return;
                    }
                    // Third run: stay alive long enough for the test to observe.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            },
            5,
            Duration::from_millis(1),
        );

        // Wait for at least 3 starts.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            count.load(Ordering::SeqCst) >= 3,
            "should have restarted at least twice"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn gives_up_after_max_failures() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);

        let handle = spawn_supervised(
            "test-max-fail",
            move || {
                let c = Arc::clone(&count_clone);
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    // Always exit immediately.
                }
            },
            3,
            Duration::from_millis(1),
        );

        // Wait for supervisor to give up.
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

        let final_count = count.load(Ordering::SeqCst);
        // Should run exactly max_failures + 1 times (initial + 3 retries).
        assert_eq!(final_count, 4, "should run initial + max_failures times");
    }
}

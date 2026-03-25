//! Task spawning utilities: monitored (fire-and-forget with panic logging)
//! and supervised (auto-restart with exponential backoff).

use std::future::Future;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::task::JoinHandle;
use tracing::Instrument;

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
    let span = tracing::info_span!("supervised_task", task = name);
    tokio::spawn(
        async move {
            let mut consecutive_failures: u32 = 0;

            loop {
                let start = tokio::time::Instant::now();

                tracing::debug!(task = name, "starting supervised task");

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

                if consecutive_failures == 1 {
                    tracing::warn!(
                        task = name,
                        attempt = consecutive_failures,
                        backoff_ms = backoff.as_millis(),
                        "restarting supervised task after backoff"
                    );
                } else {
                    tracing::debug!(
                        task = name,
                        attempt = consecutive_failures,
                        backoff_ms = backoff.as_millis(),
                        "restarting supervised task after backoff"
                    );
                }

                tokio::time::sleep(backoff).await;
            }
        }
        .instrument(span),
    )
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "test code ignores timeout results"
)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tokio::sync::Notify;

    use super::*;

    #[tokio::test]
    #[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
    async fn monitored_normal_completion() {
        let handle = spawn_monitored("test-normal", async {});
        handle.await.unwrap();
    }

    #[tokio::test]
    #[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
    #[expect(
        clippy::panic,
        reason = "intentional panic to test that spawn_monitored catches it"
    )]
    async fn monitored_panic_is_swallowed() {
        let handle = spawn_monitored("test-panic", async { panic!("intentional panic") });
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn restarts_after_exit() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);
        let notify = Arc::new(Notify::new());
        let notify_clone = Arc::clone(&notify);

        let handle = spawn_supervised(
            "test-restart",
            move || {
                let c = Arc::clone(&count_clone);
                let n = Arc::clone(&notify_clone);
                async move {
                    let prev = c.fetch_add(1, Ordering::SeqCst);
                    if prev < 2 {
                        return;
                    }
                    n.notify_one();
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            },
            5,
            Duration::from_millis(1),
        );

        notify.notified().await;
        assert!(
            count.load(Ordering::SeqCst) >= 3,
            "should have restarted at least twice"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn supervised_restarts_after_panic() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);
        let notify = Arc::new(Notify::new());
        let notify_clone = Arc::clone(&notify);

        let handle = spawn_supervised(
            "test-panic-restart",
            move || {
                let c = Arc::clone(&count_clone);
                let n = Arc::clone(&notify_clone);
                async move {
                    let prev = c.fetch_add(1, Ordering::SeqCst);
                    assert!(prev != 0, "intentional panic on first run");
                    n.notify_one();
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            },
            5,
            Duration::from_millis(1),
        );

        notify.notified().await;
        assert!(
            count.load(Ordering::SeqCst) >= 2,
            "supervisor should have restarted after panic"
        );
        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_threshold_resets_failure_counter() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);
        let notify = Arc::new(Notify::new());
        let notify_clone = Arc::clone(&notify);

        // max_failures=1: without a healthy run the supervisor gives up after 2 starts.
        // Run 1 stays alive past HEALTHY_THRESHOLD so the counter resets, allowing run 2.
        let handle = spawn_supervised(
            "test-threshold",
            move || {
                let c = Arc::clone(&count_clone);
                let n = Arc::clone(&notify_clone);
                async move {
                    let run = c.fetch_add(1, Ordering::SeqCst);
                    if run == 1 {
                        tokio::time::sleep(HEALTHY_THRESHOLD + Duration::from_secs(1)).await;
                    }
                    if run == 2 {
                        // Signal that the reset worked; stay alive so the supervisor doesn't
                        // immediately count another failure before we assert.
                        n.notify_one();
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                }
            },
            1,
            Duration::from_millis(1),
        );

        // Each advance yields once to let woken tasks run. Sleep deadlines are set
        // relative to the mock clock at the moment the task executes, so run 1's
        // sleep (started after the first big advance) needs a second large advance.
        tokio::time::advance(Duration::from_millis(10)).await; // run 0 done; 2ms backoff pending
        tokio::time::advance(Duration::from_millis(10)).await; // 2ms backoff fires; run 1 starts
        tokio::time::advance(HEALTHY_THRESHOLD + Duration::from_secs(2)).await; // run 1 sleep fires; 1ms backoff set
        tokio::time::advance(Duration::from_millis(10)).await; // 1ms backoff fires; run 2 starts → notify

        // notify_one() is stored; notified() returns immediately if already signalled.
        notify.notified().await;
        assert!(
            count.load(Ordering::SeqCst) >= 3,
            "healthy threshold reset should allow supervisor to start run 2"
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

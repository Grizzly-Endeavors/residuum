use std::time::Duration;

use residuum_bus::Publisher;
use residuum_bus::notice::publish_notice;

const BASE_BACKOFF: Duration = Duration::from_secs(2);

const MAX_BACKOFF: Duration = Duration::from_secs(30);

const MAX_ATTEMPTS: u32 = 10;

pub(crate) async fn retry_connect<F, Fut, T, E>(
    publisher: &Publisher,
    adapter: &str,
    mut connect: F,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::IntoFuture<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt: u32 = 0;
    loop {
        match connect().await {
            Ok(value) => {
                if attempt > 0 {
                    tracing::warn!(
                        adapter,
                        attempts = attempt,
                        "adapter connected after retrying"
                    );
                    publish_notice(
                        publisher,
                        format!("{adapter} reconnected after {attempt} attempt(s) — it's back up."),
                    )
                    .await;
                }
                return Some(value);
            }
            Err(err) => {
                attempt += 1;
                if attempt == 1 {
                    tracing::warn!(
                        adapter,
                        error = %err,
                        "adapter failed to connect at startup, retrying with backoff"
                    );
                    publish_notice(
                        publisher,
                        format!(
                            "{adapter} couldn't connect ({err}). Retrying in the background — it'll come back on its own once the problem clears."
                        ),
                    )
                    .await;
                } else {
                    tracing::debug!(adapter, attempt, error = %err, "adapter still failing to connect");
                }

                if attempt >= MAX_ATTEMPTS {
                    tracing::error!(
                        adapter,
                        attempts = attempt,
                        error = %err,
                        "adapter gave up connecting after repeated failures"
                    );
                    publish_notice(
                        publisher,
                        format!(
                            "{adapter} couldn't connect after {attempt} attempts ({err}) and has given up. It stays down until you fix the problem, then reload or restart."
                        ),
                    )
                    .await;
                    return None;
                }

                let backoff = BASE_BACKOFF
                    .saturating_mul(2_u32.saturating_pow(attempt - 1))
                    .min(MAX_BACKOFF);
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_publisher() -> (Publisher, residuum_bus::BusHandle) {
        let bus = residuum_bus::spawn_broker();
        (bus.publisher(), bus)
    }

    #[tokio::test]
    async fn succeeds_immediately_without_retrying() {
        let (publisher, _rx) = test_publisher();
        let calls = AtomicU32::new(0);
        let result: Option<u32> = retry_connect(&publisher, "test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<u32, String>(42) }
        })
        .await;
        assert_eq!(result, Some(42));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should not retry on success"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retries_then_succeeds() {
        let (publisher, _rx) = test_publisher();
        let calls = AtomicU32::new(0);
        let result: Option<u32> = retry_connect(&publisher, "test", || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err("not yet".to_string())
                } else {
                    Ok(99)
                }
            }
        })
        .await;
        assert_eq!(result, Some(99));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "should retry until success"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_after_max_attempts() {
        let (publisher, _rx) = test_publisher();
        let calls = AtomicU32::new(0);
        let result: Option<u32> = retry_connect(&publisher, "test", || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<u32, String>("always fails".to_string()) }
        })
        .await;
        assert_eq!(result, None, "should give up eventually");
        assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS);
    }
}

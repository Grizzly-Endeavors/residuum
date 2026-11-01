//! Retry logic with exponential backoff for model provider calls.

use std::time::Duration;

use tokio::time::sleep;
use tracing::{debug, warn};

use super::InferenceError;
use crate::attempt::ListsAttempts;
use residuum_config::RetryConfig;

/// Execute a fallible async operation with retry logic.
///
/// Only retries if the error is marked as retryable via [`InferenceError::is_retryable()`].
/// The result lists every attempt the failed tries recorded ahead of its own.
///
/// # Errors
/// Returns the last error if all retries are exhausted or if a non-retryable error occurs.
#[tracing::instrument(skip_all, fields(max_retries = config.max_retries))]
pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T, InferenceError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, InferenceError>>,
    T: ListsAttempts,
{
    let mut attempts = 0;
    let mut delay = config.initial_delay;
    let mut spent = Vec::new();

    loop {
        let (e, tried) = match operation().await {
            Ok(result) => return Ok(result.after_attempts(spent)),
            Err(e) => e.into_parts(),
        };
        spent.extend(tried);
        if !e.is_retryable() {
            return Err(e.after_attempts(spent));
        }
        if attempts >= config.max_retries {
            warn!(
                attempts = attempts + 1,
                error = %e,
                "max retries exceeded"
            );
            return Err(e.after_attempts(spent));
        }
        if attempts == 0 {
            warn!(
                max_retries = config.max_retries,
                error = %e,
                "transient error, retrying"
            );
        } else {
            debug!(
                attempt = attempts + 1,
                max_retries = config.max_retries,
                error = %e,
                "retry attempt"
            );
        }
        attempts += 1;

        // Add jitter (+-25%)
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for jitter calculation"
        )]
        let jitter = delay.as_millis() as f64 * (rand::random::<f64>() * 0.5 - 0.25);
        #[expect(
            clippy::cast_precision_loss,
            reason = "precision loss acceptable for duration calculation"
        )]
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "jittered delay is always positive and fits in u64"
        )]
        let jittered_delay = Duration::from_millis((delay.as_millis() as f64 + jitter) as u64);
        sleep(jittered_delay).await;

        // Exponential backoff
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "backoff duration fits in u64 range"
        )]
        let next_delay =
            Duration::from_millis((delay.as_millis() as f64 * config.backoff_multiplier) as u64);
        delay = next_delay.min(config.max_delay);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::InferenceResponse;
    use crate::Usage;
    use crate::attempt::{FailedRequest, record_attempt};
    use crate::types::{AttemptOutcome, AttemptUsage};
    use residuum_config::ProviderKind;

    fn fast_retry() -> RetryConfig {
        RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            backoff_multiplier: 1.0,
        }
    }

    fn output(tokens: u32) -> Usage {
        Usage {
            output_tokens: Some(tokens),
            ..Usage::default()
        }
    }

    #[tokio::test]
    async fn a_retried_call_lists_the_failed_attempt_and_the_success() {
        let call_count = Arc::new(AtomicU32::new(0));
        let result = with_retry(&fast_retry(), || {
            let count = Arc::clone(&call_count);
            async move {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    let failed = FailedRequest::spent(
                        InferenceError::StreamInterrupted("reset".to_string()),
                        Some(output(3)),
                    );
                    return record_attempt(ProviderKind::Anthropic, "claude", Err(failed));
                }
                let mut response = InferenceResponse::new("done".to_string(), vec![]);
                response.usage = Some(output(9));
                record_attempt(ProviderKind::Anthropic, "claude", Ok(response))
            }
        })
        .await
        .unwrap();

        let attempt = |outcome, usage| AttemptUsage {
            provider: "anthropic".to_string(),
            model: "claude".to_string(),
            outcome,
            usage,
        };
        assert_eq!(
            result.attempts,
            vec![
                attempt(AttemptOutcome::Failed, Some(output(3))),
                attempt(AttemptOutcome::Succeeded, Some(output(9))),
            ]
        );
        assert_eq!(
            result.usage,
            Some(output(9)),
            "usage stays the success's own"
        );
    }

    #[tokio::test]
    async fn a_call_that_exhausts_its_retries_carries_every_attempt() {
        let config = RetryConfig {
            max_retries: 2,
            ..fast_retry()
        };
        let err = with_retry(&config, || async {
            record_attempt(
                ProviderKind::OpenAi,
                "gpt",
                Err(InferenceError::Api("503 Service Unavailable: busy".to_string()).into()),
            )
        })
        .await
        .unwrap_err();

        assert_eq!(err.attempts().len(), 3, "initial + 2 retries: {err:?}");
        assert!(
            err.attempts()
                .iter()
                .all(|a| a.outcome == AttemptOutcome::Failed && a.usage.is_none()),
            "every attempt failed and reported nothing: {err:?}"
        );
        assert!(
            matches!(err.cause(), InferenceError::Api(_)),
            "the cause is the last error"
        );
    }

    #[tokio::test]
    async fn succeeds_immediately() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig::default();
        let result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, InferenceError>(InferenceResponse::new("success".to_string(), vec![]))
            }
        })
        .await;

        assert!(result.is_ok(), "should succeed on first attempt");
        assert_eq!(
            result.unwrap().content,
            "success",
            "value should pass through"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "should only be called once"
        );
    }

    #[tokio::test]
    async fn retries_on_retryable_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
        };

        let result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                let current = count.fetch_add(1, Ordering::SeqCst);
                if current < 2 {
                    Err(InferenceError::Api("rate limit exceeded".to_string()))
                } else {
                    Ok::<_, InferenceError>(InferenceResponse::new("success".to_string(), vec![]))
                }
            }
        })
        .await;

        assert!(result.is_ok(), "should succeed after retries");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "should be called 3 times"
        );
    }

    #[tokio::test]
    async fn no_retry_on_permanent_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
        };

        let result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<InferenceResponse, _>(InferenceError::Parse("invalid json".to_string()))
            }
        })
        .await;

        assert!(result.is_err(), "should fail immediately");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "should only be called once for non-retryable"
        );
    }

    #[tokio::test]
    async fn max_retries_respected() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
        };

        let result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<InferenceResponse, _>(InferenceError::Api("rate limit exceeded".to_string()))
            }
        })
        .await;

        assert!(result.is_err(), "should fail after exhausting retries");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "initial + 2 retries = 3 calls"
        );
    }

    #[tokio::test]
    async fn no_retry_config() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig::no_retry();
        let result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<InferenceResponse, _>(InferenceError::Api("rate limit exceeded".to_string()))
            }
        })
        .await;

        assert!(result.is_err(), "should fail immediately with no_retry");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "should only be called once"
        );
    }

    #[tokio::test]
    async fn exponential_backoff_timing() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        let config = RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        };

        let start = Instant::now();
        let _result = with_retry(&config, || {
            let count = Arc::clone(&cc);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<InferenceResponse, _>(InferenceError::Api("rate limit exceeded".to_string()))
            }
        })
        .await;

        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(80),
            "elapsed {elapsed:?} should include backoff delays"
        );
        assert!(
            elapsed <= Duration::from_secs(1),
            "elapsed {elapsed:?} should not have excessive delays"
        );
    }

    #[test]
    fn retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3, "default max_retries");
        assert_eq!(
            config.initial_delay,
            Duration::from_millis(500),
            "default initial_delay"
        );
        assert_eq!(
            config.max_delay,
            Duration::from_secs(30),
            "default max_delay"
        );
        assert!(
            (config.backoff_multiplier - 2.0).abs() < f64::EPSILON,
            "default multiplier"
        );
    }

    #[test]
    fn retry_config_no_retry() {
        let config = RetryConfig::no_retry();
        assert_eq!(config.max_retries, 0, "no_retry should have 0 retries");
    }
}

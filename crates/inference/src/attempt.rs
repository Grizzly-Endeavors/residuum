//! Recording every request a model call sends, so the usage of attempts that
//! failed (a retry, a failover hop, a stream that broke after reporting its
//! usage) is not lost.
//!
//! Each provider client records the one request it sends with
//! [`record_attempt`]; everything that sends more than one request for a call
//! (retry, failover, the providers' own resend-on-refusal paths) puts the
//! attempts made so far ahead of the next result with
//! [`ListsAttempts::after_attempts`].

use residuum_config::ProviderKind;

use crate::embedding::EmbeddingResponse;
use crate::types::{AttemptOutcome, AttemptUsage};
use crate::{InferenceError, InferenceResponse, Usage};

/// One request that failed, with the usage the provider reported before it
/// did: a stream that broke after its usage arrived, or a response whose
/// body was read but could not be used.
#[derive(Debug)]
pub(crate) struct FailedRequest {
    pub(crate) error: InferenceError,
    pub(crate) usage: Option<Usage>,
}

impl FailedRequest {
    /// A request that failed with `error` after the provider reported `usage`.
    pub(crate) fn spent(error: InferenceError, usage: Option<Usage>) -> Self {
        Self { error, usage }
    }
}

impl From<InferenceError> for FailedRequest {
    fn from(error: InferenceError) -> Self {
        Self { error, usage: None }
    }
}

/// Record one request sent to `model` at `provider`: a response lists it as
/// its one attempt, and an error carries it with the usage reported before
/// the failure.
pub(crate) fn record_attempt(
    provider: ProviderKind,
    model: &str,
    result: Result<InferenceResponse, FailedRequest>,
) -> Result<InferenceResponse, InferenceError> {
    let attempt = |outcome: AttemptOutcome, usage: Option<Usage>| AttemptUsage {
        provider: provider.to_string(),
        model: model.to_string(),
        outcome,
        usage,
    };
    match result {
        Ok(mut response) => {
            response.attempts = vec![attempt(AttemptOutcome::Succeeded, response.usage)];
            Ok(response)
        }
        Err(failed) => Err(failed
            .error
            .after_attempts(vec![attempt(AttemptOutcome::Failed, failed.usage)])),
    }
}

/// A result that lists the requests a call sent.
pub(crate) trait ListsAttempts {
    /// This result, with `earlier` attempts listed ahead of its own.
    #[must_use]
    fn after_attempts(self, earlier: Vec<AttemptUsage>) -> Self;
}

impl ListsAttempts for InferenceResponse {
    fn after_attempts(mut self, mut earlier: Vec<AttemptUsage>) -> Self {
        earlier.append(&mut self.attempts);
        self.attempts = earlier;
        self
    }
}

/// Embedding requests record no attempts, so there are never earlier ones to
/// list.
impl ListsAttempts for EmbeddingResponse {
    fn after_attempts(self, _earlier: Vec<AttemptUsage>) -> Self {
        self
    }
}

impl<T: ListsAttempts> ListsAttempts for Result<T, InferenceError> {
    fn after_attempts(self, earlier: Vec<AttemptUsage>) -> Self {
        match self {
            Ok(value) => Ok(value.after_attempts(earlier)),
            Err(error) => Err(error.after_attempts(earlier)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(output: u32) -> Usage {
        Usage {
            output_tokens: Some(output),
            ..Usage::default()
        }
    }

    #[test]
    fn a_response_lists_its_request_as_the_successful_attempt() {
        let mut response = InferenceResponse::new("hi".to_string(), vec![]);
        response.usage = Some(usage(5));
        let response = record_attempt(ProviderKind::Fireworks, "m", Ok(response)).unwrap();
        assert_eq!(
            response.attempts,
            vec![AttemptUsage {
                provider: "fireworks".to_string(),
                model: "m".to_string(),
                outcome: AttemptOutcome::Succeeded,
                usage: Some(usage(5)),
            }]
        );
    }

    #[test]
    fn a_failed_request_keeps_the_usage_reported_before_it_failed() {
        let failed = FailedRequest::spent(
            InferenceError::StreamInterrupted("reset".to_string()),
            Some(usage(7)),
        );
        let err = record_attempt(ProviderKind::Anthropic, "claude", Err(failed)).unwrap_err();
        assert!(
            matches!(err.cause(), InferenceError::StreamInterrupted(_)),
            "the cause stays the stream error: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "the response stream was interrupted: reset",
            "the recorded error reads as its cause"
        );
        assert_eq!(
            err.attempts(),
            [AttemptUsage {
                provider: "anthropic".to_string(),
                model: "claude".to_string(),
                outcome: AttemptOutcome::Failed,
                usage: Some(usage(7)),
            }]
        );
        assert!(
            err.is_retryable(),
            "classification looks through the record"
        );
    }

    #[test]
    fn earlier_attempts_go_ahead_of_a_results_own() {
        let first = record_attempt(
            ProviderKind::OpenAi,
            "a",
            Err(InferenceError::Timeout(1).into()),
        )
        .unwrap_err();
        let second = record_attempt(
            ProviderKind::OpenAi,
            "b",
            Err(InferenceError::Timeout(2).into()),
        )
        .unwrap_err();
        let (_, earlier) = first.into_parts();
        let combined = second.after_attempts(earlier);
        let models: Vec<&str> = combined
            .attempts()
            .iter()
            .map(|a| a.model.as_str())
            .collect();
        assert_eq!(models, ["a", "b"]);
        assert!(
            !matches!(combined.cause(), InferenceError::Attempted { .. }),
            "records never nest"
        );
        assert!(
            matches!(combined.cause(), InferenceError::Timeout(2)),
            "the last error is the cause"
        );
    }

    #[test]
    fn an_error_with_no_attempts_stays_bare() {
        let err = InferenceError::Parse("x".to_string()).after_attempts(Vec::new());
        assert!(matches!(err, InferenceError::Parse(_)));
    }
}

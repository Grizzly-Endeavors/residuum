//! Errors returned by inference provider operations.

use thiserror::Error;

/// Errors from model provider operations.
#[derive(Error, Debug)]
pub enum InferenceError {
    /// HTTP request failed (network, DNS, TLS)
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// Response could not be parsed
    #[error("failed to parse response: {0}")]
    Parse(String),

    /// API returned an error status
    #[error("API error: {0}")]
    Api(String),

    /// Request timed out
    #[error("request timed out after {0} seconds")]
    Timeout(u64),
}

impl InferenceError {
    /// Whether this error is likely to succeed on retry.
    ///
    /// - Request/Timeout: transient network failures
    /// - Parse: permanent -- malformed response won't improve
    /// - Api: retryable only when the message indicates rate-limiting or overload
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Request(_) | Self::Timeout(_) => true,
            Self::Parse(_) => false,
            Self::Api(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("rate")
                    || lower.contains("limit")
                    || lower.contains("overload")
                    || lower.contains("capacity")
                    || lower.contains("429")
                    || lower.contains("500")
                    || lower.contains("502")
                    || lower.contains("503")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_error_display_parse() {
        let err = InferenceError::Parse("bad json".to_string());
        assert_eq!(
            err.to_string(),
            "failed to parse response: bad json",
            "parse error should include context"
        );
    }

    #[test]
    fn inference_error_display_timeout() {
        let err = InferenceError::Timeout(60);
        assert_eq!(
            err.to_string(),
            "request timed out after 60 seconds",
            "timeout should show duration"
        );
    }

    #[test]
    fn inference_error_is_retryable_transient() {
        assert!(
            InferenceError::Timeout(60).is_retryable(),
            "timeout should be retryable"
        );

        assert!(
            InferenceError::Api("rate limit exceeded".to_string()).is_retryable(),
            "rate limit should be retryable"
        );

        assert!(
            InferenceError::Api("Error 429: too many requests".to_string()).is_retryable(),
            "429 should be retryable"
        );

        assert!(
            InferenceError::Api(
                "anthropic api error 500 Internal Server Error: Internal server error".to_string()
            )
            .is_retryable(),
            "500 should be retryable"
        );
    }

    #[test]
    fn inference_error_is_retryable_permanent() {
        assert!(
            !InferenceError::Parse("invalid json".to_string()).is_retryable(),
            "parse error should not be retryable"
        );

        assert!(
            !InferenceError::Api("invalid api key".to_string()).is_retryable(),
            "auth error should not be retryable"
        );
    }
}

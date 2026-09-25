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

    /// Classify this error into a plain-language cause, for a non-technical
    /// user who has no reason to know what an HTTP status code or a
    /// provider's own error string means.
    fn category(&self) -> FailureCategory {
        match self {
            Self::Timeout(_) => FailureCategory::Timeout,
            Self::Request(_) => FailureCategory::NetworkUnreachable,
            Self::Parse(_) => FailureCategory::Unclassified,
            Self::Api(msg) => FailureCategory::from_api_message(msg),
        }
    }

    /// Plain-language sentence naming what went wrong and what to do next.
    /// Never exposes a raw status code, error type, or module path — see
    /// `CLAUDE.md`'s "No Silent Failures".
    #[must_use]
    pub fn user_message(&self) -> String {
        self.category().user_message(self)
    }

    /// Short plain-language fragment naming the cause, for embedding into a
    /// longer sentence (a failover notice: "main model unavailable
    /// ({cause}), using ..."). Unlike [`Self::user_message`] this carries no
    /// "what to do next" — the caller supplies that context.
    #[must_use]
    pub(crate) fn cause_phrase(&self) -> &'static str {
        self.category().cause_phrase()
    }
}

/// A plain-language bucket for an [`InferenceError`], independent of which
/// provider produced it. Providers today collapse their own status code and
/// error body into `InferenceError::Api`'s single string (see
/// `src/inference/providers/`), so classification matches on that string —
/// the same substring-matching approach `InferenceError::is_retryable`
/// already uses for the same reason.
enum FailureCategory {
    AuthFailed,
    RateLimited,
    ContextTooLong,
    ModelNotFound,
    ProviderOutage,
    NetworkUnreachable,
    Timeout,
    Unclassified,
}

impl FailureCategory {
    /// Classify a provider's collapsed `"{status}: {body}"` (or similar)
    /// error string. Order matters: more specific matches are checked
    /// before generic ones (e.g. a 400 naming the context limit is checked
    /// before a bare "4" status range that would otherwise fall through to
    /// "unclassified").
    fn from_api_message(msg: &str) -> Self {
        let lower = msg.to_lowercase();
        let has_status = |codes: &[&str]| codes.iter().any(|c| lower.contains(c));

        if has_status(&["401", "403"])
            || lower.contains("invalid api key")
            || lower.contains("invalid x-api-key")
            || lower.contains("authentication_error")
            || lower.contains("authentication failed")
            || lower.contains("incorrect api key")
        {
            Self::AuthFailed
        } else if has_status(&["429"])
            || lower.contains("rate_limit")
            || lower.contains("rate limit")
            || lower.contains("quota")
        {
            Self::RateLimited
        } else if lower.contains("context_length_exceeded")
            || lower.contains("maximum context length")
            || lower.contains("context window")
            || lower.contains("too many tokens")
            || lower.contains("prompt is too long")
        {
            Self::ContextTooLong
        } else if lower.contains("model_not_found")
            || lower.contains("no such model")
            || lower.contains("does not exist")
            || (lower.contains("model") && lower.contains("not found"))
        {
            Self::ModelNotFound
        } else if has_status(&["500", "502", "503", "504"])
            || lower.contains("overloaded_error")
            || lower.contains("overload")
            || lower.contains("capacity")
            || lower.contains("internal server error")
            || lower.contains("bad gateway")
            || lower.contains("service unavailable")
        {
            Self::ProviderOutage
        } else {
            Self::Unclassified
        }
    }

    /// Short fragment naming the cause, for a sentence the caller builds.
    fn cause_phrase(&self) -> &'static str {
        match self {
            Self::AuthFailed => "an authentication failure",
            Self::RateLimited => "rate limiting",
            Self::ContextTooLong => "the conversation exceeding the model's context limit",
            Self::ModelNotFound => "the configured model being unavailable",
            Self::ProviderOutage => "a server error at the provider",
            Self::NetworkUnreachable => "a network problem",
            Self::Timeout => "a timeout",
            Self::Unclassified => "an error talking to the AI provider",
        }
    }

    /// Full plain-language sentence: what happened, and what to do next.
    /// `err` supplies the one category (`Timeout`) whose sentence needs a
    /// value out of the error itself.
    fn user_message(&self, err: &InferenceError) -> String {
        match self {
            Self::AuthFailed => "Residuum couldn't authenticate with the AI provider. Check \
                that the API key in Settings is correct and hasn't expired."
                .to_string(),
            Self::RateLimited => "The AI provider is rate-limiting requests right now. Wait a \
                moment and try again, or check your usage limits with the provider."
                .to_string(),
            Self::ContextTooLong => "The conversation is too long for the model's context \
                window. Start a new conversation or shorten this one, then try again."
                .to_string(),
            Self::ModelNotFound => "The configured model isn't available from the provider. \
                Check the model name in Settings."
                .to_string(),
            Self::ProviderOutage => {
                "The AI provider is having problems on its end. Try again shortly.".to_string()
            }
            Self::NetworkUnreachable => "Residuum couldn't reach the AI provider. Check your \
                internet connection and try again."
                .to_string(),
            Self::Timeout => {
                if let InferenceError::Timeout(secs) = err {
                    format!(
                        "The request to the AI provider timed out after {secs} seconds. Try \
                         again — if it keeps happening, the provider may be slow or overloaded."
                    )
                } else {
                    "The request to the AI provider timed out. Try again — if it keeps \
                     happening, the provider may be slow or overloaded."
                        .to_string()
                }
            }
            Self::Unclassified => "Something went wrong talking to the AI provider. Try again; \
                if it keeps happening, check Residuum's logs for details."
                .to_string(),
        }
    }
}

/// A turn failure described for two different audiences: `message` is
/// plain language for the user (never a raw error type or status code, per
/// `CLAUDE.md`'s "No Silent Failures"), and `details` is the full technical
/// cause chain for a developer or an expandable "details" view.
#[derive(Debug, Clone)]
pub struct FailureDescription {
    /// Plain-language cause and next step.
    pub message: String,
    /// Full technical chain (`anyhow`'s alternate `{:#}` formatting).
    pub details: String,
}

/// Describe a turn failure for both audiences at once: a plain-language
/// message for the user, classified from the originating [`InferenceError`]
/// when the failure came from a model call, and the full technical chain
/// for a details view or the logs.
///
/// Walks `err`'s cause chain rather than requiring the caller to classify
/// before wrapping it in `.context(...)` — `anyhow::Error`'s `Display`
/// (what `.to_string()` gives you) only shows the outermost context, so the
/// originating error's detail would otherwise never reach the user.
#[must_use]
pub fn describe_turn_failure(err: &anyhow::Error) -> FailureDescription {
    let details = format!("{err:#}");
    let message = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<InferenceError>())
        .map_or_else(
            || {
                "Something went wrong while the agent was working. Try again; if it keeps \
                 happening, check Residuum's logs for details."
                    .to_string()
            },
            InferenceError::user_message,
        );
    FailureDescription { message, details }
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

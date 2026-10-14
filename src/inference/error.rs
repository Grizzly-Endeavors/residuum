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

    /// A streaming response went quiet: no bytes arrived for this many seconds
    #[error("the model stopped responding for {0}s")]
    Stalled(u64),

    /// A streaming response ended or broke before it was complete
    #[error("the response stream was interrupted: {0}")]
    StreamInterrupted(String),
}

impl InferenceError {
    /// Whether this error is likely to succeed on retry.
    ///
    /// - Request/Timeout/Stalled/StreamInterrupted: transient network failures
    /// - Parse: permanent -- malformed response won't improve
    /// - Api: by its HTTP status when the message names one (a timeout, a
    ///   rate limit or a server error); otherwise when the message indicates
    ///   rate-limiting, overload or a server-side failure
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Request(_) | Self::Timeout(_) | Self::Stalled(_) | Self::StreamInterrupted(_) => {
                true
            }
            Self::Parse(_) => false,
            Self::Api(msg) => {
                let lower = msg.to_lowercase();
                if let Some(status) = http_status(&lower) {
                    return status == 408 || status == 429 || status >= 500;
                }
                lower.contains("rate")
                    || lower.contains("limit")
                    || lower.contains("overload")
                    || lower.contains("capacity")
                    || has_server_error_type(&lower)
            }
        }
    }

    /// Classify this error into a plain-language cause, for a non-technical
    /// user who has no reason to know what an HTTP status code or a
    /// provider's own error string means.
    fn category(&self) -> FailureCategory {
        match self {
            Self::Timeout(_) | Self::Stalled(_) => FailureCategory::Timeout,
            Self::StreamInterrupted(_) => FailureCategory::ConnectionLost,
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

    /// Whether the provider turned the request down because it carried
    /// image content the model can't take. Matches only the rejections
    /// listed in [`IMAGE_INPUT_REJECTIONS`]; any other error, a `400`
    /// included, is not one.
    #[must_use]
    pub(crate) fn rejects_image_input(&self) -> bool {
        let Self::Api(msg) = self else {
            return false;
        };
        let lower = msg.to_lowercase();
        IMAGE_INPUT_REJECTIONS
            .iter()
            .any(|rejection| lower.contains(rejection))
    }
}

/// Lowercased error text a provider returns when a request carries image
/// content its model can't take. Each entry is a rejection a real provider
/// returned, named beside it; add a shape only once it has been seen.
const IMAGE_INPUT_REJECTIONS: &[&str] = &[
    // Z.ai's OpenAI-compatible API serving the text-only glm-5.3, reported
    // in issue #463: `messages.content.type is invalid`.
    "messages.content.type is invalid",
    // OpenAI Chat Completions with a model that has no vision: `Invalid
    // content type. image_url is only supported by certain models.`
    "image_url is only supported by certain models",
    // DeepSeek's API, whose content parts are text only: `Failed to
    // deserialize the JSON body into the target type: messages[1]: unknown
    // variant `image_url`, expected `text``.
    "unknown variant `image_url`",
];

/// The HTTP status a lowercased provider error names, read only where the
/// clients put it: at the very start (`"429 too many requests: ..."`,
/// Gemini's `"503 unavailable: ..."`) or right after `"error "`
/// (`"anthropic api error 500 ..."`). Digits elsewhere, such as the column
/// number in DeepSeek's `"... at line 1 column 15003"`, are not a status.
fn http_status(lower: &str) -> Option<u16> {
    let after_error = lower
        .find("error ")
        .and_then(|at| lower.get(at + "error ".len()..));
    [Some(lower), after_error]
        .into_iter()
        .flatten()
        .find_map(leading_status)
}

/// A three-digit status at the start of `text`, followed by anything but
/// another digit.
fn leading_status(text: &str) -> Option<u16> {
    let digits = text.get(..3)?;
    let ends = text
        .chars()
        .nth(3)
        .is_none_or(|next| !next.is_ascii_digit());
    if !ends || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|status| (100..600).contains(status))
}

/// Whether a lowercased provider error names a server-side failure by its
/// error type rather than an HTTP status. A streaming response reports a
/// failure after its `200` status has gone out, so the error type
/// (Anthropic's `api_error`, `OpenAI`'s `server_error`) is all there is to
/// classify it by.
fn has_server_error_type(lower: &str) -> bool {
    lower.contains("api_error") || lower.contains("server_error")
}

/// A plain-language bucket for an [`InferenceError`], independent of which
/// provider produced it. Providers collapse their own status code and
/// error body into `InferenceError::Api`'s single string (see
/// `src/inference/providers/`), so classification reads the status where the
/// clients put it (see [`http_status`]) and matches the rest of that string.
enum FailureCategory {
    AuthFailed,
    RateLimited,
    ContextTooLong,
    ModelNotFound,
    ProviderOutage,
    NetworkUnreachable,
    ConnectionLost,
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
        let status = http_status(&lower);
        let has_status = |codes: &[u16]| status.is_some_and(|status| codes.contains(&status));

        if has_status(&[401, 403])
            || lower.contains("invalid api key")
            || lower.contains("invalid x-api-key")
            || lower.contains("authentication_error")
            || lower.contains("authentication failed")
            || lower.contains("incorrect api key")
        {
            Self::AuthFailed
        } else if has_status(&[429])
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
        } else if has_status(&[500, 502, 503, 504])
            || has_server_error_type(&lower)
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
            Self::ConnectionLost => "a dropped connection",
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
                that the API key in Settings → Model is correct and hasn't expired."
                .to_string(),
            Self::RateLimited => "The AI provider is rate-limiting requests right now. Wait a \
                moment and try again, or check your usage limits with the provider."
                .to_string(),
            Self::ContextTooLong => "The conversation is too long for the model's context \
                window. Start a new conversation or shorten this one, then try again."
                .to_string(),
            Self::ModelNotFound => "The configured model isn't available from the provider. \
                Check the model name in Settings → Model."
                .to_string(),
            Self::ProviderOutage => {
                "The AI provider is having problems on its end. Try again shortly.".to_string()
            }
            Self::NetworkUnreachable => "Residuum couldn't reach the AI provider. Check your \
                internet connection and try again."
                .to_string(),
            Self::ConnectionLost => "The connection to the AI provider dropped before the answer \
                finished. Try again."
                .to_string(),
            Self::Timeout => {
                if let InferenceError::Timeout(secs) = err {
                    format!(
                        "The request to the AI provider timed out after {secs} seconds. Try \
                         again — if it keeps happening, the provider may be slow or overloaded."
                    )
                } else if let InferenceError::Stalled(secs) = err {
                    format!(
                        "The AI provider stopped responding for {secs} seconds before it \
                         finished its answer. Try again — if it keeps happening, the provider \
                         may be slow or overloaded."
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

    #[test]
    fn stalled_stream_reads_plainly_and_is_retryable() {
        let err = InferenceError::Stalled(60);
        assert_eq!(
            err.to_string(),
            "the model stopped responding for 60s",
            "a stalled stream names how long it was quiet"
        );
        assert!(err.is_retryable(), "a stalled stream is worth retrying");
        assert!(
            err.user_message().contains("60 seconds"),
            "the user is told how long the provider was quiet: {}",
            err.user_message()
        );
    }

    #[test]
    fn interrupted_stream_is_retryable_and_described_for_the_user() {
        let err = InferenceError::StreamInterrupted("connection reset".to_string());
        assert!(err.is_retryable(), "a dropped stream is worth retrying");
        assert!(
            err.user_message().contains("dropped"),
            "the user is told the connection dropped: {}",
            err.user_message()
        );
        assert_eq!(
            err.cause_phrase(),
            "a dropped connection",
            "a failover notice names the dropped connection"
        );
    }

    #[test]
    fn in_stream_server_error_types_are_retryable() {
        assert!(
            InferenceError::Api("anthropic stream error (api_error): Internal server error".into())
                .is_retryable(),
            "Anthropic's api_error is a server-side failure"
        );
        assert!(
            InferenceError::Api("server_error: The server had an error".into()).is_retryable(),
            "OpenAI's server_error is a server-side failure"
        );
        assert!(
            InferenceError::Api("anthropic stream error (overloaded_error): Overloaded".into())
                .is_retryable(),
            "overloaded_error is retryable"
        );
        assert!(
            !InferenceError::Api(
                "anthropic stream error (invalid_request_error): bad input".into()
            )
            .is_retryable(),
            "a rejected request is not retryable"
        );
    }

    #[test]
    fn digits_inside_the_body_are_not_a_status() {
        let err = InferenceError::Api(
            "400 Bad Request: Failed to deserialize the JSON body into the target type: \
             messages[1]: unknown variant `image_url`, expected `text` at line 1 column 15003"
                .to_string(),
        );
        assert!(
            !err.is_retryable(),
            "a 400 whose column number holds 500 is not a server error"
        );
        assert_eq!(err.cause_phrase(), "an error talking to the AI provider");

        let quota = InferenceError::Api("400 Bad Request: token 4290 is invalid".to_string());
        assert!(
            !quota.is_retryable(),
            "429 inside a number is not a rate limit"
        );
    }

    #[test]
    fn the_status_decides_where_the_message_names_one() {
        for (message, retryable) in [
            ("429 Too Many Requests: slow down", true),
            ("503 Service Unavailable: try later", true),
            ("529 <unknown status code>: overloaded", true),
            ("408 Request Timeout: took too long", true),
            ("anthropic api error 500 Internal Server Error: oops", true),
            ("Error 429: too many requests", true),
            ("400 Bad Request: the limit on tools is 128", false),
            ("401 Unauthorized: invalid api key", false),
            (
                "anthropic api error 404 Not Found (not_found_error): model",
                false,
            ),
        ] {
            assert_eq!(
                InferenceError::Api(message.to_string()).is_retryable(),
                retryable,
                "{message}"
            );
        }
    }

    #[test]
    fn image_input_rejections_are_recognised() {
        for body in [
            r#"400 Bad Request: {"error":{"code":"1214","message":"messages.content.type is invalid"}}"#,
            r#"400 Bad Request: {"error":{"message":"Invalid content type. image_url is only supported by certain models.","type":"invalid_request_error"}}"#,
            "400 Bad Request: Failed to deserialize the JSON body into the target type: \
             messages[1]: unknown variant `image_url`, expected `text` at line 1 column 312",
        ] {
            assert!(
                InferenceError::Api(body.to_string()).rejects_image_input(),
                "should recognise an image rejection: {body}"
            );
        }
    }

    #[test]
    fn other_errors_are_not_image_rejections() {
        for err in [
            InferenceError::Api(
                r#"400 Bad Request: {"error":{"message":"Invalid value for 'temperature'"}}"#
                    .to_string(),
            ),
            InferenceError::Api("400 Bad Request: prompt is too long".to_string()),
            InferenceError::Parse("messages.content.type is invalid".to_string()),
            InferenceError::Timeout(30),
        ] {
            assert!(
                !err.rejects_image_input(),
                "should not be taken for an image rejection: {err}"
            );
        }
    }
}

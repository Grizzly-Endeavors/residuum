//! The real provider clients, pointed at a test server, so another crate's
//! test can check the request bodies a provider's own request builder sends.
//!
//! Each client has an API key, no retries, and the default HTTP settings.
//! Only the client itself is built: no failover chain, image guard or role
//! options around it.

use residuum_config::RetryConfig;

use crate::http::{HttpClientConfig, SharedHttpClient};
use crate::providers::anthropic::AnthropicClient;
use crate::providers::openai::OpenAiClient;
use crate::{InferenceError, InferenceProvider};

/// The most output tokens a request asks for when its options set none.
const MAX_TOKENS: u32 = 4096;

/// The Anthropic Messages API client for `model` at `base_url` (it posts to
/// `<base_url>/v1/messages`).
///
/// # Errors
/// Returns an error if the HTTP client cannot be built.
pub fn anthropic_client(
    base_url: &str,
    model: &str,
) -> Result<Box<dyn InferenceProvider>, InferenceError> {
    Ok(Box::new(AnthropicClient::new(
        SharedHttpClient::new(&HttpClientConfig::default())?,
        base_url,
        "test-api-key",
        model,
        MAX_TOKENS,
        RetryConfig::no_retry(),
    )))
}

/// The OpenAI-compatible chat completions client for `model` at `base_url`
/// (it posts to `<base_url>/chat/completions`), speaking the default
/// dialect.
///
/// # Errors
/// Returns an error if the HTTP client cannot be built.
pub fn openai_compatible_client(
    base_url: &str,
    model: &str,
) -> Result<Box<dyn InferenceProvider>, InferenceError> {
    Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
        SharedHttpClient::new(&HttpClientConfig::default())?,
        base_url,
        model,
        "test-api-key",
        MAX_TOKENS,
        RetryConfig::no_retry(),
    )))
}

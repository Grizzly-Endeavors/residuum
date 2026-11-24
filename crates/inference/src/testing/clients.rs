use residuum_config::RetryConfig;

use crate::http::{HttpClientConfig, SharedHttpClient};
use crate::providers::anthropic::AnthropicClient;
use crate::providers::openai::OpenAiClient;
use crate::{InferenceError, InferenceProvider};

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

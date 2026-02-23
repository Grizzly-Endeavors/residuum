//! Provider factory functions for constructing `ModelProvider` instances from config.

use crate::config::{ModelSpec, ProviderKind, ProviderSpec};
use crate::error::IronclawError;

use super::anthropic::AnthropicClient;
use super::gemini::GeminiClient;
use super::ollama::OllamaClient;
use super::openai::OpenAiClient;
use super::{ModelProvider, SharedHttpClient};

/// Build a model provider from a resolved `ProviderSpec`.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
pub(crate) fn build_provider_from_provider_spec(
    spec: &ProviderSpec,
    max_tokens: u32,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    build_provider_from_spec(
        &spec.model,
        &spec.provider_url,
        spec.api_key.as_deref(),
        max_tokens,
        http,
    )
}

/// Build a model provider from explicit parameters.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
pub(crate) fn build_provider_from_spec(
    spec: &ModelSpec,
    url: &str,
    api_key: Option<&str>,
    max_tokens: u32,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    match spec.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                IronclawError::Config(
                    "anthropic requires an API key (set ANTHROPIC_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(AnthropicClient::new(
                http,
                url,
                key,
                &spec.model,
                max_tokens,
            )))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| {
                IronclawError::Config(
                    "gemini requires an API key (set GEMINI_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(GeminiClient::new(
                http,
                url,
                key,
                &spec.model,
                max_tokens,
            )))
        }
        ProviderKind::Ollama => Ok(Box::new(OllamaClient::with_http_client(
            http,
            url,
            &spec.model,
        ))),
        ProviderKind::OpenAi => {
            if let Some(key) = api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    url,
                    &spec.model,
                    key,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    url,
                    &spec.model,
                )))
            }
        }
    }
}

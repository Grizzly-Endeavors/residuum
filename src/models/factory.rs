//! Provider factory functions for constructing `ModelProvider` instances from config.

use crate::config::{ProviderKind, ProviderSpec};
use crate::error::FatalError;

use super::anthropic::AnthropicClient;
use super::failover::FailoverProvider;
use super::gemini::GeminiClient;
use super::ollama::OllamaClient;
use super::openai::OpenAiClient;
use super::retry::RetryConfig;
use super::{ModelProvider, SharedHttpClient};

/// Build a model provider from a resolved `ProviderSpec`.
///
/// # Errors
/// Returns `FatalError::Config` if the API key is missing for providers
/// that require it.
pub(crate) fn build_provider_from_provider_spec(
    spec: &ProviderSpec,
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn ModelProvider>, FatalError> {
    match spec.model.kind {
        ProviderKind::Anthropic => {
            let key = spec.api_key.as_deref().ok_or_else(|| {
                FatalError::Config(
                    "anthropic requires an API key (set ANTHROPIC_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(AnthropicClient::new(
                http,
                &spec.provider_url,
                key,
                &spec.model.model,
                max_tokens,
                retry,
            )))
        }
        ProviderKind::Gemini => {
            let key = spec.api_key.as_deref().ok_or_else(|| {
                FatalError::Config(
                    "gemini requires an API key (set GEMINI_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(GeminiClient::new(
                http,
                &spec.provider_url,
                key,
                &spec.model.model,
                max_tokens,
                retry,
            )))
        }
        ProviderKind::Ollama => {
            if let Some(ref key) = spec.api_key {
                Ok(Box::new(OllamaClient::with_http_client_and_api_key(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    key,
                    spec.keep_alive.clone(),
                    retry,
                )))
            } else {
                Ok(Box::new(OllamaClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    spec.keep_alive.clone(),
                    retry,
                )))
            }
        }
        ProviderKind::OpenAi => {
            if let Some(ref key) = spec.api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    key,
                    retry,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    retry,
                )))
            }
        }
    }
}

/// Build a provider from a chain of specs.
///
/// Single spec → direct provider. Multiple specs → `FailoverProvider`.
///
/// # Errors
/// Returns `FatalError::Config` if any provider in the chain cannot be built.
pub(crate) fn build_provider_chain(
    specs: &[ProviderSpec],
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn ModelProvider>, FatalError> {
    if let [spec] = specs {
        return build_provider_from_provider_spec(spec, max_tokens, http, retry);
    }

    let mut providers = Vec::with_capacity(specs.len());
    for spec in specs {
        providers.push(build_provider_from_provider_spec(
            spec,
            max_tokens,
            http.clone(),
            retry.clone(),
        )?);
    }

    Ok(Box::new(FailoverProvider::new(providers)))
}

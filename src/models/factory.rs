//! Provider factory functions for constructing `ModelProvider` instances from config.

use crate::config::{ModelSpec, ProviderKind, ProviderSpec};
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
    build_provider_from_spec(
        &spec.model,
        &spec.provider_url,
        spec.api_key.as_deref(),
        spec.keep_alive.clone(),
        max_tokens,
        http,
        retry,
    )
}

/// Build a model provider from explicit parameters.
///
/// # Errors
/// Returns `FatalError::Config` if the API key is missing for providers
/// that require it.
fn build_provider_from_spec(
    spec: &ModelSpec,
    url: &str,
    api_key: Option<&str>,
    keep_alive: Option<String>,
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn ModelProvider>, FatalError> {
    match spec.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                FatalError::Config(
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
                retry,
            )))
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or_else(|| {
                FatalError::Config(
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
                retry,
            )))
        }
        ProviderKind::Ollama => {
            if let Some(key) = api_key {
                Ok(Box::new(OllamaClient::with_http_client_and_api_key(
                    http,
                    url,
                    &spec.model,
                    key,
                    keep_alive,
                    retry,
                )))
            } else {
                Ok(Box::new(OllamaClient::with_http_client(
                    http,
                    url,
                    &spec.model,
                    keep_alive,
                    retry,
                )))
            }
        }
        ProviderKind::OpenAi => {
            if let Some(key) = api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    url,
                    &spec.model,
                    key,
                    retry,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    url,
                    &spec.model,
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
    if specs.len() == 1
        && let Some(spec) = specs.first()
    {
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

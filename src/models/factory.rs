//! Provider factory functions for constructing `ModelProvider` instances from config.

use crate::config::{ProviderKind, ProviderSpec};
use crate::util::FatalError;

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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::config::{ModelSpec, ProviderKind, ProviderSpec};
    use crate::models::http::HttpClientConfig;

    fn make_spec(kind: ProviderKind, model: &str, api_key: Option<&str>) -> ProviderSpec {
        ProviderSpec {
            name: kind.to_string(),
            model: ModelSpec {
                kind,
                model: model.to_string(),
            },
            provider_url: kind.default_url().to_string(),
            api_key: api_key.map(String::from),
            keep_alive: None,
        }
    }

    #[test]
    fn anthropic_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(
            ProviderKind::Anthropic,
            "claude-sonnet-4-20250514",
            Some("sk-ant-test"),
        );
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn anthropic_requires_api_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Anthropic, "claude-sonnet-4-20250514", None);
        let result = build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry());
        assert!(result.is_err(), "anthropic without key should fail");
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err.contains("anthropic"),
            "error should mention anthropic: {err}"
        );
    }

    #[test]
    fn gemini_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Gemini, "gemini-2.0-flash", Some("AIza-test"));
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "gemini-2.0-flash");
    }

    #[test]
    fn gemini_requires_api_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Gemini, "gemini-2.0-flash", None);
        let result = build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry());
        assert!(result.is_err(), "gemini without key should fail");
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("gemini"), "error should mention gemini: {err}");
    }

    #[test]
    fn ollama_builds() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Ollama, "llama3.2", None);
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "llama3.2");
    }

    #[test]
    fn openai_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::OpenAi, "gpt-4", Some("sk-test"));
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "gpt-4");
    }

    #[test]
    fn openai_builds_without_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::OpenAi, "gpt-4", None);
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "gpt-4");
    }

    #[test]
    fn build_provider_chain_single_spec_direct() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Ollama, "llama3.2", None);
        let provider = build_provider_chain(&[spec], 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "llama3.2");
    }

    #[test]
    fn build_provider_chain_multiple_specs_creates_failover() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let specs = vec![
            make_spec(ProviderKind::Ollama, "primary-model", None),
            make_spec(ProviderKind::Ollama, "fallback-model", None),
        ];
        let provider = build_provider_chain(&specs, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(
            provider.model_name(),
            "primary-model",
            "failover returns primary model's name"
        );
    }
}

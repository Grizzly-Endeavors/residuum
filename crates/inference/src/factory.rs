use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use residuum_config::{ProviderKind, ProviderSpec};
use residuum_core::util::FatalError;

use super::failover::{FailoverProvider, NoticeSink};
use super::image_input::{ImageInputGuard, LearnedTextOnly};
use super::providers::anthropic::AnthropicClient;
use super::providers::fireworks;
use super::providers::gemini::GeminiClient;
use super::providers::ollama::OllamaClient;
use super::providers::openai::OpenAiClient;
use super::{InferenceProvider, SharedHttpClient};
use residuum_config::RetryConfig;

pub(crate) fn build_provider_from_provider_spec(
    spec: &ProviderSpec,
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn InferenceProvider>, FatalError> {
    let client = build_provider_client(spec, max_tokens, http, retry)?;
    Ok(Box::new(ImageInputGuard::new(
        client,
        spec,
        LearnedTextOnly::shared(),
    )))
}

fn build_provider_client(
    spec: &ProviderSpec,
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn InferenceProvider>, FatalError> {
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
        ProviderKind::Fireworks => {
            let key = spec.api_key.as_deref().ok_or_else(|| {
                FatalError::Config(
                    "fireworks requires an API key (set FIREWORKS_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(
                OpenAiClient::with_http_client_and_api_key(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    key,
                    max_tokens,
                    retry,
                )
                .with_dialect(fireworks::dialect(spec.session_affinity.clone())),
            ))
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
                    max_tokens,
                    retry,
                )))
            } else {
                Ok(Box::new(OllamaClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    spec.keep_alive.clone(),
                    max_tokens,
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
                    max_tokens,
                    retry,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    max_tokens,
                    retry,
                )))
            }
        }
    }
}

pub struct DroppedFallback {
    pub name: String,
    pub error: FatalError,
}

type BuiltProviderList = (Vec<Box<dyn InferenceProvider>>, Vec<DroppedFallback>);

fn build_provider_list(
    specs: &[ProviderSpec],
    max_tokens: u32,
    http: &SharedHttpClient,
    retry: &RetryConfig,
) -> Result<BuiltProviderList, FatalError> {
    let mut providers = Vec::with_capacity(specs.len());
    let mut dropped = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        match build_provider_from_provider_spec(spec, max_tokens, http.clone(), retry.clone()) {
            Ok(p) => providers.push(p),
            Err(error) if i == 0 => return Err(error),
            Err(error) => {
                tracing::warn!(
                    provider = %spec.name,
                    error = %error,
                    "dropping unbuildable fallback provider from chain"
                );
                dropped.push(DroppedFallback {
                    name: spec.name.clone(),
                    error,
                });
            }
        }
    }
    Ok((providers, dropped))
}

pub fn build_provider_chain(
    specs: &[ProviderSpec],
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<(Box<dyn InferenceProvider>, Vec<DroppedFallback>), FatalError> {
    if let [spec] = specs {
        return build_provider_from_provider_spec(spec, max_tokens, http, retry)
            .map(|p| (p, Vec::new()));
    }

    let (providers, dropped) = build_provider_list(specs, max_tokens, &http, &retry)?;
    Ok((Box::new(FailoverProvider::new(providers)), dropped))
}

pub fn build_provider_chain_with_notices(
    specs: &[ProviderSpec],
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
    notices: Arc<dyn NoticeSink>,
    role: impl Into<String>,
) -> Result<(Box<dyn InferenceProvider>, Vec<DroppedFallback>), FatalError> {
    if let [spec] = specs {
        return build_provider_from_provider_spec(spec, max_tokens, http, retry)
            .map(|p| (p, Vec::new()));
    }

    let (providers, dropped) = build_provider_list(specs, max_tokens, &http, &retry)?;
    Ok((
        Box::new(FailoverProvider::new(providers).with_notices(notices, role)),
        dropped,
    ))
}

pub fn build_provider_chain_with_shared_notices(
    specs: &[ProviderSpec],
    max_tokens: u32,
    http: SharedHttpClient,
    retry: RetryConfig,
    notices: Arc<dyn NoticeSink>,
    role: impl Into<String>,
    active_index: Arc<AtomicUsize>,
) -> Result<(Box<dyn InferenceProvider>, Vec<DroppedFallback>), FatalError> {
    if let [spec] = specs {
        return build_provider_from_provider_spec(spec, max_tokens, http, retry)
            .map(|p| (p, Vec::new()));
    }

    let (providers, dropped) = build_provider_list(specs, max_tokens, &http, &retry)?;
    Ok((
        Box::new(
            FailoverProvider::new(providers)
                .with_shared_active_index(active_index)
                .with_notices(notices, role),
        ),
        dropped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpClientConfig;
    use residuum_config::{ModelSpec, ProviderKind, ProviderSpec};

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
            session_affinity: None,
            image_input: residuum_config::ImageInput::Undeclared,
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
    fn fireworks_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(
            ProviderKind::Fireworks,
            "accounts/fireworks/models/deepseek-v3p1",
            Some("fw-test"),
        );
        let provider =
            build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(
            provider.model_name(),
            "accounts/fireworks/models/deepseek-v3p1"
        );
    }

    #[test]
    fn fireworks_requires_api_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(
            ProviderKind::Fireworks,
            "accounts/fireworks/models/deepseek-v3p1",
            None,
        );
        let result = build_provider_from_provider_spec(&spec, 1024, http, RetryConfig::no_retry());
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err.contains("FIREWORKS_API_KEY"),
            "error should say how to supply the key: {err}"
        );
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
        let (provider, dropped) =
            build_provider_chain(&[spec], 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "llama3.2");
        assert!(dropped.is_empty());
    }

    #[test]
    fn build_provider_chain_multiple_specs_creates_failover() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let specs = vec![
            make_spec(ProviderKind::Ollama, "primary-model", None),
            make_spec(ProviderKind::Ollama, "fallback-model", None),
        ];
        let (provider, dropped) =
            build_provider_chain(&specs, 1024, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(
            provider.model_name(),
            "primary-model",
            "failover returns primary model's name"
        );
        assert!(dropped.is_empty());
    }

    #[test]
    fn build_provider_chain_drops_unbuildable_fallback_keeps_primary() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let specs = vec![
            make_spec(ProviderKind::Ollama, "primary-model", None),
            make_spec(ProviderKind::Fireworks, "broken-fallback", None),
        ];
        let (provider, dropped) = build_provider_chain(&specs, 1024, http, RetryConfig::no_retry())
            .expect("chain should build despite the broken fallback");
        assert_eq!(provider.model_name(), "primary-model");
        assert_eq!(dropped.len(), 1, "the broken fallback should be dropped");
        assert_eq!(
            dropped.first().unwrap().name,
            ProviderKind::Fireworks.to_string()
        );
    }

    #[test]
    fn build_provider_chain_unbuildable_primary_is_fatal() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let specs = vec![
            make_spec(ProviderKind::Fireworks, "broken-primary", None),
            make_spec(ProviderKind::Ollama, "fallback-model", None),
        ];
        let result = build_provider_chain(&specs, 1024, http, RetryConfig::no_retry());
        assert!(
            result.is_err(),
            "an unbuildable primary must fail the whole chain, even with a working fallback"
        );
    }
}

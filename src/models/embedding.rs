//! Embedding provider trait and factory for vector embedding APIs.
//!
//! Separate from `ModelProvider` because embedding models use different
//! endpoints, model names, and don't need `max_tokens` or chat semantics.

use async_trait::async_trait;

use super::ModelError;
use super::SharedHttpClient;
use super::retry::RetryConfig;
use crate::config::{ProviderKind, ProviderSpec};
use crate::error::ResiduumError;

/// Trait for model providers that generate vector embeddings.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed one or more texts into vector representations.
    ///
    /// Returns one embedding per input text, in the same order as `texts`.
    ///
    /// # Errors
    /// Returns `ModelError` if the request fails, times out, or the response is malformed.
    async fn embed(&self, texts: &[&str]) -> Result<EmbeddingResponse, ModelError>;

    /// Get the model identifier.
    fn model_name(&self) -> &str;
}

/// Response from an embedding provider.
#[derive(Debug, Clone)]
pub struct EmbeddingResponse {
    /// One embedding vector per input text, in input order.
    pub embeddings: Vec<Vec<f32>>,
    /// Dimensionality of each embedding vector.
    pub dimensions: usize,
}

/// Build an embedding provider from a resolved `ProviderSpec`.
///
/// # Errors
/// Returns `ResiduumError::Config` if:
/// - The provider is Anthropic (no embeddings API)
/// - A required API key is missing
pub(crate) fn build_embedding_provider(
    spec: &ProviderSpec,
    http: SharedHttpClient,
    retry: RetryConfig,
) -> Result<Box<dyn EmbeddingProvider>, ResiduumError> {
    match spec.model.kind {
        ProviderKind::Anthropic => Err(ResiduumError::Config(
            "anthropic does not offer an embeddings API; \
             use openai, ollama, or gemini for models.embedding"
                .to_string(),
        )),
        ProviderKind::OpenAi => {
            let client = if let Some(ref key) = spec.api_key {
                super::openai::OpenAiEmbeddingClient::with_http_client_and_api_key(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    key,
                    retry,
                )
            } else {
                super::openai::OpenAiEmbeddingClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    retry,
                )
            };
            Ok(Box::new(client))
        }
        ProviderKind::Ollama => {
            let client = if let Some(ref key) = spec.api_key {
                super::ollama::OllamaEmbeddingClient::with_http_client_and_api_key(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    key,
                    spec.keep_alive.clone(),
                    retry,
                )
            } else {
                super::ollama::OllamaEmbeddingClient::with_http_client(
                    http,
                    &spec.provider_url,
                    &spec.model.model,
                    spec.keep_alive.clone(),
                    retry,
                )
            };
            Ok(Box::new(client))
        }
        ProviderKind::Gemini => {
            let key = spec.api_key.as_ref().ok_or_else(|| {
                ResiduumError::Config(
                    "gemini embeddings require an API key \
                     (set GEMINI_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;
            let client = super::gemini::GeminiEmbeddingClient::new(
                http,
                &spec.provider_url,
                key,
                &spec.model.model,
                retry,
            );
            Ok(Box::new(client))
        }
    }
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
    fn anthropic_rejected() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Anthropic, "some-model", Some("key"));
        let result = build_embedding_provider(&spec, http, RetryConfig::no_retry());
        assert!(result.is_err(), "anthropic should be rejected");
        let err = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err.contains("anthropic"),
            "error should mention anthropic: {err}"
        );
    }

    #[test]
    fn openai_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(
            ProviderKind::OpenAi,
            "text-embedding-3-small",
            Some("sk-test"),
        );
        let provider = build_embedding_provider(&spec, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "text-embedding-3-small");
    }

    #[test]
    fn openai_builds_without_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::OpenAi, "text-embedding-3-small", None);
        let provider = build_embedding_provider(&spec, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "text-embedding-3-small");
    }

    #[test]
    fn ollama_builds() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Ollama, "nomic-embed-text", None);
        let provider = build_embedding_provider(&spec, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "nomic-embed-text");
    }

    #[test]
    fn gemini_builds_with_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(
            ProviderKind::Gemini,
            "text-embedding-004",
            Some("AIza-test"),
        );
        let provider = build_embedding_provider(&spec, http, RetryConfig::no_retry()).unwrap();
        assert_eq!(provider.model_name(), "text-embedding-004");
    }

    #[test]
    fn gemini_requires_api_key() {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let spec = make_spec(ProviderKind::Gemini, "text-embedding-004", None);
        let result = build_embedding_provider(&spec, http, RetryConfig::no_retry());
        assert!(result.is_err(), "gemini without key should fail");
    }
}

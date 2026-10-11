//! Which agent's embedding model embeds the team wiki.
//!
//! The wiki index is shared by every agent, so it uses one embedder: the one
//! configured by the first agent (by name) that has an embedding model. The
//! host re-evaluates the choice whenever an agent starts, reloads its config,
//! or is deleted.

use std::sync::Arc;

use crate::config::{Config, ProviderSpec};
use crate::inference::retry::RetryConfig;
use crate::inference::{EmbeddingProvider, HttpClientConfig, SharedHttpClient};

/// An agent's embedding model, as the team wiki would use it.
#[derive(Clone)]
pub(crate) struct EmbeddingSource {
    agent: String,
    spec: ProviderSpec,
    timeout_secs: u64,
    retry: RetryConfig,
}

impl EmbeddingSource {
    /// The embedding model `cfg` configures for the agent `agent`, if any.
    pub(crate) fn from_config(agent: &str, cfg: &Config) -> Option<Self> {
        cfg.embedding.as_ref().map(|spec| Self {
            agent: agent.to_string(),
            spec: spec.clone(),
            timeout_secs: cfg.timeout_secs,
            retry: cfg.retry.clone(),
        })
    }

    /// Whether both name the same agent and the same model settings.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.agent == other.agent && self.spec == other.spec
    }

    /// The agent whose configuration provides the model.
    pub(crate) fn agent(&self) -> &str {
        &self.agent
    }

    /// Build the provider that embeds wiki pages and queries.
    ///
    /// # Errors
    /// Returns a plain-language reason when the HTTP client or the provider
    /// can't be built (a missing API key, or a provider without embeddings).
    pub(crate) fn build(&self) -> Result<Arc<dyn EmbeddingProvider>, String> {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(self.timeout_secs))
            .map_err(|e| format!("couldn't build an HTTP client: {e}"))?;
        crate::inference::build_embedding_provider(&self.spec, http, self.retry.clone())
            .map(Arc::from)
            .map_err(|e| e.to_string())
    }
}

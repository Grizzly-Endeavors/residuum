use std::sync::Arc;

use residuum_config::RetryConfig;
use residuum_config::{Config, ProviderSpec};
use residuum_inference::{EmbeddingProvider, HttpClientConfig, SharedHttpClient};

#[derive(Clone)]
pub(crate) struct EmbeddingSource {
    agent: String,
    spec: ProviderSpec,
    timeout_secs: u64,
    retry: RetryConfig,
}

impl EmbeddingSource {
    pub(crate) fn from_config(agent: &str, cfg: &Config) -> Option<Self> {
        cfg.embedding.as_ref().map(|spec| Self {
            agent: agent.to_string(),
            spec: spec.clone(),
            timeout_secs: cfg.timeout_secs,
            retry: cfg.retry.clone(),
        })
    }

    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.agent == other.agent && self.spec == other.spec
    }

    pub(crate) fn agent(&self) -> &str {
        &self.agent
    }

    pub(crate) fn build(&self) -> Result<Arc<dyn EmbeddingProvider>, String> {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(self.timeout_secs))
            .map_err(|e| format!("couldn't build an HTTP client: {e}"))?;
        residuum_inference::build_embedding_provider(&self.spec, http, self.retry.clone())
            .map(Arc::from)
            .map_err(|e| e.to_string())
    }
}

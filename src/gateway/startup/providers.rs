//! Model providers and memory pipeline initialization.

use crate::config::Config;
use crate::error::ResiduumError;
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::models::{EmbeddingProvider, SharedHttpClient, build_embedding_provider, build_provider_chain};

use super::memory::build_memory_components;

/// Model providers and memory pipeline observers built from config.
pub struct ProviderComponents {
    pub provider: Box<dyn crate::models::ModelProvider>,
    pub observer: Observer,
    pub reflector: Reflector,
    pub embedding_provider: Option<std::sync::Arc<dyn EmbeddingProvider>>,
}

/// Build model providers, observer, reflector, and embedding provider.
///
/// # Errors
/// Returns `ResiduumError` if the main model provider fails to build.
pub fn init_providers(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<ProviderComponents, ResiduumError> {
    let provider =
        build_provider_chain(&cfg.main, cfg.max_tokens, http.clone(), cfg.retry.clone())?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    let (observer, reflector) = match build_memory_components(cfg, tz, http.clone()) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!(
                "warning: memory subsystem failed to initialize, running without observer/reflector: {err}"
            );
            tracing::warn!(error = %err, "memory subsystem degraded: observer and reflector disabled");
            (Observer::disabled(tz), Reflector::disabled(tz))
        }
    };

    let embedding_provider: Option<std::sync::Arc<dyn EmbeddingProvider>> = match cfg
        .embedding
        .as_ref()
        .map(|spec| build_embedding_provider(spec, http, cfg.retry.clone()))
        .transpose()
    {
        Ok(ep) => {
            if let Some(ref e) = ep {
                tracing::info!(model = e.model_name(), "embedding provider ready");
            }
            ep.map(std::sync::Arc::from)
        }
        Err(err) => {
            eprintln!("warning: embedding provider failed to initialize: {err}");
            tracing::warn!(error = %err, "embedding provider degraded");
            None
        }
    };

    Ok(ProviderComponents {
        provider,
        observer,
        reflector,
        embedding_provider,
    })
}

//! Model providers and memory pipeline initialization.

use crate::bus::Publisher;
use crate::config::Config;
use crate::inference::{
    CompletionOptions, EmbeddingProvider, SharedHttpClient, WebSearchNativeConfig,
    build_embedding_provider, build_provider_chain_with_notices,
};
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::util::FatalError;

use super::memory::build_memory_components;

/// Model providers and memory pipeline observers built from config.
pub struct ProviderComponents {
    pub provider: Box<dyn crate::inference::InferenceProvider>,
    pub options: CompletionOptions,
    pub observer: Observer,
    pub reflector: Reflector,
    pub embedding_provider: Option<std::sync::Arc<dyn EmbeddingProvider>>,
}

/// Build model providers, observer, reflector, and embedding provider.
///
/// Only an unusable primary main-model provider is fatal here. Every other
/// degradation — a dropped fallback, a disabled observer or reflector, the
/// embedding provider below — is collected into `degradations` for the
/// caller to report as one grouped notice; the main model's own chain
/// additionally gets a live notice on a fallback/recovery transition at
/// runtime (see `crate::inference::FailoverProvider`).
///
/// # Errors
/// Returns `FatalError` if the primary main model provider fails to build.
pub fn init_providers(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
    publisher: Publisher,
    degradations: &mut Vec<String>,
) -> Result<ProviderComponents, FatalError> {
    let (provider, dropped_main) = build_provider_chain_with_notices(
        &cfg.main,
        cfg.max_tokens,
        http.clone(),
        cfg.retry.clone(),
        publisher,
        "main model",
    )?;
    tracing::info!(model = provider.model_name(), "model provider ready");
    for fallback in &dropped_main {
        tracing::warn!(
            provider = %fallback.name,
            error = %fallback.error,
            "dropped an unbuildable fallback provider from the main chain"
        );
        degradations.push(format!(
            "the fallback model \"{}\" in your main provider chain couldn't be started and was skipped: {}",
            fallback.name, fallback.error
        ));
    }

    let (observer, reflector, memory_notices) = build_memory_components(cfg, tz, http.clone());
    degradations.extend(memory_notices);

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
            tracing::warn!(error = %err, "embedding provider degraded");
            degradations.push(format!(
                "the embedding provider is unavailable, so semantic search is disabled: {err}"
            ));
            None
        }
    };

    let web_search = cfg
        .web_search
        .provider_native
        .as_ref()
        .map(|pn| WebSearchNativeConfig {
            max_uses: pn.max_uses,
            allowed_domains: pn.allowed_domains.clone(),
            blocked_domains: pn.blocked_domains.clone(),
            search_context_size: pn.search_context_size.clone(),
            exclude_domains: pn.exclude_domains.clone(),
        });
    let mut options = cfg.completion_options_for_role("main");
    options.web_search = web_search;

    Ok(ProviderComponents {
        provider,
        options,
        observer,
        reflector,
        embedding_provider,
    })
}

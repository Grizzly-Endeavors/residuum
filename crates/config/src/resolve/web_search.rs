//! Hosted and standalone web-search settings.

use super::super::deserialize::WebSearchConfigFile;
use super::super::provider::ProviderKind;
use super::super::secrets::SecretStore;
use super::super::types::{ProviderNativeSearchConfig, StandaloneBackendConfig, WebSearchConfig};

/// Resolve web search configuration from TOML section.
///
/// Provider-native search is enabled automatically when the main provider supports it
/// (Anthropic, `OpenAI`, Gemini). Standalone backends are resolved from the `backend` field.
pub(super) fn resolve_web_search_config(
    section: Option<&WebSearchConfigFile>,
    main_chain: &[super::super::provider::ProviderSpec],
    secrets: &SecretStore,
) -> WebSearchConfig {
    let mut cfg = WebSearchConfig::default();

    // Determine the main provider kind for native search detection
    let main_kind = main_chain.first().map(|p| p.model.kind);

    let has_native = main_chain.first().is_some_and(offers_native_web_search);

    if has_native {
        let mut native = ProviderNativeSearchConfig::default();

        if let Some(s) = section {
            if main_kind == Some(ProviderKind::Anthropic)
                && let Some(ref a) = s.anthropic
            {
                native.max_uses = a.max_uses;
                native.allowed_domains.clone_from(&a.allowed_domains);
                native.blocked_domains.clone_from(&a.blocked_domains);
            }
            if main_kind == Some(ProviderKind::OpenAi)
                && let Some(ref o) = s.openai
            {
                native
                    .search_context_size
                    .clone_from(&o.search_context_size);
            }
            if main_kind == Some(ProviderKind::Gemini)
                && let Some(ref g) = s.gemini
            {
                native.exclude_domains.clone_from(&g.exclude_domains);
            }
        }

        cfg.provider_native = Some(native);
    }

    // Standalone backend
    if let Some(s) = section
        && let Some(ref backend_name) = s.backend
    {
        let resolved = match backend_name.as_str() {
            "brave" => resolve_standalone_backend(
                "brave",
                s.brave.as_ref().and_then(|b| b.api_key.as_deref()),
                "BRAVE_API_KEY",
                None,
                secrets,
            ),
            "tavily" => resolve_standalone_backend(
                "tavily",
                s.tavily.as_ref().and_then(|t| t.api_key.as_deref()),
                "TAVILY_API_KEY",
                None,
                secrets,
            ),
            "ollama" => resolve_standalone_backend(
                "ollama",
                s.ollama.as_ref().and_then(|o| o.api_key.as_deref()),
                "OLLAMA_API_KEY",
                s.ollama.as_ref().and_then(|o| o.base_url.clone()),
                secrets,
            ),
            other => {
                tracing::warn!(
                    section = "web_search",
                    backend = other,
                    "unknown backend; expected brave, tavily, or ollama"
                );
                None
            }
        };

        if resolved.is_none() && !backend_name.is_empty() {
            tracing::warn!(
                section = "web_search",
                backend = backend_name.as_str(),
                "backend configured but no API key found; set api_key in config or the corresponding env var"
            );
        }

        cfg.standalone_backend = resolved;
    }

    cfg
}

/// Resolve a standalone web search backend by looking up the API key from config,
/// secrets, or an environment variable fallback.
fn resolve_standalone_backend(
    name: &str,
    api_key: Option<&str>,
    env_var: &str,
    base_url: Option<String>,
    secrets: &SecretStore,
) -> Option<StandaloneBackendConfig> {
    let key = api_key
        .and_then(|k| super::resolve_secret_value(k, secrets))
        .or_else(|| std::env::var(env_var).ok())?;
    Some(StandaloneBackendConfig {
        name: name.to_string(),
        api_key: key,
        base_url,
    })
}

/// Whether the main provider offers a built-in web search tool.
///
/// The `openai` kind also covers self-hosted compatible servers (vLLM, LM
/// Studio), which reject `OpenAI`'s hosted search tool, so it only qualifies
/// when pointed at the `OpenAI` API itself.
fn offers_native_web_search(spec: &super::super::provider::ProviderSpec) -> bool {
    match spec.model.kind {
        ProviderKind::Anthropic | ProviderKind::Gemini => true,
        ProviderKind::OpenAi => {
            let hosted = spec.provider_url.trim_end_matches('/')
                == super::super::constants::DEFAULT_OPENAI_URL;
            if !hosted {
                tracing::debug!(
                    provider_url = %spec.provider_url,
                    "native web search off: openai-compatible endpoint is not the OpenAI API"
                );
            }
            hosted
        }
        ProviderKind::Fireworks | ProviderKind::Ollama => false,
    }
}

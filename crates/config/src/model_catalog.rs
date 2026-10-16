//! Resolving one `provider/model` id into a [`ProviderSpec`].
//!
//! Every role in `[models]` and every `[background.models]` tier is resolved
//! once, at config load. An explicit model id named by a pulse, a scheduled
//! action, or a `subagent_spawn` call is resolved when its session starts,
//! against the [`ModelCatalog`] built from the agent's current
//! `providers.toml`, so a provider added later works without a restart.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use super::deserialize::{InputModalityFile, ProviderEntryFile};
use super::provider::{ImageInput, ModelSpec, ProviderKind, ProviderSpec};
use super::secrets::SecretStore;
use super::types::ModelSelector;

/// One `[providers.<name>]` entry with its API key already resolved.
#[derive(Clone, PartialEq)]
pub(super) struct NamedProvider {
    /// The entry's `type`, as written. Parsed when a model is resolved, so a
    /// bad type is reported against the model id that names the entry.
    kind: String,
    url: Option<String>,
    api_key: Option<String>,
    keep_alive: Option<String>,
    /// Each listed model's declared image input, keyed by model name.
    image_inputs: HashMap<String, ImageInput>,
}

impl NamedProvider {
    /// Resolve an entry's API key (`${ENV}`, `secret:name`, or a literal) and
    /// declared model inputs.
    pub(super) fn from_entry(entry: &ProviderEntryFile, secrets: &SecretStore) -> Self {
        let image_inputs = entry
            .models
            .iter()
            .flatten()
            .map(|(name, model)| {
                let input = match model.input.as_ref() {
                    None => ImageInput::Undeclared,
                    Some(kinds) if kinds.contains(&InputModalityFile::Image) => {
                        ImageInput::Accepted
                    }
                    Some(_) => ImageInput::TextOnly,
                };
                (name.clone(), input)
            })
            .collect();
        Self {
            kind: entry.kind.clone(),
            url: entry.url.clone(),
            api_key: entry
                .api_key
                .as_deref()
                .and_then(|raw| super::resolve::resolve_secret_value(raw, secrets)),
            keep_alive: entry.keep_alive.clone(),
            image_inputs,
        }
    }

    /// The spec for `model_name` served by this entry, named `name`.
    ///
    /// # Errors
    /// Returns a message if the entry's `type` is not a known provider type.
    pub(super) fn spec(&self, name: &str, model_name: &str) -> Result<ProviderSpec, String> {
        let kind = ProviderKind::from_str(&self.kind)
            .map_err(|e| format!("provider '{name}' has invalid type '{}': {e}", self.kind))?;
        Ok(ProviderSpec {
            name: name.to_owned(),
            model: ModelSpec {
                kind,
                model: model_name.to_owned(),
            },
            provider_url: self
                .url
                .clone()
                .unwrap_or_else(|| kind.default_url().to_string()),
            api_key: self.api_key.clone().or_else(|| provider_api_key_env(kind)),
            keep_alive: self.keep_alive.clone(),
            session_affinity: None,
            image_input: self
                .image_inputs
                .get(model_name)
                .copied()
                .unwrap_or_default(),
        })
    }
}

/// Split a `provider_or_name/model` id on its first `/`.
///
/// # Errors
/// Returns a message if there is no `/` or the model part is empty.
pub(super) fn split_model_id(model_id: &str) -> Result<(&str, &str), String> {
    let (provider_part, model_name) = model_id
        .split_once('/')
        .ok_or_else(|| format!("expected 'provider/model' format, got '{model_id}'"))?;
    if model_name.is_empty() {
        return Err("model name cannot be empty".to_string());
    }
    Ok((provider_part, model_name))
}

/// The spec for a model id whose provider part names a provider type
/// (`anthropic`, `openai`, …) rather than a `[providers]` entry. The API key
/// comes from that provider's environment variable.
///
/// # Errors
/// Returns a message if `provider_part` is not a known provider type.
pub(super) fn implicit_provider_spec(
    provider_part: &str,
    model_name: &str,
) -> Result<ProviderSpec, String> {
    let kind = ProviderKind::from_str(provider_part).map_err(|_parse_err| {
        format!(
            "'{provider_part}' is not a known provider name or type \
             (expected one of: anthropic, fireworks, gemini, ollama, openai, \
             or a key from [providers])"
        )
    })?;
    Ok(ProviderSpec {
        name: provider_part.to_owned(),
        model: ModelSpec {
            kind,
            model: model_name.to_owned(),
        },
        provider_url: kind.default_url().to_string(),
        api_key: provider_api_key_env(kind),
        keep_alive: None,
        session_affinity: None,
        image_input: ImageInput::Undeclared,
    })
}

/// Get the provider-specific API key from environment variables.
pub(super) fn provider_api_key_env(kind: ProviderKind) -> Option<String> {
    match kind {
        ProviderKind::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
        ProviderKind::Fireworks => std::env::var("FIREWORKS_API_KEY").ok(),
        ProviderKind::Gemini => std::env::var("GEMINI_API_KEY").ok(),
        ProviderKind::OpenAi => std::env::var("OPENAI_API_KEY").ok(),
        ProviderKind::Ollama => std::env::var("OLLAMA_API_KEY").ok(),
    }
}

/// The providers an explicit `provider/model` id can name: this agent's
/// `[providers]` entries, plus every provider type by name.
#[derive(Clone, Default, PartialEq)]
pub struct ModelCatalog {
    providers: HashMap<String, NamedProvider>,
    /// Session-affinity key stamped on every spec this catalog resolves (see
    /// [`ProviderSpec::session_affinity`]).
    session_affinity: Option<String>,
}

impl ModelCatalog {
    /// Build the catalog from the agent's `[providers]` table. Every spec it
    /// resolves carries the `bg_explicit` session-affinity key for
    /// `workspace_dir`.
    pub(super) fn new(
        providers_map: Option<&HashMap<String, ProviderEntryFile>>,
        secrets: &SecretStore,
        workspace_dir: &Path,
    ) -> Self {
        Self {
            providers: providers_map
                .into_iter()
                .flatten()
                .map(|(name, entry)| (name.clone(), NamedProvider::from_entry(entry, secrets)))
                .collect(),
            session_affinity: Some(super::resolve::session_affinity_key(
                workspace_dir,
                "bg_explicit",
            )),
        }
    }

    /// Wrap this catalog for sharing (see [`SharedModelCatalog`]).
    #[must_use]
    pub fn into_shared(self) -> SharedModelCatalog {
        std::sync::Arc::new(tokio::sync::RwLock::new(self))
    }

    /// Check that `selector` can run: a tier always can, an explicit model
    /// only when [`Self::resolve`] resolves it.
    ///
    /// # Errors
    /// Returns a plain-language message naming the model and why it doesn't
    /// resolve.
    pub fn check(&self, selector: &ModelSelector) -> Result<(), String> {
        match selector {
            ModelSelector::Tier(_) => Ok(()),
            ModelSelector::Explicit(id) => self
                .resolve(id)
                .map(drop)
                .map_err(|e| format!("model '{id}' can't be used: {e}")),
        }
    }

    /// Resolve an explicit `provider_or_name/model` id the same way
    /// `providers.toml`'s `[models]` resolves one: a provider part that names
    /// a `[providers]` entry uses that entry, anything else must be a
    /// provider type.
    ///
    /// # Errors
    /// Returns a plain-language message if the id is malformed, names neither
    /// a `[providers]` entry nor a provider type, or names an entry whose
    /// `type` is invalid.
    pub fn resolve(&self, model_id: &str) -> Result<ProviderSpec, String> {
        let (provider_part, model_name) = split_model_id(model_id)?;
        let mut spec = match self.providers.get(provider_part) {
            Some(entry) => entry.spec(provider_part, model_name)?,
            None => implicit_provider_spec(provider_part, model_name)?,
        };
        spec.session_affinity.clone_from(&self.session_affinity);
        Ok(spec)
    }
}

/// The live [`ModelCatalog`], replaced in place on every config reload so
/// the tools and sessions holding it resolve against the current
/// `providers.toml`.
pub type SharedModelCatalog = std::sync::Arc<tokio::sync::RwLock<ModelCatalog>>;

impl fmt::Debug for ModelCatalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        names.sort_unstable();
        f.debug_struct("ModelCatalog")
            .field("providers", &names)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_secrets() -> SecretStore {
        let dir = tempfile::tempdir().unwrap();
        SecretStore::load(dir.path()).unwrap()
    }

    fn entry(kind: &str) -> ProviderEntryFile {
        ProviderEntryFile {
            kind: kind.to_string(),
            api_key: Some("sk-literal".to_string()),
            url: Some("http://localhost:9000".to_string()),
            keep_alive: None,
            models: None,
        }
    }

    fn catalog(providers: &[(&str, &str)]) -> ModelCatalog {
        let map: HashMap<String, ProviderEntryFile> = providers
            .iter()
            .map(|(name, kind)| ((*name).to_string(), entry(kind)))
            .collect();
        ModelCatalog::new(
            Some(&map),
            &empty_secrets(),
            Path::new("/tmp/residuum-test-agent"),
        )
    }

    #[test]
    fn resolves_a_named_provider_entry() {
        let spec = catalog(&[("local", "ollama")])
            .resolve("local/qwen3:8b")
            .unwrap();
        assert_eq!(spec.name, "local");
        assert_eq!(spec.model.kind, ProviderKind::Ollama);
        assert_eq!(spec.model.model, "qwen3:8b");
        assert_eq!(spec.provider_url, "http://localhost:9000");
        assert_eq!(spec.api_key.as_deref(), Some("sk-literal"));
        assert!(
            spec.session_affinity
                .as_deref()
                .is_some_and(|key| key.starts_with("residuum-bg_explicit-")),
            "{:?}",
            spec.session_affinity
        );
    }

    #[test]
    fn resolves_a_provider_type_with_no_entry() {
        let spec = catalog(&[]).resolve("anthropic/claude-haiku-4-5").unwrap();
        assert_eq!(spec.name, "anthropic");
        assert_eq!(spec.model.kind, ProviderKind::Anthropic);
        assert_eq!(spec.model.model, "claude-haiku-4-5");
    }

    #[test]
    fn model_part_keeps_later_slashes() {
        let spec = catalog(&[("router", "openai")])
            .resolve("router/meta-llama/llama-3")
            .unwrap();
        assert_eq!(spec.model.model, "meta-llama/llama-3");
    }

    #[test]
    fn unknown_provider_is_an_error_naming_it() {
        let err = catalog(&[("local", "ollama")])
            .resolve("lcoal/qwen3:8b")
            .unwrap_err();
        assert!(err.contains("'lcoal' is not a known provider"), "{err}");
    }

    #[test]
    fn entry_with_a_bad_type_is_an_error() {
        let err = catalog(&[("odd", "not-a-type")])
            .resolve("odd/model")
            .unwrap_err();
        assert!(err.contains("provider 'odd' has invalid type"), "{err}");
    }

    #[test]
    fn malformed_ids_are_errors() {
        let cat = catalog(&[]);
        assert!(cat.resolve("no-slash").is_err());
        assert!(cat.resolve("anthropic/").is_err());
    }

    #[test]
    fn debug_never_shows_api_keys() {
        let shown = format!("{:?}", catalog(&[("local", "ollama")]));
        assert!(shown.contains("local"), "{shown}");
        assert!(!shown.contains("sk-literal"), "{shown}");
    }
}

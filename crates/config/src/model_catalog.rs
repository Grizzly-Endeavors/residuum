use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use super::deserialize::{InputModalityFile, ProviderEntryFile};
use super::provider::{ImageInput, ModelSpec, ProviderKind, ProviderSpec};
use super::secrets::SecretStore;
use super::types::ModelSelector;

#[derive(Clone, PartialEq)]
pub(super) struct NamedProvider {
    kind: String,
    url: Option<String>,
    api_key: Option<String>,
    keep_alive: Option<String>,
    image_inputs: HashMap<String, ImageInput>,
}

impl NamedProvider {
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

pub(super) fn split_model_id(model_id: &str) -> Result<(&str, &str), String> {
    let (provider_part, model_name) = model_id
        .split_once('/')
        .ok_or_else(|| format!("expected 'provider/model' format, got '{model_id}'"))?;
    if model_name.is_empty() {
        return Err("model name cannot be empty".to_string());
    }
    Ok((provider_part, model_name))
}

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

pub(super) fn provider_api_key_env(kind: ProviderKind) -> Option<String> {
    match kind {
        ProviderKind::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
        ProviderKind::Fireworks => std::env::var("FIREWORKS_API_KEY").ok(),
        ProviderKind::Gemini => std::env::var("GEMINI_API_KEY").ok(),
        ProviderKind::OpenAi => std::env::var("OPENAI_API_KEY").ok(),
        ProviderKind::Ollama => std::env::var("OLLAMA_API_KEY").ok(),
    }
}

const fn required_api_key(kind: ProviderKind) -> Option<(&'static str, &'static str)> {
    match kind {
        ProviderKind::Anthropic => Some(("anthropic", "ANTHROPIC_API_KEY")),
        ProviderKind::Fireworks => Some(("fireworks", "FIREWORKS_API_KEY")),
        ProviderKind::Gemini => Some(("gemini", "GEMINI_API_KEY")),
        ProviderKind::OpenAi | ProviderKind::Ollama => None,
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct ModelCatalog {
    providers: HashMap<String, NamedProvider>,
    session_affinity: Option<String>,
}

impl ModelCatalog {
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

    #[must_use]
    pub fn into_shared(self) -> SharedModelCatalog {
        std::sync::Arc::new(tokio::sync::RwLock::new(self))
    }

    #[must_use]
    pub fn credential_changed_providers(&self, other: &Self) -> Vec<String> {
        let mut names: Vec<String> = self
            .providers
            .iter()
            .filter_map(|(name, old_entry)| {
                let new_entry = other.providers.get(name)?;
                (old_entry.kind == new_entry.kind
                    && old_entry.url == new_entry.url
                    && old_entry.api_key != new_entry.api_key)
                    .then(|| name.clone())
            })
            .collect();
        names.sort();
        names
    }

    pub fn check(&self, selector: &ModelSelector) -> Result<(), String> {
        match selector {
            ModelSelector::Tier(_) => Ok(()),
            ModelSelector::Explicit(id) => {
                let spec = self
                    .resolve(id)
                    .map_err(|e| format!("model '{id}' can't be used: {e}"))?;
                if let Some((kind_name, env_var)) = required_api_key(spec.model.kind)
                    && spec.api_key.as_deref().is_none_or(str::is_empty)
                {
                    let configured_hint = self.configured_providers_hint();
                    return Err(format!(
                        "model '{id}' can't be used: {kind_name} requires an API key \
                         (set {env_var} or api_key in providers.toml{configured_hint})"
                    ));
                }
                Ok(())
            }
        }
    }

    fn configured_providers_hint(&self) -> String {
        let mut names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        names.sort_unstable();
        if names.is_empty() {
            "; or pass a tier: \"small\", \"medium\", \"large\"".to_string()
        } else {
            format!(
                "; configured providers in providers.toml: {}; or pass a tier: \"small\", \"medium\", \"large\"",
                names.join(", ")
            )
        }
    }

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

    #[test]
    fn credential_changed_providers_detects_api_key_rotations() {
        let mut old_entry = entry("openai");
        old_entry.api_key = Some("sk-old".to_string());
        let mut new_entry = entry("openai");
        new_entry.api_key = Some("sk-new".to_string());

        let old_map = HashMap::from([("openrouter".to_string(), old_entry)]);
        let new_map = HashMap::from([("openrouter".to_string(), new_entry)]);
        let secrets = empty_secrets();
        let ws = Path::new("/tmp/residuum-test-agent");
        let old_cat = ModelCatalog::new(Some(&old_map), &secrets, ws);
        let new_cat = ModelCatalog::new(Some(&new_map), &secrets, ws);

        assert_eq!(
            old_cat.credential_changed_providers(&new_cat),
            vec!["openrouter".to_string()]
        );
        assert_eq!(
            old_cat.credential_changed_providers(&old_cat),
            Vec::<String>::new()
        );
    }

    #[test]
    fn check_rejects_implicit_provider_missing_required_api_key_and_names_configured_providers() {
        let cat = catalog(&[("nabu", "openai")]);
        assert!(
            cat.check(&ModelSelector::Explicit("nabu/glm-5.3".to_string()))
                .is_ok()
        );
        let err = cat
            .check(&ModelSelector::Explicit(
                "anthropic/claude-haiku-4-5".to_string(),
            ))
            .unwrap_err();
        assert!(err.contains("anthropic requires an API key"), "{err}");
        assert!(
            err.contains("configured providers in providers.toml: nabu"),
            "{err}"
        );
    }
}

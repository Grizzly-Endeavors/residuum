//! Model and provider chain resolution.
//!
//! Resolves `"provider/model"` strings and `[models]` config into fully-built
//! `ProviderSpec` values with failover chains.

use std::collections::HashMap;
use std::str::FromStr;

use crate::config::types::RoleOverrides;
use crate::error::ResiduumError;

use super::super::deserialize::{ModelAssignment, ModelsConfigFile, ProviderEntryFile};
use super::super::provider::{ModelSpec, ProviderKind, ProviderSpec};
use super::super::secrets::SecretStore;

/// Resolved model provider specs for all roles.
pub(super) struct ResolvedModels {
    pub(super) main: Vec<ProviderSpec>,
    pub(super) observer: Vec<ProviderSpec>,
    pub(super) reflector: Vec<ProviderSpec>,
    pub(super) pulse: Vec<ProviderSpec>,
    pub(super) embedding: Option<ProviderSpec>,
    pub(super) role_overrides: HashMap<String, RoleOverrides>,
}

/// Resolve all model specs (main, observer, reflector, pulse, embedding) from the
/// `[models]` config section and environment overrides.
///
/// # Errors
/// Returns `ResiduumError::Config` if any model string is invalid or an unsupported
/// provider is used for embeddings.
pub(super) fn resolve_all_model_specs(
    models: Option<&ModelsConfigFile>,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<ResolvedModels, ResiduumError> {
    let mut role_overrides = HashMap::new();

    // Resolve main: RESIDUUM_MODEL env > models.main > default
    let main = if let Ok(env_model) = std::env::var("RESIDUUM_MODEL") {
        vec![resolve_model_string(&env_model, providers_map, secrets)?]
    } else if let Some(main_spec) = models.and_then(|m| m.main.clone()) {
        extract_role_overrides("main", &main_spec, &mut role_overrides)?;
        resolve_assignment_chain(main_spec, providers_map, secrets)?
    } else {
        vec![resolve_model_string(
            "anthropic/claude-sonnet-4-6",
            providers_map,
            secrets,
        )?]
    };

    // RESIDUUM_PROVIDER_URL overrides first provider in main chain only
    let main = if let Ok(url) = std::env::var("RESIDUUM_PROVIDER_URL") {
        let mut chain = main;
        if let Some(first) = chain.first_mut() {
            first.provider_url = url;
        }
        chain
    } else {
        main
    };

    // Resolve each role: models.<role> > models.default > main
    let default_assignment = models.and_then(|m| m.default.clone());

    let observer = resolve_role_chain_from_assignment(
        models.and_then(|m| m.observer.clone()),
        default_assignment.as_ref(),
        &main,
        providers_map,
        secrets,
        "observer",
        &mut role_overrides,
    )?;
    let reflector = resolve_role_chain_from_assignment(
        models.and_then(|m| m.reflector.clone()),
        default_assignment.as_ref(),
        &main,
        providers_map,
        secrets,
        "reflector",
        &mut role_overrides,
    )?;
    let pulse = resolve_role_chain_from_assignment(
        models.and_then(|m| m.pulse.clone()),
        default_assignment.as_ref(),
        &main,
        providers_map,
        secrets,
        "pulse",
        &mut role_overrides,
    )?;

    // Resolve embedding: models.embedding only, no fallback to default or main
    let embedding = models
        .and_then(|m| m.embedding.as_deref())
        .map(|s| resolve_model_string(s, providers_map, secrets))
        .transpose()?;
    if let Some(ref spec) = embedding
        && spec.model.kind == ProviderKind::Anthropic
    {
        return Err(ResiduumError::Config(
            "anthropic does not offer an embeddings API; \
             use openai, ollama, or gemini for models.embedding"
                .to_string(),
        ));
    }

    Ok(ResolvedModels {
        main,
        observer,
        reflector,
        pulse,
        embedding,
        role_overrides,
    })
}

/// Resolve a `"provider_or_name/model"` string into a `ProviderSpec`.
///
/// Splits on the first `/`:
/// - If `provider_part` matches a key in `providers_map`, that entry's `type`,
///   `url`, and `api_key` are used.
/// - Otherwise `provider_part` is treated as an implicit `ProviderKind` name
///   (e.g. `"anthropic"`). API key falls back to provider-specific env var,
///   then `RESIDUUM_API_KEY`.
///
/// # Errors
/// Returns `ResiduumError::Config` if the model string format is invalid,
/// the provider is unknown, or an explicit provider entry references an
/// unknown type.
fn resolve_model_string(
    model_str: &str,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<ProviderSpec, ResiduumError> {
    let (provider_part, model_name) = model_str.split_once('/').ok_or_else(|| {
        ResiduumError::Config(format!(
            "expected 'provider/model' format, got '{model_str}'"
        ))
    })?;

    if model_name.is_empty() {
        return Err(ResiduumError::Config(
            "model name cannot be empty".to_string(),
        ));
    }

    // Check if provider_part matches a named [providers] entry
    if let Some(entry) = providers_map.and_then(|p| p.get(provider_part)) {
        let kind = ProviderKind::from_str(&entry.kind).map_err(|e| {
            ResiduumError::Config(format!(
                "provider '{provider_part}' has invalid type '{}': {e}",
                entry.kind
            ))
        })?;

        let provider_url = entry
            .url
            .clone()
            .unwrap_or_else(|| kind.default_url().to_string());

        let api_key = entry
            .api_key
            .as_deref()
            .and_then(|raw| super::resolve_secret_value(raw, secrets))
            .or_else(|| provider_api_key_env(kind))
            .or_else(|| std::env::var("RESIDUUM_API_KEY").ok());

        return Ok(ProviderSpec {
            name: provider_part.to_owned(),
            model: ModelSpec {
                kind,
                model: model_name.to_owned(),
            },
            provider_url,
            api_key,
            keep_alive: entry.keep_alive.clone(),
        });
    }

    // Treat provider_part as an implicit ProviderKind
    let kind = ProviderKind::from_str(provider_part).map_err(|_parse_err| {
        ResiduumError::Config(format!(
            "'{provider_part}' is not a known provider name or type \
             (expected one of: anthropic, gemini, ollama, openai, \
             or a key from [providers])"
        ))
    })?;

    let provider_url = kind.default_url().to_string();

    let api_key = provider_api_key_env(kind).or_else(|| std::env::var("RESIDUUM_API_KEY").ok());

    Ok(ProviderSpec {
        name: provider_part.to_owned(),
        model: ModelSpec {
            kind,
            model: model_name.to_owned(),
        },
        provider_url,
        api_key,
        keep_alive: None,
    })
}

/// Resolve a `ModelAssignment` into a `Vec<ProviderSpec>` (failover chain).
///
/// # Errors
/// Returns `ResiduumError::Config` if any model string in the assignment cannot be resolved.
pub(super) fn resolve_assignment_chain(
    assignment: ModelAssignment,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<Vec<ProviderSpec>, ResiduumError> {
    assignment
        .into_model_strings()
        .iter()
        .map(|s| resolve_model_string(s, providers_map, secrets))
        .collect()
}

/// Extract per-role overrides from a `ModelAssignment` and insert into the map.
///
/// Public within the resolve module for use by background tier resolution.
///
/// # Errors
/// Returns `ResiduumError::Config` if the thinking string is invalid or temperature
/// is out of range.
pub(super) fn extract_role_overrides_pub(
    role: &str,
    assignment: &ModelAssignment,
    overrides: &mut HashMap<String, RoleOverrides>,
) -> Result<(), ResiduumError> {
    extract_role_overrides(role, assignment, overrides)
}

/// Extract per-role overrides from a `ModelAssignment` and insert into the map.
///
/// # Errors
/// Returns `ResiduumError::Config` if the thinking string is invalid or temperature
/// is out of range.
fn extract_role_overrides(
    role: &str,
    assignment: &ModelAssignment,
    overrides: &mut HashMap<String, RoleOverrides>,
) -> Result<(), ResiduumError> {
    let (temp, thinking_str) = assignment.overrides();
    if temp.is_none() && thinking_str.is_none() {
        return Ok(());
    }

    if let Some(t) = temp
        && !(0.0..=2.0).contains(&t)
    {
        return Err(ResiduumError::Config(format!(
            "invalid temperature for role '{role}': {t} (expected 0.0–2.0)"
        )));
    }

    let thinking = thinking_str.map(super::parse_thinking_config).transpose()?;

    overrides.insert(
        role.to_string(),
        RoleOverrides {
            temperature: temp,
            thinking,
        },
    );
    Ok(())
}

/// Resolve a role's provider chain from a `ModelAssignment`:
/// explicit role > default > clone of main chain.
///
/// Also extracts per-role overrides into the map.
///
/// # Errors
/// Returns `ResiduumError::Config` if any model string cannot be resolved.
fn resolve_role_chain_from_assignment(
    role_spec: Option<ModelAssignment>,
    default_spec: Option<&ModelAssignment>,
    main: &[ProviderSpec],
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
    role: &str,
    overrides: &mut HashMap<String, RoleOverrides>,
) -> Result<Vec<ProviderSpec>, ResiduumError> {
    if let Some(spec) = role_spec {
        extract_role_overrides(role, &spec, overrides)?;
        return resolve_assignment_chain(spec, providers_map, secrets);
    }
    if let Some(spec) = default_spec {
        tracing::debug!(role, "using models.default for role");
        return resolve_assignment_chain(spec.clone(), providers_map, secrets);
    }
    tracing::debug!(role, "using main chain for role");
    Ok(main.to_vec())
}

/// Get the provider-specific API key from environment variables.
fn provider_api_key_env(kind: ProviderKind) -> Option<String> {
    match kind {
        ProviderKind::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
        ProviderKind::Gemini => std::env::var("GEMINI_API_KEY").ok(),
        ProviderKind::OpenAi => std::env::var("OPENAI_API_KEY").ok(),
        ProviderKind::Ollama => std::env::var("OLLAMA_API_KEY").ok(),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes into known-length vecs for clarity"
)]
#[expect(
    unsafe_code,
    reason = "std::env::set_var/remove_var require unsafe in edition 2024"
)]
mod tests {
    use super::super::super::constants::DEFAULT_ANTHROPIC_URL;
    use super::super::super::deserialize::{ConfigFile, ProvidersFile};
    use super::super::from_file_and_env;
    use super::*;

    /// Create an empty `SecretStore` for tests that don't need real secrets.
    fn empty_secrets() -> SecretStore {
        let dir = std::env::temp_dir().join("residuum-test-empty-secrets");
        SecretStore::load(&dir).unwrap()
    }

    /// Create a temp dir for `from_file_and_env` calls.
    fn test_config_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("residuum-test-config")
    }

    /// Parse a TOML string into a `ConfigFile` (config-only: timezone, memory, etc.).
    fn parse_config(toml: &str) -> ConfigFile {
        toml::from_str(toml).unwrap()
    }

    /// Parse a TOML string into a `ProvidersFile` (providers and models sections).
    fn parse_providers(toml: &str) -> ProvidersFile {
        toml::from_str(toml).unwrap()
    }

    // ── Provider / model resolution ───────────────────────────────────────────

    #[test]
    fn implicit_provider_resolution() {
        let secrets = empty_secrets();
        let spec = resolve_model_string("anthropic/claude-sonnet-4-6", None, &secrets).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::Anthropic);
        assert_eq!(spec.model.model, "claude-sonnet-4-6");
        assert_eq!(spec.provider_url, DEFAULT_ANTHROPIC_URL);
        assert_eq!(spec.name, "anthropic");
    }

    #[test]
    fn explicit_provider_resolution() {
        let secrets = empty_secrets();
        let mut providers = HashMap::new();
        providers.insert(
            "my-claude".to_string(),
            ProviderEntryFile {
                kind: "anthropic".to_string(),
                api_key: Some("sk-explicit".to_string()),
                url: None,
                keep_alive: None,
            },
        );

        let spec = resolve_model_string("my-claude/claude-sonnet-4-6", Some(&providers), &secrets)
            .unwrap();
        assert_eq!(spec.model.kind, ProviderKind::Anthropic);
        assert_eq!(spec.model.model, "claude-sonnet-4-6");
        assert_eq!(spec.name, "my-claude");
        assert_eq!(spec.api_key.as_deref(), Some("sk-explicit"));
        assert_eq!(spec.provider_url, DEFAULT_ANTHROPIC_URL);
    }

    #[test]
    fn unknown_implicit_provider_errors() {
        let secrets = empty_secrets();
        let result = resolve_model_string("foobar/some-model", None, &secrets);
        assert!(result.is_err(), "unknown implicit provider should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("foobar"),
            "error should mention the bad provider: {err}"
        );
    }

    #[test]
    fn explicit_provider_url_override() {
        let secrets = empty_secrets();
        let mut providers = HashMap::new();
        providers.insert(
            "cerebras".to_string(),
            ProviderEntryFile {
                kind: "openai".to_string(),
                api_key: Some("csk-123".to_string()),
                url: Some("https://api.cerebras.ai/v1".to_string()),
                keep_alive: None,
            },
        );

        let spec = resolve_model_string("cerebras/llama-4", Some(&providers), &secrets).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::OpenAi);
        assert_eq!(spec.provider_url, "https://api.cerebras.ai/v1");
    }

    // ── Full config resolution via from_file_and_env ──────────────────────────

    #[test]
    fn default_model_fallback() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        // observer was not set, so it falls back to default
        assert_eq!(cfg.observer[0].model.model, "claude-haiku-4-5");
        assert_eq!(cfg.reflector[0].model.model, "claude-haiku-4-5");
        assert_eq!(cfg.pulse[0].model.model, "claude-haiku-4-5");
        // main is still the explicit main
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn role_specific_overrides_default() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
observer = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.observer[0].model.model, "gemini-3.0-flash",
            "explicit observer should override default"
        );
        assert_eq!(
            cfg.reflector[0].model.model, "claude-haiku-4-5",
            "unset reflector should still use default"
        );
    }

    #[test]
    fn all_roles_resolved_to_main_by_default() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.observer[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.reflector[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.pulse[0].model.model, "claude-sonnet-4-6");
    }

    // ── Failover chain resolution ──────────────────────────────────────────

    #[test]
    fn model_chain_single_string() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.main.len(),
            1,
            "single string should produce 1-element chain"
        );
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn model_chain_array() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(cfg.main.len(), 2, "array should produce 2-element chain");
        assert_eq!(cfg.main[0].model.kind, ProviderKind::Anthropic);
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.main[1].model.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.main[1].model.model, "gpt-4o");
    }

    #[test]
    fn role_chain_inherits_main_chain() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.observer.len(),
            2,
            "observer should inherit main chain length"
        );
        assert_eq!(cfg.observer[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.observer[1].model.model, "gpt-4o");
    }

    #[test]
    fn role_chain_overrides_main_chain() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
observer = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.observer.len(),
            1,
            "explicit observer should override main chain"
        );
        assert_eq!(cfg.observer[0].model.model, "gemini-3.0-flash");
    }

    #[test]
    fn deny_unknown_fields_rejects_typos() {
        let toml_str = r#"
[models]
main = "anthropic/claude-sonnet-4-6"
typo_field = "oops"
"#;
        let result = toml::from_str::<ProvidersFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [models] should be rejected"
        );
    }

    #[test]
    fn provider_entry_type_field() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[providers.cerebras]
type = "openai"
api_key = "csk-123"
url = "https://api.cerebras.ai/v1"

[models]
main = "cerebras/llama-4"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(cfg.main[0].model.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.main[0].provider_url, "https://api.cerebras.ai/v1");
    }

    // ── Embedding config ──────────────────────────────────────────────────

    #[test]
    fn embedding_role_resolved() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
embedding = "openai/text-embedding-3-small"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let emb = cfg.embedding.as_ref();
        assert!(emb.is_some(), "embedding should be resolved");
        let emb = emb.unwrap();
        assert_eq!(emb.model.kind, ProviderKind::OpenAi);
        assert_eq!(emb.model.model, "text-embedding-3-small");
    }

    #[test]
    fn embedding_anthropic_rejected() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
embedding = "anthropic/some-model"
"#,
        );
        let result = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir());
        assert!(result.is_err(), "anthropic embedding should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("anthropic"),
            "error should mention anthropic: {err}"
        );
    }

    #[test]
    fn embedding_absent_is_none() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.embedding.is_none(),
            "missing embedding should yield None"
        );
    }

    #[test]
    fn embedding_no_fallback_to_default() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "openai/gpt-4o"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.embedding.is_none(),
            "embedding should not fall back to default"
        );
    }

    // ── Secret / env expansion in providers ──────────────────────────────────

    #[test]
    fn provider_api_key_env_expansion() {
        let secrets = empty_secrets();
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("RESIDUUM_TEST_PROVIDER_KEY", "expanded-key") };

        let mut providers = HashMap::new();
        providers.insert(
            "test-prov".to_string(),
            ProviderEntryFile {
                kind: "openai".to_string(),
                api_key: Some("${RESIDUUM_TEST_PROVIDER_KEY}".to_string()),
                url: None,
                keep_alive: None,
            },
        );

        let spec = resolve_model_string("test-prov/gpt-4o", Some(&providers), &secrets).unwrap();
        assert_eq!(
            spec.api_key.as_deref(),
            Some("expanded-key"),
            "env var in api_key should expand"
        );
        unsafe { std::env::remove_var("RESIDUUM_TEST_PROVIDER_KEY") };
    }

    #[test]
    fn provider_api_key_secret_ref() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();
        store.set("my_openai", "sk-from-store", dir.path()).unwrap();

        let mut providers = HashMap::new();
        providers.insert(
            "test-prov".to_string(),
            ProviderEntryFile {
                kind: "openai".to_string(),
                api_key: Some("secret:my_openai".to_string()),
                url: None,
                keep_alive: None,
            },
        );

        let spec = resolve_model_string("test-prov/gpt-4o", Some(&providers), &store).unwrap();
        assert_eq!(
            spec.api_key.as_deref(),
            Some("sk-from-store"),
            "secret:name in api_key should resolve from store"
        );
    }

    #[test]
    fn provider_api_key_missing_secret_falls_back() {
        let secrets = empty_secrets();
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("OPENAI_API_KEY", "fallback-env-key") };

        let mut providers = HashMap::new();
        providers.insert(
            "test-prov".to_string(),
            ProviderEntryFile {
                kind: "openai".to_string(),
                api_key: Some("secret:nonexistent".to_string()),
                url: None,
                keep_alive: None,
            },
        );

        let spec = resolve_model_string("test-prov/gpt-4o", Some(&providers), &secrets).unwrap();
        assert_eq!(
            spec.api_key.as_deref(),
            Some("fallback-env-key"),
            "missing secret should fall back to provider env var"
        );
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    // ── ModelAssignment deserialization and overrides ──────────────────────

    #[test]
    fn model_assignment_simple_string() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.observer[0].model.model, "gemini-3.0-flash");
        assert!(
            cfg.role_overrides.is_empty(),
            "no overrides for simple strings"
        );
    }

    #[test]
    fn model_assignment_with_overrides() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", temperature = 0.2, thinking = "off" }
reflector = { model = "anthropic/claude-sonnet-4-6", thinking = "low" }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        assert_eq!(cfg.observer[0].model.model, "gemini-3.0-flash");
        assert_eq!(cfg.reflector[0].model.model, "claude-sonnet-4-6");

        let obs_ov = &cfg.role_overrides["observer"];
        assert_eq!(obs_ov.temperature, Some(0.2));
        assert_eq!(
            obs_ov.thinking,
            Some(crate::models::ThinkingConfig::Toggle(false))
        );

        let ref_ov = &cfg.role_overrides["reflector"];
        assert_eq!(ref_ov.temperature, None);
        assert_eq!(
            ref_ov.thinking,
            Some(crate::models::ThinkingConfig::Level(
                crate::models::ThinkingLevel::Low
            ))
        );
    }

    #[test]
    fn model_assignment_table_no_overrides() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
pulse = { model = "anthropic/claude-haiku" }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        assert_eq!(cfg.pulse[0].model.model, "claude-haiku");
        assert!(
            !cfg.role_overrides.contains_key("pulse"),
            "table with no overrides should not create entry"
        );
    }

    #[test]
    fn model_assignment_table_with_list() {
        let prov_file = parse_providers(
            r#"
[models]
main = { model = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"], temperature = 1.0 }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        assert_eq!(
            cfg.main.len(),
            2,
            "should resolve failover chain from table"
        );
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.main[1].model.model, "gpt-4o");

        let main_ov = &cfg.role_overrides["main"];
        assert_eq!(main_ov.temperature, Some(1.0));
    }

    #[test]
    fn model_assignment_invalid_temperature() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", temperature = 3.0 }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let result = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir());
        assert!(result.is_err(), "temperature > 2.0 should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("temperature"),
            "error should mention temperature: {err}"
        );
    }

    #[test]
    fn model_assignment_invalid_thinking() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", thinking = "turbo" }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let result = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir());
        assert!(result.is_err(), "invalid thinking value should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("thinking"),
            "error should mention thinking: {err}"
        );
    }

    #[test]
    fn background_model_with_overrides() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"

[background.models]
small = { model = "ollama/llama3", thinking = "off" }
medium = "anthropic/claude-haiku"
large = { model = "anthropic/claude-sonnet-4-6", thinking = "medium" }
"#,
        );
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        assert!(cfg.background.models.small.is_some());
        assert!(cfg.background.models.medium.is_some());
        assert!(cfg.background.models.large.is_some());

        let bg_small_ov = &cfg.role_overrides["bg_small"];
        assert_eq!(
            bg_small_ov.thinking,
            Some(crate::models::ThinkingConfig::Toggle(false))
        );
        assert!(!cfg.role_overrides.contains_key("bg_medium"));
        let bg_large_ov = &cfg.role_overrides["bg_large"];
        assert_eq!(
            bg_large_ov.thinking,
            Some(crate::models::ThinkingConfig::Level(
                crate::models::ThinkingLevel::Medium
            ))
        );
    }

    // ── completion_options_for_role ────────────────────────────────────────

    #[test]
    fn completion_options_for_role_with_override() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", temperature = 0.2 }
"#,
        );
        let cfg_file = parse_config(
            r#"
timezone = "UTC"
temperature = 0.8
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        let main_opts = cfg.completion_options_for_role("main");
        assert_eq!(
            main_opts.temperature,
            Some(0.8),
            "main should use global temp"
        );

        let obs_opts = cfg.completion_options_for_role("observer");
        assert_eq!(
            obs_opts.temperature,
            Some(0.2),
            "observer should use override temp"
        );
    }

    #[test]
    fn completion_options_for_role_fallback_to_global() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg_file = parse_config(
            r#"
timezone = "UTC"
temperature = 0.5
thinking = "medium"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();

        let opts = cfg.completion_options_for_role("reflector");
        assert_eq!(opts.temperature, Some(0.5), "should fall back to global");
        assert_eq!(
            opts.thinking,
            Some(crate::models::ThinkingConfig::Level(
                crate::models::ThinkingLevel::Medium
            )),
            "should fall back to global thinking"
        );
    }
}

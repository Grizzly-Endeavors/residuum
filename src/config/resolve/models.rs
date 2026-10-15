//! Model and provider chain resolution.
//!
//! Resolves `"provider/model"` strings and `[models]` config into fully-built
//! `ProviderSpec` values with failover chains.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use crate::config::types::{BackgroundModelsConfig, RoleOverrides};
use crate::util::FatalError;

use super::super::deserialize::{ModelAssignment, ModelsConfigFile, ProviderEntryFile};
use super::super::model_catalog::{NamedProvider, implicit_provider_spec, split_model_id};
use super::super::provider::{ProviderKind, ProviderSpec};
use super::super::secrets::SecretStore;

/// Resolved model provider specs for all roles.
pub(super) struct ResolvedModels {
    pub(super) main: Vec<ProviderSpec>,
    pub(super) observer: Vec<ProviderSpec>,
    pub(super) reflector: Vec<ProviderSpec>,
    pub(super) pulse: Vec<ProviderSpec>,
    pub(super) subconscious: Vec<ProviderSpec>,
    pub(super) embedding: Option<ProviderSpec>,
    pub(super) role_overrides: HashMap<String, RoleOverrides>,
}

/// Resolve all model specs (main, observer, reflector, pulse, embedding) from the
/// `[models]` config section.
///
/// # Errors
/// Returns `FatalError::Config` if any model string is invalid or an unsupported
/// provider is used for embeddings.
pub(super) fn resolve_all_model_specs(
    models: Option<&ModelsConfigFile>,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<ResolvedModels, FatalError> {
    let mut role_overrides = HashMap::new();

    // Resolve main: models.main > default
    let main = if let Some(main_spec) = models.and_then(|m| m.main.clone()) {
        extract_role_overrides("main", &main_spec, &mut role_overrides)?;
        resolve_assignment_chain(main_spec, providers_map, secrets)?
    } else {
        vec![resolve_model_string(
            "anthropic/claude-sonnet-4-6",
            providers_map,
            secrets,
        )?]
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
    let subconscious = resolve_role_chain_from_assignment(
        models.and_then(|m| m.subconscious.clone()),
        default_assignment.as_ref(),
        &main,
        providers_map,
        secrets,
        "subconscious",
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
        return Err(FatalError::Config(
            "anthropic does not offer an embeddings API; \
             use openai, fireworks, ollama, or gemini for models.embedding"
                .to_string(),
        ));
    }

    Ok(ResolvedModels {
        main,
        observer,
        reflector,
        pulse,
        subconscious,
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
///   (e.g. `"anthropic"`). API key falls back to the provider-specific env var.
///
/// # Errors
/// Returns `FatalError::Config` if the model string format is invalid,
/// the provider is unknown, or an explicit provider entry references an
/// unknown type.
fn resolve_model_string(
    model_str: &str,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<ProviderSpec, FatalError> {
    let (provider_part, model_name) = split_model_id(model_str).map_err(FatalError::Config)?;
    match providers_map.and_then(|p| p.get(provider_part)) {
        Some(entry) => NamedProvider::from_entry(entry, secrets).spec(provider_part, model_name),
        None => implicit_provider_spec(provider_part, model_name),
    }
    .map_err(FatalError::Config)
}

/// Resolve a `ModelAssignment` into a `Vec<ProviderSpec>` (failover chain).
///
/// # Errors
/// Returns `FatalError::Config` if any model string in the assignment cannot be resolved.
pub(super) fn resolve_assignment_chain(
    assignment: ModelAssignment,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<Vec<ProviderSpec>, FatalError> {
    assignment
        .into_model_strings()
        .iter()
        .map(|s| resolve_model_string(s, providers_map, secrets))
        .collect()
}

/// Extract per-role overrides from a `ModelAssignment` and insert into the map.
///
/// # Errors
/// Returns `FatalError::Config` if the thinking string is invalid or temperature
/// is out of range.
pub(super) fn extract_role_overrides(
    role: &str,
    assignment: &ModelAssignment,
    overrides: &mut HashMap<String, RoleOverrides>,
) -> Result<(), FatalError> {
    let (temp, thinking_str) = assignment.overrides();
    if temp.is_none() && thinking_str.is_none() {
        return Ok(());
    }

    if let Some(t) = temp
        && !(0.0..=2.0).contains(&t)
    {
        return Err(FatalError::Config(format!(
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
/// Returns `FatalError::Config` if any model string cannot be resolved.
fn resolve_role_chain_from_assignment(
    role_spec: Option<ModelAssignment>,
    default_spec: Option<&ModelAssignment>,
    main: &[ProviderSpec],
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
    role: &str,
    overrides: &mut HashMap<String, RoleOverrides>,
) -> Result<Vec<ProviderSpec>, FatalError> {
    if let Some(spec) = role_spec {
        extract_role_overrides(role, &spec, overrides)?;
        return resolve_assignment_chain(spec, providers_map, secrets);
    }
    if let Some(spec) = default_spec {
        tracing::debug!(role = %role, "using models.default for role");
        return resolve_assignment_chain(spec.clone(), providers_map, secrets);
    }
    tracing::debug!(role = %role, "using main chain for role");
    Ok(main.to_vec())
}

/// Give every role's chain, including background tiers, its session-affinity key.
pub(super) fn scope_all_session_affinity(
    models: &mut ResolvedModels,
    background: &mut BackgroundModelsConfig,
    workspace_dir: &Path,
) {
    for (specs, role) in [
        (&mut models.main, "main"),
        (&mut models.observer, "observer"),
        (&mut models.reflector, "reflector"),
        (&mut models.pulse, "pulse"),
        (&mut models.subconscious, "subconscious"),
    ] {
        scope_session_affinity(specs, workspace_dir, role);
    }
    for (tier, role) in [
        (&mut background.small, "bg_small"),
        (&mut background.medium, "bg_medium"),
        (&mut background.large, "bg_large"),
    ] {
        if let Some(specs) = tier {
            scope_session_affinity(specs, workspace_dir, role);
        }
    }
}

/// A role's session-affinity key: `residuum-<role>-<hash of the workspace path>`.
pub(in crate::config) fn session_affinity_key(workspace_dir: &Path, role: &str) -> String {
    let mut hasher = DefaultHasher::new();
    workspace_dir.hash(&mut hasher);
    format!("residuum-{role}-{:016x}", hasher.finish())
}

/// Stamp every spec in a role's chain with that role's session-affinity key.
///
/// The key hashes the workspace path so no filesystem detail leaves the
/// machine, and includes the role because each role sends a different stable
/// prompt prefix. Runs after role resolution because roles without their own
/// assignment clone the main chain, including main's key.
fn scope_session_affinity(specs: &mut [ProviderSpec], workspace_dir: &Path, role: &str) {
    let key = session_affinity_key(workspace_dir, role);
    for spec in specs {
        spec.session_affinity = Some(key.clone());
    }
}

#[cfg(test)]
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
    use super::super::super::provider::ImageInput;
    use super::super::from_file_and_env;
    use super::super::test_helpers::*;
    use super::*;

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
                models: None,
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
                models: None,
            },
        );

        let spec = resolve_model_string("cerebras/llama-4", Some(&providers), &secrets).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::OpenAi);
        assert_eq!(spec.provider_url, "https://api.cerebras.ai/v1");
    }

    // ── Per-model input declarations ─────────────────────────────────────────

    fn chain_with_models(toml: &str) -> Vec<ProviderSpec> {
        let prov_file = parse_providers(toml);
        let cfg = from_file_and_env(
            Some(&parse_config("")),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        cfg.main
    }

    #[test]
    fn model_input_declarations_resolve_per_model() {
        let main = chain_with_models(
            r#"
[providers.zai]
type = "openai"
url = "https://api.z.ai/api/paas/v4"

[providers.zai.models."glm-5.3"]
input = ["text"]

[providers.zai.models."glm-5v"]
input = ["text", "image"]

[models]
main = ["zai/glm-5.3", "zai/glm-5v", "zai/glm-other", "ollama/llama3"]
"#,
        );
        let inputs: Vec<ImageInput> = main.iter().map(|spec| spec.image_input).collect();
        assert_eq!(
            inputs,
            vec![
                ImageInput::TextOnly,
                ImageInput::Accepted,
                ImageInput::Undeclared,
                ImageInput::Undeclared,
            ],
            "each model in the chain takes its own declaration; undeclared and implicit-provider models stay undeclared"
        );
    }

    #[test]
    fn model_input_without_image_is_text_only() {
        let main = chain_with_models(
            r#"
[providers.local]
type = "ollama"
models = { "qwen3" = { input = [] } }

[models]
main = "local/qwen3"
"#,
        );
        assert_eq!(main[0].image_input, ImageInput::TextOnly);
    }

    /// Apply the patch the web Settings sends for a model's image setting.
    fn patched(toml: &str, models: &serde_json::Value) -> String {
        let diff = serde_json::json!({ "providers": { "zai": { "models": models.clone() } } });
        crate::config::patch::apply_patch(toml, diff.as_object().unwrap(), "providers.toml")
            .unwrap()
    }

    #[test]
    fn model_input_round_trips_through_a_settings_patch() {
        for original in [
            "[providers.zai]\ntype = \"openai\"\n\n[models]\nmain = \"zai/glm-5.3\"\n",
            "[providers]\nzai = { type = \"openai\" }\n\n[models]\nmain = \"zai/glm-5.3\"\n",
        ] {
            let declared = patched(
                original,
                &serde_json::json!({ "glm-5.3": { "input": ["text"] } }),
            );
            assert_eq!(
                chain_with_models(&declared)[0].image_input,
                ImageInput::TextOnly,
                "declared text-only from:\n{original}\nas:\n{declared}"
            );

            let cleared = patched(&declared, &serde_json::json!({ "glm-5.3": null }));
            assert_eq!(
                chain_with_models(&cleared)[0].image_input,
                ImageInput::Undeclared,
                "cleared again as:\n{cleared}"
            );
            assert!(
                !cleared.contains("models.") && !cleared.contains("models ="),
                "clearing the last model leaves no empty models table:\n{cleared}"
            );
        }
    }

    #[test]
    fn model_input_rejects_an_unknown_kind() {
        let err = toml::from_str::<super::super::super::deserialize::ProvidersFile>(
            r#"
[providers.zai]
type = "openai"

[providers.zai.models."glm-5.3"]
input = ["text", "imgae"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("imgae") && err.contains("image"),
            "the error should name the bad value and the accepted ones: {err}"
        );
    }

    #[test]
    fn model_entry_rejects_unknown_keys() {
        let result = toml::from_str::<super::super::super::deserialize::ProvidersFile>(
            r#"
[providers.zai]
type = "openai"

[providers.zai.models."glm-5.3"]
inputs = ["text"]
"#,
        );
        assert!(
            result.is_err(),
            "a misspelled per-model key should not load silently"
        );
    }

    // ── Full config resolution via from_file_and_env ──────────────────────────

    #[test]
    fn default_model_fallback() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        // observer was not set, so it falls back to default
        assert_eq!(cfg.observer[0].model.model, "claude-haiku-4-5");
        assert_eq!(cfg.reflector[0].model.model, "claude-haiku-4-5");
        assert_eq!(cfg.pulse[0].model.model, "claude-haiku-4-5");
        assert_eq!(cfg.subconscious[0].model.model, "claude-haiku-4-5");
        // main is still the explicit main
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn role_specific_overrides_default() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
observer = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
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
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.observer[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.reflector[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.pulse[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.subconscious[0].model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn subconscious_role_overrides_default() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
subconscious = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert_eq!(
            cfg.subconscious[0].model.model, "gemini-3.0-flash",
            "explicit subconscious should override default"
        );
        assert_eq!(
            cfg.observer[0].model.model, "claude-haiku-4-5",
            "unset observer should still use default"
        );
    }

    // ── Failover chain resolution ──────────────────────────────────────────

    #[test]
    fn model_chain_single_string() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert_eq!(
            cfg.main.len(),
            1,
            "single string should produce 1-element chain"
        );
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn model_chain_array() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert_eq!(cfg.main.len(), 2, "array should produce 2-element chain");
        assert_eq!(cfg.main[0].model.kind, ProviderKind::Anthropic);
        assert_eq!(cfg.main[0].model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.main[1].model.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.main[1].model.model, "gpt-4o");
    }

    #[test]
    fn role_chain_inherits_main_chain() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
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
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
observer = "gemini/gemini-3.0-flash"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
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
        let result = toml::from_str::<super::super::super::deserialize::ProvidersFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [models] should be rejected"
        );
    }

    #[test]
    fn provider_entry_type_field() {
        let cfg_file = parse_config("");
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
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert_eq!(cfg.main[0].model.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.main[0].provider_url, "https://api.cerebras.ai/v1");
    }

    // ── Embedding config ──────────────────────────────────────────────────

    #[test]
    fn embedding_role_resolved() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
embedding = "openai/text-embedding-3-small"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        let emb = cfg.embedding.as_ref();
        assert!(emb.is_some(), "embedding should be resolved");
        let emb = emb.unwrap();
        assert_eq!(emb.model.kind, ProviderKind::OpenAi);
        assert_eq!(emb.model.model, "text-embedding-3-small");
    }

    #[test]
    fn embedding_anthropic_rejected() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
embedding = "anthropic/some-model"
"#,
        );
        let result = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        );
        assert!(result.is_err(), "anthropic embedding should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("anthropic"),
            "error should mention anthropic: {err}"
        );
    }

    #[test]
    fn embedding_absent_is_none() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert!(
            cfg.embedding.is_none(),
            "missing embedding should yield None"
        );
    }

    #[test]
    fn embedding_no_fallback_to_default() {
        let cfg_file = parse_config("");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
default = "openai/gpt-4o"
"#,
        );
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();
        assert!(
            cfg.embedding.is_none(),
            "embedding should not fall back to default"
        );
    }

    // ── Secret / env expansion in providers ──────────────────────────────────

    #[test]
    fn provider_api_key_env_expansion() {
        let _guard = ENV_MUTEX.lock().unwrap();
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
                models: None,
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
                models: None,
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
        let _guard = ENV_MUTEX.lock().unwrap();
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
                models: None,
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
        let cfg_file = parse_config("");
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

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
        let cfg_file = parse_config("");
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

        assert_eq!(cfg.observer[0].model.model, "gemini-3.0-flash");
        assert_eq!(cfg.reflector[0].model.model, "claude-sonnet-4-6");

        let obs_ov = &cfg.role_overrides["observer"];
        assert_eq!(obs_ov.temperature, Some(0.2));
        assert_eq!(
            obs_ov.thinking,
            Some(crate::config::ThinkingConfig::Toggle(false))
        );

        let ref_ov = &cfg.role_overrides["reflector"];
        assert_eq!(ref_ov.temperature, None);
        assert_eq!(
            ref_ov.thinking,
            Some(crate::config::ThinkingConfig::Level(
                crate::config::ThinkingLevel::Low
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
        let cfg_file = parse_config("");
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

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
        let cfg_file = parse_config("");
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

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
        let cfg_file = parse_config("");
        let result = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        );
        assert!(result.is_err(), "temperature > 2.0 should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("temperature"),
            "error should mention temperature: {err}"
        );
    }

    #[test]
    fn model_assignment_invalid_temperature_negative() {
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", temperature = -0.1 }
"#,
        );
        let cfg_file = parse_config("");
        let result = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        );
        assert!(result.is_err(), "temperature < 0.0 should error");
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
        let cfg_file = parse_config("");
        let result = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        );
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
        let cfg_file = parse_config("");
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

        assert!(cfg.background.models.small.is_some());
        assert!(cfg.background.models.medium.is_some());
        assert!(cfg.background.models.large.is_some());

        let bg_small_ov = &cfg.role_overrides["bg_small"];
        assert_eq!(
            bg_small_ov.thinking,
            Some(crate::config::ThinkingConfig::Toggle(false))
        );
        assert!(!cfg.role_overrides.contains_key("bg_medium"));
        let bg_large_ov = &cfg.role_overrides["bg_large"];
        assert_eq!(
            bg_large_ov.thinking,
            Some(crate::config::ThinkingConfig::Level(
                crate::config::ThinkingLevel::Medium
            ))
        );
    }
}

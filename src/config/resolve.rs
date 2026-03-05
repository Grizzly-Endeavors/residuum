//! Config resolution logic: maps raw TOML structs + env vars into validated Config.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::ResiduumError;
use crate::models::retry::RetryConfig;

use super::Config;
use super::bootstrap::default_workspace_dir;
use super::constants::{DEFAULT_IDLE_TIMEOUT_MINUTES, DEFAULT_MAX_TOKENS, DEFAULT_TIMEOUT_SECS};
use super::deserialize::{
    AgentConfigFile, BackgroundConfigFile, BackgroundModelsFile, ConfigFile, DiscordConfigFile,
    GatewayConfigFile, ModelStringOrList, ProviderEntryFile, ProvidersFile, SearchConfigFile,
    SkillsConfigFile, TelegramConfigFile, WebhookConfigFile,
};
use super::provider::{ModelSpec, ProviderKind, ProviderSpec};
use super::secrets::SecretStore;
use super::types::{
    AgentAbilitiesConfig, BackgroundConfig, DiscordConfig, GatewayConfig, IdleConfig, MemoryConfig,
    SearchConfig, SkillsConfig, TelegramConfig, WebhookConfig,
};

/// Build a `Config` from an optional config file and environment variables.
///
/// # Errors
/// Returns `ResiduumError::Config` if the model spec cannot be parsed or
/// the workspace directory cannot be determined.
#[expect(
    clippy::too_many_lines,
    reason = "config resolution is a single sequential pipeline; splitting would obscure the precedence chain"
)]
pub(crate) fn from_file_and_env(
    file: Option<&ConfigFile>,
    providers_file: Option<&ProvidersFile>,
    config_dir: &Path,
) -> Result<Config, ResiduumError> {
    warn_deprecated_env_vars();

    let secrets = SecretStore::load(config_dir)?;
    let providers_map = providers_file.and_then(|f| f.providers.as_ref());
    let models = providers_file.and_then(|f| f.models.as_ref());

    // Resolve main: RESIDUUM_MODEL env > models.main > default
    let main = if let Ok(env_model) = std::env::var("RESIDUUM_MODEL") {
        vec![resolve_model_string(&env_model, providers_map, &secrets)?]
    } else if let Some(main_spec) = models.and_then(|m| m.main.clone()) {
        resolve_model_chain(main_spec, providers_map, &secrets)?
    } else {
        vec![resolve_model_string(
            "anthropic/claude-sonnet-4-6",
            providers_map,
            &secrets,
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
    let default_chain = models.and_then(|m| m.default.clone());

    let observer = resolve_role_chain(
        models.and_then(|m| m.observer.clone()),
        default_chain.as_ref(),
        &main,
        providers_map,
        &secrets,
    )?;
    let reflector = resolve_role_chain(
        models.and_then(|m| m.reflector.clone()),
        default_chain.as_ref(),
        &main,
        providers_map,
        &secrets,
    )?;
    let pulse_spec = resolve_role_chain(
        models.and_then(|m| m.pulse.clone()),
        default_chain.as_ref(),
        &main,
        providers_map,
        &secrets,
    )?;
    // Resolve embedding: models.embedding only, no fallback to default or main
    let embedding = models
        .and_then(|m| m.embedding.as_deref())
        .map(|s| resolve_model_string(s, providers_map, &secrets))
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

    // Workspace dir: env > file > default
    let workspace_dir = std::env::var("RESIDUUM_WORKSPACE")
        .ok()
        .or_else(|| file.and_then(|f| f.workspace_dir.clone()))
        .map(|s| {
            let expanded = shellexpand::tilde(&s);
            PathBuf::from(expanded.as_ref())
        })
        .map_or_else(default_workspace_dir, Ok)?;

    let timeout_secs = file
        .and_then(|f| f.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let max_tokens = file
        .and_then(|f| f.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let memory = {
        let mem_section = file.and_then(|f| f.memory.as_ref());
        let mut mem = MemoryConfig::default();
        if let Some(s) = mem_section {
            if let Some(v) = s.observer_threshold_tokens {
                mem.observer_threshold_tokens = v;
            }
            if let Some(v) = s.reflector_threshold_tokens {
                mem.reflector_threshold_tokens = v;
            }
            if let Some(v) = s.observer_cooldown_secs {
                mem.observer_cooldown_secs = v;
            }
            if let Some(v) = s.observer_force_threshold_tokens {
                mem.observer_force_threshold_tokens = v;
            }
        }
        mem.search = resolve_search_config(mem_section.and_then(|m| m.search.as_ref()));
        mem
    };

    let pulse_enabled = file
        .and_then(|f| f.pulse.as_ref())
        .and_then(|p| p.enabled)
        .unwrap_or(true);

    let gateway = resolve_gateway_config(file.and_then(|f| f.gateway.as_ref()));
    let discord = resolve_discord_config(file.and_then(|f| f.discord.as_ref()), &secrets);
    let telegram = resolve_telegram_config(file.and_then(|f| f.telegram.as_ref()), &secrets);
    let webhook = resolve_webhook_config(file.and_then(|f| f.webhook.as_ref()), &secrets);
    let skills = resolve_skills_config(file.and_then(|f| f.skills.as_ref()), &workspace_dir);

    let agent = resolve_agent_config(file.and_then(|f| f.agent.as_ref()));

    let idle = {
        let section = file.and_then(|f| f.idle.as_ref());
        let timeout_minutes = section
            .and_then(|s| s.timeout_minutes)
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_MINUTES);
        let idle_channel = section.and_then(|s| s.idle_channel.clone());

        if let Some(ref channel) = idle_channel {
            let valid = match channel.as_str() {
                "telegram" => telegram.is_some(),
                "discord" => discord.is_some(),
                "websocket" => true,
                other => {
                    return Err(ResiduumError::Config(format!(
                        "idle_channel \"{other}\" is not a recognized interface"
                    )));
                }
            };
            if !valid {
                return Err(ResiduumError::Config(format!(
                    "idle_channel \"{channel}\" configured but [{channel}] section is missing"
                )));
            }
        }

        IdleConfig {
            timeout: std::time::Duration::from_secs(timeout_minutes * 60),
            idle_channel,
        }
    };

    let background = resolve_background_config(
        file.and_then(|f| f.background.as_ref()),
        providers_file
            .and_then(|pf| pf.background.as_ref())
            .and_then(|b| b.models.as_ref()),
        providers_map,
        &secrets,
    )?;

    let retry = {
        let r = file.and_then(|f| f.retry.as_ref());
        let mut cfg = RetryConfig::default();
        if let Some(v) = r.and_then(|r| r.max_retries) {
            cfg.max_retries = v;
        }
        if let Some(v) = r.and_then(|r| r.initial_delay_ms) {
            cfg.initial_delay = std::time::Duration::from_millis(v);
        }
        if let Some(v) = r.and_then(|r| r.max_delay_ms) {
            cfg.max_delay = std::time::Duration::from_millis(v);
        }
        if let Some(v) = r.and_then(|r| r.backoff_multiplier) {
            cfg.backoff_multiplier = v;
        }
        cfg
    };

    let timezone_str = std::env::var("RESIDUUM_TIMEZONE")
        .ok()
        .or_else(|| file.and_then(|f| f.timezone.clone()));
    let tz_name = timezone_str.ok_or_else(|| {
        ResiduumError::Config(
            "timezone is required: set RESIDUUM_TIMEZONE env var or 'timezone' in config.toml \
             (IANA name, e.g. \"America/New_York\")"
                .to_string(),
        )
    })?;
    let timezone: chrono_tz::Tz = tz_name.parse().map_err(|_err| {
        ResiduumError::Config(format!(
            "invalid timezone '{tz_name}': expected IANA name like 'America/New_York' or 'UTC'"
        ))
    })?;

    let name = file.and_then(|f| f.name.clone());

    Ok(Config {
        name,
        main,
        observer,
        reflector,
        pulse: pulse_spec,
        embedding,
        workspace_dir,
        timeout_secs,
        max_tokens,
        memory,
        pulse_enabled,
        gateway,
        timezone,
        discord,
        telegram,
        webhook,
        skills,
        retry,
        background,
        agent,
        idle,
        config_dir: PathBuf::new(),
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
            .and_then(|raw| resolve_secret_value(raw, secrets))
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
    })
}

/// Resolve a `ModelStringOrList` into a `Vec<ProviderSpec>` (failover chain).
///
/// # Errors
/// Returns `ResiduumError::Config` if any model string in the list cannot be resolved.
fn resolve_model_chain(
    spec: ModelStringOrList,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<Vec<ProviderSpec>, ResiduumError> {
    spec.into_vec()
        .iter()
        .map(|s| resolve_model_string(s, providers_map, secrets))
        .collect()
}

/// Resolve a role's provider chain: explicit role > default > clone of main chain.
///
/// # Errors
/// Returns `ResiduumError::Config` if any model string cannot be resolved.
fn resolve_role_chain(
    role_spec: Option<ModelStringOrList>,
    default_spec: Option<&ModelStringOrList>,
    main: &[ProviderSpec],
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<Vec<ProviderSpec>, ResiduumError> {
    if let Some(spec) = role_spec {
        return resolve_model_chain(spec, providers_map, secrets);
    }
    if let Some(spec) = default_spec {
        return resolve_model_chain(spec.clone(), providers_map, secrets);
    }
    Ok(main.to_vec())
}

/// Resolve gateway configuration from environment variables and defaults only.
///
/// Used by the setup server which runs before any config file exists.
#[must_use]
pub(crate) fn resolve_default_gateway_config() -> GatewayConfig {
    resolve_gateway_config(None)
}

/// Resolve gateway configuration from TOML section and environment variables.
fn resolve_gateway_config(section: Option<&GatewayConfigFile>) -> GatewayConfig {
    let mut cfg = GatewayConfig::default();

    // Env > file > default for bind
    if let Ok(val) = std::env::var("RESIDUUM_GATEWAY_BIND") {
        cfg.bind = val;
    } else if let Some(val) = section.and_then(|s| s.bind.clone()) {
        cfg.bind = val;
    }

    // Env > file > default for port
    match std::env::var("RESIDUUM_GATEWAY_PORT") {
        Ok(val) => match val.parse::<u16>() {
            Ok(p) => cfg.port = p,
            Err(e) => {
                eprintln!("warning: RESIDUUM_GATEWAY_PORT '{val}' is not a valid port: {e}");
            }
        },
        Err(_) => {
            if let Some(p) = section.and_then(|s| s.port) {
                cfg.port = p;
            }
        }
    }

    cfg
}

/// Resolve Discord configuration from TOML section and environment.
///
/// Token resolution: `RESIDUUM_DISCORD_TOKEN` env > `token` field in TOML (with
/// `${ENV_VAR}` / `secret:name` expansion) > `None` if section is absent or no token found.
fn resolve_discord_config(
    section: Option<&DiscordConfigFile>,
    secrets: &SecretStore,
) -> Option<DiscordConfig> {
    let token = std::env::var("RESIDUUM_DISCORD_TOKEN")
        .ok()
        .or_else(|| {
            section
                .and_then(|s| s.token.as_ref())
                .and_then(|t| resolve_secret_value(t, secrets))
        })
        .filter(|t| !t.is_empty());

    match (section, token) {
        (_, Some(tok)) => Some(DiscordConfig { token: tok }),
        (Some(_), None) => {
            eprintln!(
                "warning: [discord] section present but no token found; \
                 set RESIDUUM_DISCORD_TOKEN or token in config"
            );
            None
        }
        (None, None) => None,
    }
}

/// Resolve Telegram configuration from TOML section and environment.
///
/// Token resolution: `RESIDUUM_TELEGRAM_TOKEN` env > `token` field in TOML (with
/// `${ENV_VAR}` / `secret:name` expansion) > `None` if section is absent or no token found.
fn resolve_telegram_config(
    section: Option<&TelegramConfigFile>,
    secrets: &SecretStore,
) -> Option<TelegramConfig> {
    let token = std::env::var("RESIDUUM_TELEGRAM_TOKEN")
        .ok()
        .or_else(|| {
            section
                .and_then(|s| s.token.as_ref())
                .and_then(|t| resolve_secret_value(t, secrets))
        })
        .filter(|t| !t.is_empty());

    match (section, token) {
        (_, Some(tok)) => Some(TelegramConfig { token: tok }),
        (Some(_), None) => {
            eprintln!(
                "warning: [telegram] section present but no token found; \
                 set RESIDUUM_TELEGRAM_TOKEN or token in config"
            );
            None
        }
        (None, None) => None,
    }
}

/// Expand `${ENV_VAR}` references in a token string.
///
/// Returns `Some(value)` if expansion succeeds or the string contains no `${...}`.
/// Returns `None` if the referenced env var is not set.
fn expand_env_token(raw: &str) -> Option<String> {
    let inner = raw
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
        .filter(|s| !s.is_empty());

    match inner {
        Some(var_name) => std::env::var(var_name).ok(),
        None => Some(raw.to_string()),
    }
}

/// Resolve a secret reference. Supports three modes:
/// - `${ENV_VAR}` → environment variable lookup
/// - `secret:name` → encrypted secrets file lookup
/// - Anything else → literal string passthrough
fn resolve_secret_value(raw: &str, secrets: &SecretStore) -> Option<String> {
    if let Some(name) = raw.strip_prefix("secret:") {
        return secrets.get(name).map(String::from);
    }
    expand_env_token(raw)
}

/// Resolve webhook configuration from TOML section.
fn resolve_webhook_config(
    section: Option<&WebhookConfigFile>,
    secrets: &SecretStore,
) -> WebhookConfig {
    let mut cfg = WebhookConfig::default();
    if let Some(s) = section {
        if let Some(v) = s.enabled {
            cfg.enabled = v;
        }
        cfg.secret = s
            .secret
            .as_deref()
            .and_then(|raw| resolve_secret_value(raw, secrets));
    }
    cfg
}

/// Resolve skills configuration from TOML section.
///
/// Defaults to the workspace `skills/` directory. Additional directories
/// from the config are expanded and appended.
fn resolve_skills_config(section: Option<&SkillsConfigFile>, workspace_dir: &Path) -> SkillsConfig {
    let layout = crate::workspace::layout::WorkspaceLayout::new(workspace_dir);
    let mut dirs = vec![layout.skills_dir()];

    if let Some(extra) = section.and_then(|s| s.dirs.as_ref()) {
        for raw in extra {
            let expanded = shellexpand::tilde(raw);
            dirs.push(PathBuf::from(expanded.as_ref()));
        }
    }

    SkillsConfig { dirs }
}

/// Resolve hybrid search configuration from TOML section with defaults.
fn resolve_search_config(section: Option<&SearchConfigFile>) -> SearchConfig {
    let mut cfg = SearchConfig::default();

    if let Some(s) = section {
        if let Some(v) = s.vector_weight {
            cfg.vector_weight = v;
        }
        if let Some(v) = s.text_weight {
            cfg.text_weight = v;
        }
        if let Some(v) = s.min_score {
            cfg.min_score = v;
        }
        if let Some(v) = s.candidate_multiplier {
            cfg.candidate_multiplier = v;
        }
        if let Some(v) = s.temporal_decay {
            cfg.temporal_decay = v;
        }
        if let Some(v) = s.temporal_decay_half_life_days {
            if v <= 0.0 {
                eprintln!(
                    "warning: [memory.search] temporal_decay_half_life_days must be positive, \
                     got {v}; using default {}",
                    cfg.temporal_decay_half_life_days
                );
            } else {
                cfg.temporal_decay_half_life_days = v;
            }
        }
    }

    let sum = cfg.vector_weight + cfg.text_weight;
    if (sum - 1.0).abs() > 0.01 {
        eprintln!(
            "warning: [memory.search] vector_weight ({}) + text_weight ({}) = {sum:.2}, expected ~1.0",
            cfg.vector_weight, cfg.text_weight
        );
    }

    cfg
}

/// Resolve agent ability gates from TOML section.
fn resolve_agent_config(section: Option<&AgentConfigFile>) -> AgentAbilitiesConfig {
    let mut cfg = AgentAbilitiesConfig::default();
    if let Some(s) = section {
        if let Some(v) = s.modify_mcp {
            cfg.modify_mcp = v;
        }
        if let Some(v) = s.modify_channels {
            cfg.modify_channels = v;
        }
    }
    cfg
}

/// Resolve background task configuration.
///
/// Reads `max_concurrent` and `transcript_retention_days` from `config.toml`'s
/// `[background]` section, and model tiers from `providers.toml`'s
/// `[background.models]` section.
///
/// # Errors
/// Returns `ResiduumError::Config` if a model tier string cannot be resolved.
fn resolve_background_config(
    section: Option<&BackgroundConfigFile>,
    models_section: Option<&BackgroundModelsFile>,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
) -> Result<BackgroundConfig, ResiduumError> {
    let mut cfg = BackgroundConfig::default();

    if let Some(section) = section {
        if let Some(v) = section.max_concurrent {
            cfg.max_concurrent = v;
        }
        if let Some(v) = section.transcript_retention_days {
            cfg.transcript_retention_days = v;
        }
    }

    if let Some(models_section) = models_section {
        cfg.models.small = models_section
            .small
            .clone()
            .map(|spec| resolve_model_chain(spec, providers_map, secrets))
            .transpose()?;
        cfg.models.medium = models_section
            .medium
            .clone()
            .map(|spec| resolve_model_chain(spec, providers_map, secrets))
            .transpose()?;
        cfg.models.large = models_section
            .large
            .clone()
            .map(|spec| resolve_model_chain(spec, providers_map, secrets))
            .transpose()?;
    }

    Ok(cfg)
}

/// Warn on deprecated environment variables that no longer have effect.
fn warn_deprecated_env_vars() {
    let deprecated = [
        "RESIDUUM_OBSERVER_MODEL",
        "RESIDUUM_REFLECTOR_MODEL",
        "RESIDUUM_OBSERVER_API_KEY",
        "RESIDUUM_REFLECTOR_API_KEY",
    ];

    for var in &deprecated {
        if std::env::var(var).is_ok() {
            eprintln!(
                "warning: {var} is deprecated and has no effect; \
                 use [models] observer/reflector in config.toml instead"
            );
        }
    }
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
    use super::super::constants::{
        DEFAULT_ANTHROPIC_URL, DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD,
        DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    };
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
    fn deny_unknown_fields_rejects_top_level_typos() {
        let toml_str = r#"
timezone = "UTC"

[memori]
observer_threshold_tokens = 30000
"#;
        let result = toml::from_str::<ConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown top-level section should be rejected"
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

    #[test]
    fn memory_config_just_thresholds() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[memory]
observer_threshold_tokens = 20000
reflector_threshold_tokens = 50000
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(cfg.memory.observer_threshold_tokens, 20000);
        assert_eq!(cfg.memory.reflector_threshold_tokens, 50000);
    }

    #[test]
    fn memory_config_defaults_when_absent() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.memory.observer_threshold_tokens,
            DEFAULT_OBSERVER_THRESHOLD
        );
        assert_eq!(
            cfg.memory.reflector_threshold_tokens,
            DEFAULT_REFLECTOR_THRESHOLD
        );
    }

    #[test]
    fn config_no_timezone_errors() {
        let result = from_file_and_env(None, None, &test_config_dir());
        assert!(result.is_err(), "missing timezone should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timezone"),
            "error should mention timezone: {err}"
        );
    }

    #[test]
    fn config_with_timezone() {
        let cfg_file = parse_config("timezone = \"America/New_York\"\n");
        let prov_file = parse_providers("[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n");
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.timezone.name(),
            "America/New_York",
            "timezone should be parsed"
        );
    }

    #[test]
    fn config_invalid_timezone_errors() {
        let cfg_file = parse_config("timezone = \"Not/A/Timezone\"\n");
        let prov_file = parse_providers("[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n");
        let result = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir());
        assert!(result.is_err(), "invalid timezone should error");
    }

    #[test]
    fn pulse_enabled_defaults() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(cfg.pulse_enabled, "pulse should default to enabled");
    }

    #[test]
    fn discord_absent_returns_none() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.discord.is_none(),
            "no [discord] section should yield None"
        );
    }

    #[test]
    fn discord_section_without_token_returns_none() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[discord]
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.discord.is_none(),
            "[discord] with no token should yield None"
        );
    }

    #[test]
    fn discord_section_with_token() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[discord]
token = "my-bot-token"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(cfg.discord.is_some(), "[discord] with token should be Some");
        assert_eq!(
            cfg.discord.as_ref().map(|d| d.token.as_str()),
            Some("my-bot-token"),
            "token should match"
        );
    }

    #[test]
    fn webhook_defaults_when_absent() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(!cfg.webhook.enabled, "webhook should default to disabled");
        assert!(
            cfg.webhook.secret.is_none(),
            "webhook secret should default to None"
        );
    }

    #[test]
    fn webhook_enabled_with_secret() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[webhook]
enabled = true
secret = "my-secret"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(cfg.webhook.enabled, "webhook should be enabled");
        assert_eq!(
            cfg.webhook.secret.as_deref(),
            Some("my-secret"),
            "webhook secret should match"
        );
    }

    #[test]
    fn memory_config_cooldown_defaults() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.memory.observer_cooldown_secs, DEFAULT_OBSERVER_COOLDOWN_SECS,
            "cooldown should default"
        );
        assert_eq!(
            cfg.memory.observer_force_threshold_tokens, DEFAULT_OBSERVER_FORCE_THRESHOLD,
            "force threshold should default"
        );
    }

    #[test]
    fn memory_config_cooldown_custom() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[memory]
observer_cooldown_secs = 60
observer_force_threshold_tokens = 50000
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert_eq!(
            cfg.memory.observer_cooldown_secs, 60,
            "cooldown should be custom"
        );
        assert_eq!(
            cfg.memory.observer_force_threshold_tokens, 50000,
            "force threshold should be custom"
        );
    }

    #[test]
    fn pulse_can_be_disabled() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[pulse]
enabled = false
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(!cfg.pulse_enabled);
    }

    // ── Agent abilities ──────────────────────────────────────────────────────

    #[test]
    fn agent_abilities_default_to_true() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(cfg.agent.modify_mcp, "modify_mcp should default to true");
        assert!(
            cfg.agent.modify_channels,
            "modify_channels should default to true"
        );
    }

    #[test]
    fn agent_abilities_custom_values() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[agent]
modify_mcp = false
modify_channels = false
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(!cfg.agent.modify_mcp, "modify_mcp should be false");
        assert!(
            !cfg.agent.modify_channels,
            "modify_channels should be false"
        );
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

    // ── Search config ─────────────────────────────────────────────────────

    #[test]
    fn search_config_defaults_when_absent() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let search = &cfg.memory.search;
        assert!(
            (search.vector_weight - 0.7).abs() < f64::EPSILON,
            "vector_weight should default to 0.7"
        );
        assert!(
            (search.text_weight - 0.3).abs() < f64::EPSILON,
            "text_weight should default to 0.3"
        );
        assert!(
            (search.min_score - 0.35).abs() < f64::EPSILON,
            "min_score should default to 0.35"
        );
        assert_eq!(
            search.candidate_multiplier, 4,
            "candidate_multiplier should default to 4"
        );
    }

    #[test]
    fn search_config_custom_values() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[memory.search]
vector_weight = 0.5
text_weight = 0.5
min_score = 0.2
candidate_multiplier = 8
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let search = &cfg.memory.search;
        assert!(
            (search.vector_weight - 0.5).abs() < f64::EPSILON,
            "vector_weight should be custom"
        );
        assert!(
            (search.text_weight - 0.5).abs() < f64::EPSILON,
            "text_weight should be custom"
        );
        assert!(
            (search.min_score - 0.2).abs() < f64::EPSILON,
            "min_score should be custom"
        );
        assert_eq!(
            search.candidate_multiplier, 8,
            "candidate_multiplier should be custom"
        );
    }

    #[test]
    fn search_config_deny_unknown_fields() {
        let toml_str = r#"
timezone = "UTC"

[memory.search]
typo_field = 0.5
"#;
        let result = toml::from_str::<ConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [memory.search] should be rejected"
        );
    }

    // ── Secret / env expansion ──────────────────────────────────────────────

    #[test]
    fn expand_env_token_literal() {
        assert_eq!(
            expand_env_token("plain-string"),
            Some("plain-string".to_string()),
            "literal should pass through"
        );
    }

    #[test]
    fn expand_env_token_present() {
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("RESIDUUM_TEST_SECRET_PRESENT", "found-it") };
        let result = expand_env_token("${RESIDUUM_TEST_SECRET_PRESENT}");
        assert_eq!(
            result,
            Some("found-it".to_string()),
            "should resolve env var"
        );
        unsafe { std::env::remove_var("RESIDUUM_TEST_SECRET_PRESENT") };
    }

    #[test]
    fn expand_env_token_missing() {
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::remove_var("RESIDUUM_TEST_SECRET_MISSING") };
        let result = expand_env_token("${RESIDUUM_TEST_SECRET_MISSING}");
        assert!(result.is_none(), "missing env var should return None");
    }

    #[test]
    fn resolve_secret_value_env() {
        let secrets = empty_secrets();
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("RESIDUUM_TEST_RSV_ENV", "env-val") };
        let result = resolve_secret_value("${RESIDUUM_TEST_RSV_ENV}", &secrets);
        assert_eq!(
            result,
            Some("env-val".to_string()),
            "should dispatch to env expansion"
        );
        unsafe { std::env::remove_var("RESIDUUM_TEST_RSV_ENV") };
    }

    #[test]
    fn resolve_secret_value_secret_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();
        store.set("test_key", "secret-val", dir.path()).unwrap();

        let result = resolve_secret_value("secret:test_key", &store);
        assert_eq!(
            result,
            Some("secret-val".to_string()),
            "should dispatch to secret store"
        );
    }

    #[test]
    fn resolve_secret_value_literal() {
        let secrets = empty_secrets();
        let result = resolve_secret_value("plain-api-key", &secrets);
        assert_eq!(
            result,
            Some("plain-api-key".to_string()),
            "literal should pass through"
        );
    }

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

    // ── Gateway config ──────────────────────────────────────────────────────

    #[test]
    fn gateway_config_defaults() {
        // Clear env vars that gateway_config_env_override may have set
        // (tests run in parallel within the same process).
        unsafe {
            std::env::remove_var("RESIDUUM_GATEWAY_BIND");
            std::env::remove_var("RESIDUUM_GATEWAY_PORT");
        }
        let cfg = resolve_gateway_config(None);
        assert_eq!(cfg.bind, "127.0.0.1", "default bind should be loopback");
        assert_eq!(cfg.port, 7700, "default port should be 7700");
        assert_eq!(cfg.addr(), "127.0.0.1:7700");
    }

    #[test]
    fn gateway_config_env_override() {
        // SAFETY: test-only, single-threaded test environment
        unsafe {
            std::env::set_var("RESIDUUM_GATEWAY_BIND", "0.0.0.0");
            std::env::set_var("RESIDUUM_GATEWAY_PORT", "8080");
        }
        let cfg = resolve_gateway_config(None);
        assert_eq!(cfg.bind, "0.0.0.0", "env should override bind");
        assert_eq!(cfg.port, 8080, "env should override port");
        assert_eq!(cfg.addr(), "0.0.0.0:8080");
        unsafe {
            std::env::remove_var("RESIDUUM_GATEWAY_BIND");
            std::env::remove_var("RESIDUUM_GATEWAY_PORT");
        }
    }

    #[test]
    fn telegram_absent_returns_none() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.telegram.is_none(),
            "no [telegram] section should yield None"
        );
    }

    #[test]
    fn telegram_section_without_token_returns_none() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[telegram]
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.telegram.is_none(),
            "[telegram] with no token should yield None"
        );
    }

    #[test]
    fn telegram_section_with_token() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[telegram]
token = "123456789:ABCdefGHIjklmnop"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.telegram.is_some(),
            "[telegram] with token should be Some"
        );
        assert_eq!(
            cfg.telegram.as_ref().map(|t| t.token.as_str()),
            Some("123456789:ABCdefGHIjklmnop"),
            "token should match"
        );
    }
}

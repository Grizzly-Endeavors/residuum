//! Config resolution logic: maps raw TOML structs + env vars into validated Config.

mod models;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::ResiduumError;
use crate::models::retry::RetryConfig;
use crate::models::{ThinkingConfig, ThinkingLevel};

use super::Config;
use super::bootstrap::default_workspace_dir;
use super::constants::{DEFAULT_IDLE_TIMEOUT_MINUTES, DEFAULT_MAX_TOKENS, DEFAULT_TIMEOUT_SECS};
use super::deserialize::{
    AgentConfigFile, BackgroundConfigFile, BackgroundModelsFile, CloudConfigFile, ConfigFile,
    DiscordConfigFile, GatewayConfigFile, MemoryConfigFile, ProviderEntryFile, ProvidersFile,
    SearchConfigFile, SkillsConfigFile, TelegramConfigFile, WebSearchConfigFile, WebhookConfigFile,
};
use super::provider::ProviderKind;
use super::secrets::SecretStore;
use super::types::{
    AgentAbilitiesConfig, BackgroundConfig, CloudConfig, DiscordConfig, GatewayConfig, IdleConfig,
    MemoryConfig, ProviderNativeSearchConfig, SearchConfig, SkillsConfig, StandaloneBackendConfig,
    TelegramConfig, WebSearchConfig, WebhookConfig,
};

/// Build a `Config` from an optional config file and environment variables.
///
/// # Errors
/// Returns `ResiduumError::Config` if the model spec cannot be parsed or
/// the workspace directory cannot be determined.
pub(crate) fn from_file_and_env(
    file: Option<&ConfigFile>,
    providers_file: Option<&ProvidersFile>,
    config_dir: &Path,
) -> Result<Config, ResiduumError> {
    warn_deprecated_env_vars();

    let secrets = SecretStore::load(config_dir)?;
    let providers_map = providers_file.and_then(|f| f.providers.as_ref());
    let models_section = providers_file.and_then(|f| f.models.as_ref());

    let mut resolved_models =
        models::resolve_all_model_specs(models_section, providers_map, &secrets)?;

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

    let memory = resolve_memory_config(file.and_then(|f| f.memory.as_ref()));

    let pulse_enabled = file
        .and_then(|f| f.pulse.as_ref())
        .and_then(|p| p.enabled)
        .unwrap_or(true);

    let gateway = resolve_gateway_config(file.and_then(|f| f.gateway.as_ref()));
    let cloud = resolve_cloud_config(file.and_then(|f| f.cloud.as_ref()), &secrets, &gateway);
    let discord = resolve_discord_config(file.and_then(|f| f.discord.as_ref()), &secrets);
    let telegram = resolve_telegram_config(file.and_then(|f| f.telegram.as_ref()), &secrets);
    let webhook = resolve_webhook_config(file.and_then(|f| f.webhook.as_ref()), &secrets);
    let skills = resolve_skills_config(file.and_then(|f| f.skills.as_ref()), &workspace_dir);

    let agent = resolve_agent_config(file.and_then(|f| f.agent.as_ref()));

    let idle = resolve_idle_config(file, telegram.as_ref(), discord.as_ref())?;

    let background = resolve_background_config(
        file.and_then(|f| f.background.as_ref()),
        providers_file
            .and_then(|pf| pf.background.as_ref())
            .and_then(|b| b.models.as_ref()),
        providers_map,
        &secrets,
        &mut resolved_models.role_overrides,
    )?;

    let retry = resolve_retry_config(file);

    let timezone = resolve_timezone(file)?;

    let name = file.and_then(|f| f.name.clone());

    let thinking = file
        .and_then(|f| f.thinking.as_deref())
        .map(parse_thinking_config)
        .transpose()?;

    let web_search = resolve_web_search_config(
        file.and_then(|f| f.web_search.as_ref()),
        &resolved_models.main,
        &secrets,
    );

    Ok(Config {
        name,
        main: resolved_models.main,
        observer: resolved_models.observer,
        reflector: resolved_models.reflector,
        pulse: resolved_models.pulse,
        embedding: resolved_models.embedding,
        workspace_dir,
        timeout_secs,
        max_tokens,
        memory,
        pulse_enabled,
        gateway,
        timezone,
        cloud,
        discord,
        telegram,
        webhook,
        skills,
        retry,
        background,
        agent,
        idle,
        temperature: file.and_then(|f| f.temperature),
        thinking,
        web_search,
        role_overrides: resolved_models.role_overrides,
        config_dir: PathBuf::new(),
    })
}

/// Resolve the timezone from env var or config file.
///
/// # Errors
/// Returns `ResiduumError::Config` if no timezone is set or the value is not a
/// valid IANA timezone name.
fn resolve_timezone(file: Option<&ConfigFile>) -> Result<chrono_tz::Tz, ResiduumError> {
    let tz_name = std::env::var("RESIDUUM_TIMEZONE")
        .ok()
        .or_else(|| file.and_then(|f| f.timezone.clone()))
        .ok_or_else(|| {
            ResiduumError::Config(
                "timezone is required: set RESIDUUM_TIMEZONE env var or 'timezone' in config.toml \
                 (IANA name, e.g. \"America/New_York\")"
                    .to_string(),
            )
        })?;
    tz_name.parse().map_err(|_err| {
        ResiduumError::Config(format!(
            "invalid timezone '{tz_name}': expected IANA name like 'America/New_York' or 'UTC'"
        ))
    })
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

/// Default relay WebSocket URL.
const DEFAULT_CLOUD_RELAY_URL: &str = "wss://agent-residuum.com/tunnel/register";

/// Resolve cloud tunnel configuration from TOML section and environment.
///
/// Token resolution: `RESIDUUM_CLOUD_TOKEN` env > `token` field in TOML (with
/// `${ENV_VAR}` / `secret:name` expansion) > `None` if section is absent, disabled,
/// or no token found.
fn resolve_cloud_config(
    section: Option<&CloudConfigFile>,
    secrets: &SecretStore,
    gateway: &GatewayConfig,
) -> Option<CloudConfig> {
    let section = section?;

    // If explicitly disabled, return None.
    if section.enabled == Some(false) {
        return None;
    }

    let token = std::env::var("RESIDUUM_CLOUD_TOKEN")
        .ok()
        .or_else(|| {
            section
                .token
                .as_ref()
                .and_then(|t| resolve_secret_value(t, secrets))
        })
        .filter(|t| !t.is_empty());

    if let Some(tok) = token {
        let relay_url = section
            .relay_url
            .clone()
            .unwrap_or_else(|| DEFAULT_CLOUD_RELAY_URL.to_string());
        let local_port = section.local_port.unwrap_or(gateway.port);
        Some(CloudConfig {
            relay_url,
            token: tok,
            local_port,
        })
    } else {
        // Section present with enabled=true but no token.
        if section.enabled == Some(true) {
            eprintln!(
                "warning: [cloud] section enabled but no token found; \
                 set RESIDUUM_CLOUD_TOKEN or token in config"
            );
        }
        None
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
pub(super) fn resolve_secret_value(raw: &str, secrets: &SecretStore) -> Option<String> {
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

/// Resolve memory subsystem configuration from TOML section with defaults.
fn resolve_memory_config(section: Option<&MemoryConfigFile>) -> MemoryConfig {
    let mut mem = MemoryConfig::default();
    if let Some(s) = section {
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
    mem.search = resolve_search_config(section.and_then(|m| m.search.as_ref()));
    mem
}

/// Resolve idle configuration from TOML section, validating the idle channel
/// against configured interfaces.
///
/// # Errors
/// Returns `ResiduumError::Config` if the idle channel references an unknown
/// or unconfigured interface.
fn resolve_idle_config(
    file: Option<&ConfigFile>,
    telegram: Option<&TelegramConfig>,
    discord: Option<&DiscordConfig>,
) -> Result<IdleConfig, ResiduumError> {
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

    Ok(IdleConfig {
        timeout: std::time::Duration::from_secs(timeout_minutes * 60),
        idle_channel,
    })
}

/// Resolve retry configuration from TOML section with defaults.
fn resolve_retry_config(file: Option<&ConfigFile>) -> RetryConfig {
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

/// Resolve web search configuration from TOML section.
///
/// Provider-native search is enabled automatically when the main provider supports it
/// (Anthropic, `OpenAI`, Gemini). Standalone backends are resolved from the `backend` field.
fn resolve_web_search_config(
    section: Option<&WebSearchConfigFile>,
    main_chain: &[super::provider::ProviderSpec],
    secrets: &SecretStore,
) -> WebSearchConfig {
    let mut cfg = WebSearchConfig::default();

    // Determine the main provider kind for native search detection
    let main_kind = main_chain.first().map(|p| p.model.kind);

    // Provider-native search: auto-enable for Anthropic, OpenAI, Gemini
    let has_native = matches!(
        main_kind,
        Some(ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::Gemini)
    );

    if has_native {
        let mut native = ProviderNativeSearchConfig::default();

        if let Some(s) = section {
            // Apply Anthropic overrides
            if let Some(ref a) = s.anthropic {
                native.max_uses = a.max_uses;
                native.allowed_domains.clone_from(&a.allowed_domains);
                native.blocked_domains.clone_from(&a.blocked_domains);
            }
            // Apply OpenAI overrides
            if let Some(ref o) = s.openai {
                native
                    .search_context_size
                    .clone_from(&o.search_context_size);
            }
            // Apply Gemini overrides
            if let Some(ref g) = s.gemini {
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
            "brave" => s.brave.as_ref().and_then(|b| {
                let api_key = b
                    .api_key
                    .as_deref()
                    .and_then(|k| resolve_secret_value(k, secrets))
                    .or_else(|| std::env::var("BRAVE_API_KEY").ok());
                api_key.map(|key| StandaloneBackendConfig {
                    name: "brave".to_string(),
                    api_key: key,
                    base_url: None,
                })
            }),
            "tavily" => s.tavily.as_ref().and_then(|t| {
                let api_key = t
                    .api_key
                    .as_deref()
                    .and_then(|k| resolve_secret_value(k, secrets))
                    .or_else(|| std::env::var("TAVILY_API_KEY").ok());
                api_key.map(|key| StandaloneBackendConfig {
                    name: "tavily".to_string(),
                    api_key: key,
                    base_url: None,
                })
            }),
            "ollama" => s.ollama.as_ref().and_then(|o| {
                let api_key = o
                    .api_key
                    .as_deref()
                    .and_then(|k| resolve_secret_value(k, secrets))
                    .or_else(|| std::env::var("OLLAMA_API_KEY").ok());
                api_key.map(|key| StandaloneBackendConfig {
                    name: "ollama".to_string(),
                    api_key: key,
                    base_url: o.base_url.clone(),
                })
            }),
            other => {
                eprintln!(
                    "warning: [web_search] unknown backend \"{other}\"; \
                         expected brave, tavily, or ollama"
                );
                None
            }
        };

        if resolved.is_none() && !backend_name.is_empty() {
            eprintln!(
                "warning: [web_search] backend \"{backend_name}\" configured but \
                     no API key found; set api_key in [web_search.{backend_name}] or the \
                     corresponding env var"
            );
        }

        cfg.standalone_backend = resolved;
    }

    cfg
}

/// Parse a thinking config string into a `ThinkingConfig`.
fn parse_thinking_config(value: &str) -> Result<ThinkingConfig, ResiduumError> {
    match value.to_lowercase().as_str() {
        "off" | "false" => Ok(ThinkingConfig::Toggle(false)),
        "on" | "true" => Ok(ThinkingConfig::Toggle(true)),
        "low" => Ok(ThinkingConfig::Level(ThinkingLevel::Low)),
        "medium" => Ok(ThinkingConfig::Level(ThinkingLevel::Medium)),
        "high" => Ok(ThinkingConfig::Level(ThinkingLevel::High)),
        other => Err(ResiduumError::Config(format!(
            "invalid thinking value '{other}': expected one of: off, on, low, medium, high"
        ))),
    }
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
    role_overrides: &mut HashMap<String, super::types::RoleOverrides>,
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
        cfg.models.small = resolve_bg_tier(
            models_section.small.clone(),
            "bg_small",
            providers_map,
            secrets,
            role_overrides,
        )?;
        cfg.models.medium = resolve_bg_tier(
            models_section.medium.clone(),
            "bg_medium",
            providers_map,
            secrets,
            role_overrides,
        )?;
        cfg.models.large = resolve_bg_tier(
            models_section.large.clone(),
            "bg_large",
            providers_map,
            secrets,
            role_overrides,
        )?;
    }

    Ok(cfg)
}

/// Resolve a single background tier assignment, extracting overrides.
fn resolve_bg_tier(
    assignment: Option<super::deserialize::ModelAssignment>,
    role_key: &str,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
    secrets: &SecretStore,
    role_overrides: &mut HashMap<String, super::types::RoleOverrides>,
) -> Result<Option<Vec<super::provider::ProviderSpec>>, ResiduumError> {
    let Some(spec) = assignment else {
        return Ok(None);
    };
    models::extract_role_overrides_pub(role_key, &spec, role_overrides)?;
    Ok(Some(models::resolve_assignment_chain(
        spec,
        providers_map,
        secrets,
    )?))
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    unsafe_code,
    reason = "std::env::set_var/remove_var require unsafe in edition 2024"
)]
mod tests {
    use super::super::constants::{
        DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD,
        DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    };
    use super::super::deserialize::{ConfigFile, ProvidersFile};
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

    // ── Section-specific resolution ───────────────────────────────────────────

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

    // ── Gateway config ──────────────────────────────────────────────────────

    #[test]
    fn gateway_config_defaults_and_env_override() {
        // Combined into one test to avoid env var races across parallel tests.
        // SAFETY: test-only environment
        unsafe {
            std::env::remove_var("RESIDUUM_GATEWAY_BIND");
            std::env::remove_var("RESIDUUM_GATEWAY_PORT");
        }

        // Defaults
        let cfg = resolve_gateway_config(None);
        assert_eq!(cfg.bind, "127.0.0.1", "default bind should be loopback");
        assert_eq!(cfg.port, 7700, "default port should be 7700");
        assert_eq!(cfg.addr(), "127.0.0.1:7700");

        // Env overrides
        unsafe {
            std::env::set_var("RESIDUUM_GATEWAY_BIND", "0.0.0.0");
            std::env::set_var("RESIDUUM_GATEWAY_PORT", "8080");
        }
        let env_cfg = resolve_gateway_config(None);
        assert_eq!(env_cfg.bind, "0.0.0.0", "env should override bind");
        assert_eq!(env_cfg.port, 8080, "env should override port");
        assert_eq!(env_cfg.addr(), "0.0.0.0:8080");
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

    // ── Cloud config ───────────────────────────────────────────────────────

    #[test]
    fn cloud_absent_returns_none() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(cfg.cloud.is_none(), "no [cloud] section should yield None");
    }

    #[test]
    fn cloud_disabled_returns_none() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[cloud]
enabled = false
token = "rst_test"
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
            cfg.cloud.is_none(),
            "[cloud] with enabled=false should yield None"
        );
    }

    #[test]
    fn cloud_with_token_and_defaults() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[cloud]
enabled = true
token = "rst_testtoken12345678901234567890"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let cloud = cfg.cloud.as_ref();
        assert!(cloud.is_some(), "[cloud] with token should be Some");
        let cloud = cloud.unwrap();
        assert_eq!(
            cloud.token, "rst_testtoken12345678901234567890",
            "token should match"
        );
        assert_eq!(
            cloud.relay_url, "wss://agent-residuum.com/tunnel/register",
            "relay_url should default"
        );
        assert_eq!(
            cloud.local_port, 7700,
            "local_port should default to gateway port"
        );
    }

    #[test]
    fn cloud_custom_relay_url_and_port() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[gateway]
port = 9000

[cloud]
enabled = true
token = "rst_testtoken12345678901234567890"
relay_url = "ws://localhost:8080/tunnel/register"
local_port = 3000
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let cloud = cfg.cloud.as_ref().unwrap();
        assert_eq!(
            cloud.relay_url, "ws://localhost:8080/tunnel/register",
            "custom relay_url"
        );
        assert_eq!(cloud.local_port, 3000, "custom local_port");
    }

    #[test]
    fn cloud_local_port_defaults_to_gateway_port() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[gateway]
port = 9000

[cloud]
enabled = true
token = "rst_testtoken12345678901234567890"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let cloud = cfg.cloud.as_ref().unwrap();
        assert_eq!(
            cloud.local_port, 9000,
            "local_port should default to gateway port"
        );
    }

    // ── Web search config ────────────────────────────────────────────────

    #[test]
    fn web_search_native_enabled_for_anthropic() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "anthropic should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_enabled_for_openai() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "openai/gpt-4o"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "openai should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_enabled_for_gemini() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "gemini/gemini-2.0-flash"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "gemini should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_disabled_for_ollama() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "ollama/llama3"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.provider_native.is_none(),
            "ollama should not get provider-native search"
        );
    }

    #[test]
    fn web_search_anthropic_overrides() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[web_search.anthropic]
max_uses = 3
allowed_domains = ["example.com"]
blocked_domains = ["spam.com"]
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert_eq!(native.max_uses, Some(3), "max_uses should be set");
        assert_eq!(
            native.allowed_domains.as_deref(),
            Some(&["example.com".to_string()][..]),
            "allowed_domains should be set"
        );
        assert_eq!(
            native.blocked_domains.as_deref(),
            Some(&["spam.com".to_string()][..]),
            "blocked_domains should be set"
        );
    }

    #[test]
    fn web_search_openai_overrides() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[web_search.openai]
search_context_size = "high"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "openai/gpt-4o"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert_eq!(
            native.search_context_size.as_deref(),
            Some("high"),
            "search_context_size should be set"
        );
    }

    #[test]
    fn web_search_standalone_brave_with_literal_key() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[web_search]
backend = "brave"

[web_search.brave]
api_key = "BSA-test-key"
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "ollama/llama3"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        let backend = cfg.web_search.standalone_backend.as_ref().unwrap();
        assert_eq!(backend.name, "brave", "backend name should be brave");
        assert_eq!(
            backend.api_key, "BSA-test-key",
            "api key should be resolved"
        );
    }

    #[test]
    fn web_search_standalone_no_key_warns() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[web_search]
backend = "brave"

[web_search.brave]
"#,
        );
        let prov_file = parse_providers(
            r#"
[models]
main = "ollama/llama3"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.standalone_backend.is_none(),
            "backend without api key should be None"
        );
    }

    #[test]
    fn web_search_both_native_and_standalone_coexist() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[web_search]
backend = "brave"

[web_search.brave]
api_key = "BSA-test"

[web_search.anthropic]
max_uses = 5
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
            cfg.web_search.provider_native.is_some(),
            "provider-native should be set"
        );
        assert!(
            cfg.web_search.standalone_backend.is_some(),
            "standalone backend should also be set"
        );
    }

    #[test]
    fn web_search_deny_unknown_fields() {
        let toml_str = r#"
timezone = "UTC"

[web_search]
typo = "bad"
"#;
        let result = toml::from_str::<ConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [web_search] should be rejected"
        );
    }

    #[test]
    fn web_search_defaults_when_absent() {
        let cfg_file = parse_config("timezone = \"UTC\"\n");
        let prov_file = parse_providers(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        );
        let cfg = from_file_and_env(Some(&cfg_file), Some(&prov_file), &test_config_dir()).unwrap();
        assert!(
            cfg.web_search.standalone_backend.is_none(),
            "standalone should be None by default"
        );
        // provider_native is auto-set for anthropic
        assert!(
            cfg.web_search.provider_native.is_some(),
            "native should be auto-set for anthropic"
        );
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert!(native.max_uses.is_none(), "max_uses should default to None");
    }

    #[test]
    fn cloud_section_no_token_returns_none() {
        let cfg_file = parse_config(
            r#"
timezone = "UTC"

[cloud]
enabled = true
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
            cfg.cloud.is_none(),
            "[cloud] enabled=true but no token should yield None"
        );
    }
}

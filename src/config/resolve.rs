//! Config resolution logic: maps raw TOML structs + env vars into validated Config.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::IronclawError;

use super::Config;
use super::bootstrap::default_workspace_dir;
use super::constants::{
    DEFAULT_GATEWAY_BIND, DEFAULT_GATEWAY_PORT, DEFAULT_MAX_TOKENS, DEFAULT_OBSERVER_COOLDOWN_SECS,
    DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    DEFAULT_TIMEOUT_SECS,
};
use super::deserialize::{
    ConfigFile, DiscordConfigFile, GatewayConfigFile, ProviderEntryFile, SkillsConfigFile,
    WebhookConfigFile,
};
use super::provider::{ModelSpec, ProviderKind, ProviderSpec};
use super::types::{DiscordConfig, GatewayConfig, MemoryConfig, SkillsConfig, WebhookConfig};

/// Build a `Config` from an optional config file and environment variables.
///
/// # Errors
/// Returns `IronclawError::Config` if the model spec cannot be parsed or
/// the workspace directory cannot be determined.
#[expect(
    clippy::too_many_lines,
    reason = "config resolution is a single sequential pipeline; splitting would obscure the precedence chain"
)]
pub(super) fn from_file_and_env(file: Option<&ConfigFile>) -> Result<Config, IronclawError> {
    warn_deprecated_env_vars();

    let providers_map = file.and_then(|f| f.providers.as_ref());
    let models = file.and_then(|f| f.models.as_ref());

    // Resolve main: IRONCLAW_MODEL env > models.main > default
    let main_model_str = std::env::var("IRONCLAW_MODEL")
        .ok()
        .or_else(|| models.and_then(|m| m.main.clone()))
        .unwrap_or_else(|| "anthropic/claude-sonnet-4-6".to_string());

    let mut main = resolve_model_string(&main_model_str, providers_map)?;

    // IRONCLAW_PROVIDER_URL overrides main provider URL only
    if let Ok(url) = std::env::var("IRONCLAW_PROVIDER_URL") {
        main.provider_url = url;
    }

    // Resolve each role: models.<role> > models.default > main
    let default_str = models.and_then(|m| m.default.clone());

    let observer = resolve_role(
        models.and_then(|m| m.observer.as_deref()),
        default_str.as_deref(),
        &main,
        providers_map,
    )?;
    let reflector = resolve_role(
        models.and_then(|m| m.reflector.as_deref()),
        default_str.as_deref(),
        &main,
        providers_map,
    )?;
    let pulse_spec = resolve_role(
        models.and_then(|m| m.pulse.as_deref()),
        default_str.as_deref(),
        &main,
        providers_map,
    )?;
    let cron_spec = resolve_role(
        models.and_then(|m| m.cron.as_deref()),
        default_str.as_deref(),
        &main,
        providers_map,
    )?;

    // Workspace dir: env > file > default
    let workspace_dir = std::env::var("IRONCLAW_WORKSPACE")
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

    let mem_section = file.and_then(|f| f.memory.as_ref());
    let memory = MemoryConfig {
        observer_threshold_tokens: mem_section
            .and_then(|m| m.observer_threshold_tokens)
            .unwrap_or(DEFAULT_OBSERVER_THRESHOLD),
        reflector_threshold_tokens: mem_section
            .and_then(|m| m.reflector_threshold_tokens)
            .unwrap_or(DEFAULT_REFLECTOR_THRESHOLD),
        observer_cooldown_secs: mem_section
            .and_then(|m| m.observer_cooldown_secs)
            .unwrap_or(DEFAULT_OBSERVER_COOLDOWN_SECS),
        observer_force_threshold_tokens: mem_section
            .and_then(|m| m.observer_force_threshold_tokens)
            .unwrap_or(DEFAULT_OBSERVER_FORCE_THRESHOLD),
    };

    let pulse_enabled = file
        .and_then(|f| f.pulse.as_ref())
        .and_then(|p| p.enabled)
        .unwrap_or(true);

    let cron_enabled = file
        .and_then(|f| f.cron.as_ref())
        .and_then(|c| c.enabled)
        .unwrap_or(true);

    let gateway = resolve_gateway_config(file.and_then(|f| f.gateway.as_ref()));
    let discord = resolve_discord_config(file.and_then(|f| f.discord.as_ref()));
    let webhook = resolve_webhook_config(file.and_then(|f| f.webhook.as_ref()));
    let skills = resolve_skills_config(file.and_then(|f| f.skills.as_ref()), &workspace_dir);

    let timezone_str = std::env::var("IRONCLAW_TIMEZONE")
        .ok()
        .or_else(|| file.and_then(|f| f.timezone.clone()));
    let tz_name = timezone_str.ok_or_else(|| {
        IronclawError::Config(
            "timezone is required: set IRONCLAW_TIMEZONE env var or 'timezone' in config.toml \
             (IANA name, e.g. \"America/New_York\")"
                .to_string(),
        )
    })?;
    let timezone: chrono_tz::Tz = tz_name.parse().map_err(|_err| {
        IronclawError::Config(format!(
            "invalid timezone '{tz_name}': expected IANA name like 'America/New_York' or 'UTC'"
        ))
    })?;

    Ok(Config {
        main,
        observer,
        reflector,
        pulse: pulse_spec,
        cron: cron_spec,
        workspace_dir,
        timeout_secs,
        max_tokens,
        memory,
        pulse_enabled,
        cron_enabled,
        gateway,
        timezone,
        discord,
        webhook,
        skills,
    })
}

/// Resolve a `"provider_or_name/model"` string into a `ProviderSpec`.
///
/// Splits on the first `/`:
/// - If `provider_part` matches a key in `providers_map`, that entry's `type`,
///   `url`, and `api_key` are used.
/// - Otherwise `provider_part` is treated as an implicit `ProviderKind` name
///   (e.g. `"anthropic"`). API key falls back to provider-specific env var,
///   then `IRONCLAW_API_KEY`.
///
/// # Errors
/// Returns `IronclawError::Config` if the model string format is invalid,
/// the provider is unknown, or an explicit provider entry references an
/// unknown type.
fn resolve_model_string(
    model_str: &str,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
) -> Result<ProviderSpec, IronclawError> {
    let (provider_part, model_name) = model_str.split_once('/').ok_or_else(|| {
        IronclawError::Config(format!(
            "expected 'provider/model' format, got '{model_str}'"
        ))
    })?;

    if model_name.is_empty() {
        return Err(IronclawError::Config(
            "model name cannot be empty".to_string(),
        ));
    }

    // Check if provider_part matches a named [providers] entry
    if let Some(entry) = providers_map.and_then(|p| p.get(provider_part)) {
        let kind = ProviderKind::from_str(&entry.kind).map_err(|e| {
            IronclawError::Config(format!(
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
            .clone()
            .or_else(|| provider_api_key_env(kind))
            .or_else(|| std::env::var("IRONCLAW_API_KEY").ok());

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
        IronclawError::Config(format!(
            "'{provider_part}' is not a known provider name or type \
             (expected one of: anthropic, gemini, ollama, openai, \
             or a key from [providers])"
        ))
    })?;

    let provider_url = kind.default_url().to_string();

    let api_key = provider_api_key_env(kind).or_else(|| std::env::var("IRONCLAW_API_KEY").ok());

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

/// Resolve a role's provider: explicit role string > default string > clone of main.
///
/// # Errors
/// Returns `IronclawError::Config` if the model string cannot be resolved.
fn resolve_role(
    role_str: Option<&str>,
    default_str: Option<&str>,
    main: &ProviderSpec,
    providers_map: Option<&HashMap<String, ProviderEntryFile>>,
) -> Result<ProviderSpec, IronclawError> {
    if let Some(s) = role_str {
        return resolve_model_string(s, providers_map);
    }
    if let Some(s) = default_str {
        return resolve_model_string(s, providers_map);
    }
    Ok(main.clone())
}

/// Resolve gateway configuration from TOML section and environment variables.
fn resolve_gateway_config(section: Option<&GatewayConfigFile>) -> GatewayConfig {
    let bind = std::env::var("IRONCLAW_GATEWAY_BIND")
        .ok()
        .or_else(|| section.and_then(|s| s.bind.clone()))
        .unwrap_or_else(|| DEFAULT_GATEWAY_BIND.to_string());

    let port_from_env = match std::env::var("IRONCLAW_GATEWAY_PORT") {
        Ok(val) => match val.parse::<u16>() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("warning: IRONCLAW_GATEWAY_PORT '{val}' is not a valid port: {e}");
                None
            }
        },
        Err(_) => None,
    };

    let port = port_from_env
        .or_else(|| section.and_then(|s| s.port))
        .unwrap_or(DEFAULT_GATEWAY_PORT);

    GatewayConfig { bind, port }
}

/// Resolve Discord configuration from TOML section and environment.
///
/// Token resolution: `IRONCLAW_DISCORD_TOKEN` env > `token` field in TOML (with
/// `${ENV_VAR}` expansion) > `None` if section is absent or no token found.
fn resolve_discord_config(section: Option<&DiscordConfigFile>) -> Option<DiscordConfig> {
    let token = std::env::var("IRONCLAW_DISCORD_TOKEN")
        .ok()
        .or_else(|| {
            section
                .and_then(|s| s.token.as_ref())
                .and_then(|t| expand_env_token(t))
        })
        .filter(|t| !t.is_empty());

    match (section, token) {
        (_, Some(tok)) => Some(DiscordConfig { token: tok }),
        (Some(_), None) => {
            eprintln!(
                "warning: [discord] section present but no token found; \
                 set IRONCLAW_DISCORD_TOKEN or token in config"
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

/// Resolve webhook configuration from TOML section.
fn resolve_webhook_config(section: Option<&WebhookConfigFile>) -> WebhookConfig {
    match section {
        Some(s) => WebhookConfig {
            enabled: s.enabled.unwrap_or(false),
            secret: s.secret.clone(),
        },
        None => WebhookConfig::default(),
    }
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

/// Warn on deprecated environment variables that no longer have effect.
fn warn_deprecated_env_vars() {
    let deprecated = [
        "IRONCLAW_OBSERVER_MODEL",
        "IRONCLAW_REFLECTOR_MODEL",
        "IRONCLAW_OBSERVER_API_KEY",
        "IRONCLAW_REFLECTOR_API_KEY",
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
mod tests {
    use super::super::constants::DEFAULT_ANTHROPIC_URL;
    use super::*;

    // ── Provider / model resolution ───────────────────────────────────────────

    #[test]
    fn implicit_provider_resolution() {
        let spec = resolve_model_string("anthropic/claude-sonnet-4-6", None).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::Anthropic);
        assert_eq!(spec.model.model, "claude-sonnet-4-6");
        assert_eq!(spec.provider_url, DEFAULT_ANTHROPIC_URL);
        assert_eq!(spec.name, "anthropic");
    }

    #[test]
    fn explicit_provider_resolution() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-claude".to_string(),
            ProviderEntryFile {
                kind: "anthropic".to_string(),
                api_key: Some("sk-explicit".to_string()),
                url: None,
            },
        );

        let spec = resolve_model_string("my-claude/claude-sonnet-4-6", Some(&providers)).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::Anthropic);
        assert_eq!(spec.model.model, "claude-sonnet-4-6");
        assert_eq!(spec.name, "my-claude");
        assert_eq!(spec.api_key.as_deref(), Some("sk-explicit"));
        assert_eq!(spec.provider_url, DEFAULT_ANTHROPIC_URL);
    }

    #[test]
    fn unknown_implicit_provider_errors() {
        let result = resolve_model_string("foobar/some-model", None);
        assert!(result.is_err(), "unknown implicit provider should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("foobar"),
            "error should mention the bad provider: {err}"
        );
    }

    #[test]
    fn explicit_provider_url_override() {
        let mut providers = HashMap::new();
        providers.insert(
            "cerebras".to_string(),
            ProviderEntryFile {
                kind: "openai".to_string(),
                api_key: Some("csk-123".to_string()),
                url: Some("https://api.cerebras.ai/v1".to_string()),
            },
        );

        let spec = resolve_model_string("cerebras/llama-4", Some(&providers)).unwrap();
        assert_eq!(spec.model.kind, ProviderKind::OpenAi);
        assert_eq!(spec.provider_url, "https://api.cerebras.ai/v1");
    }

    // ── Full config resolution via from_file_and_env ──────────────────────────

    #[test]
    fn default_model_fallback() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        // observer was not set, so it falls back to default
        assert_eq!(cfg.observer.model.model, "claude-haiku-4-5");
        assert_eq!(cfg.reflector.model.model, "claude-haiku-4-5");
        assert_eq!(cfg.pulse.model.model, "claude-haiku-4-5");
        assert_eq!(cfg.cron.model.model, "claude-haiku-4-5");
        // main is still the explicit main
        assert_eq!(cfg.main.model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn role_specific_overrides_default() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
observer = "gemini/gemini-3.0-flash"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert_eq!(
            cfg.observer.model.model, "gemini-3.0-flash",
            "explicit observer should override default"
        );
        assert_eq!(
            cfg.reflector.model.model, "claude-haiku-4-5",
            "unset reflector should still use default"
        );
    }

    #[test]
    fn all_roles_resolved_to_main_by_default() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert_eq!(cfg.main.model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.observer.model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.reflector.model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.pulse.model.model, "claude-sonnet-4-6");
        assert_eq!(cfg.cron.model.model, "claude-sonnet-4-6");
    }

    #[test]
    fn deny_unknown_fields_rejects_typos() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
typo_field = "oops"
"#;
        let result = toml::from_str::<ConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [models] should be rejected"
        );
    }

    #[test]
    fn deny_unknown_fields_rejects_top_level_typos() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

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
        let toml_str = r#"
timezone = "UTC"

[providers.cerebras]
type = "openai"
api_key = "csk-123"
url = "https://api.cerebras.ai/v1"

[models]
main = "cerebras/llama-4"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert_eq!(cfg.main.model.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.main.provider_url, "https://api.cerebras.ai/v1");
    }

    #[test]
    fn memory_config_just_thresholds() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[memory]
observer_threshold_tokens = 20000
reflector_threshold_tokens = 50000
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert_eq!(cfg.memory.observer_threshold_tokens, 20000);
        assert_eq!(cfg.memory.reflector_threshold_tokens, 50000);
    }

    #[test]
    fn memory_config_defaults_when_absent() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
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
        let result = from_file_and_env(None);
        assert!(result.is_err(), "missing timezone should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timezone"),
            "error should mention timezone: {err}"
        );
    }

    #[test]
    fn config_with_timezone() {
        let toml_str =
            "timezone = \"America/New_York\"\n\n[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert_eq!(
            cfg.timezone.name(),
            "America/New_York",
            "timezone should be parsed"
        );
    }

    #[test]
    fn config_invalid_timezone_errors() {
        let toml_str =
            "timezone = \"Not/A/Timezone\"\n\n[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let result = from_file_and_env(Some(&file));
        assert!(result.is_err(), "invalid timezone should error");
    }

    #[test]
    fn pulse_cron_enabled_defaults() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(cfg.pulse_enabled, "pulse should default to enabled");
        assert!(cfg.cron_enabled, "cron should default to enabled");
    }

    #[test]
    fn discord_absent_returns_none() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(
            cfg.discord.is_none(),
            "no [discord] section should yield None"
        );
    }

    #[test]
    fn discord_section_without_token_returns_none() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[discord]
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(
            cfg.discord.is_none(),
            "[discord] with no token should yield None"
        );
    }

    #[test]
    fn discord_section_with_token() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[discord]
token = "my-bot-token"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(cfg.discord.is_some(), "[discord] with token should be Some");
        assert_eq!(
            cfg.discord.as_ref().map(|d| d.token.as_str()),
            Some("my-bot-token"),
            "token should match"
        );
    }

    #[test]
    fn webhook_defaults_when_absent() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(!cfg.webhook.enabled, "webhook should default to disabled");
        assert!(
            cfg.webhook.secret.is_none(),
            "webhook secret should default to None"
        );
    }

    #[test]
    fn webhook_enabled_with_secret() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[webhook]
enabled = true
secret = "my-secret"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(cfg.webhook.enabled, "webhook should be enabled");
        assert_eq!(
            cfg.webhook.secret.as_deref(),
            Some("my-secret"),
            "webhook secret should match"
        );
    }

    #[test]
    fn memory_config_cooldown_defaults() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
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
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[memory]
observer_cooldown_secs = 60
observer_force_threshold_tokens = 50000
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
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
    fn pulse_cron_can_be_disabled() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"

[pulse]
enabled = false

[cron]
enabled = false
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = from_file_and_env(Some(&file)).unwrap();
        assert!(!cfg.pulse_enabled);
        assert!(!cfg.cron_enabled);
    }
}

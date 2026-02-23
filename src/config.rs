//! Configuration loading and validation.
//!
//! Uses a two-type pattern: `ConfigFile` (raw TOML deserialization) is validated
//! into `Config` (runtime-safe values). Providers are defined in `[providers]`,
//! models are assigned to roles in `[models]`, and everything resolves at load
//! time into fully-built `ProviderSpec` values.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::error::IronclawError;

/// Default base URL for the Anthropic API.
const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";

/// Default base URL for a local Ollama instance.
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default base URL for the `OpenAI` API.
const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";

/// Default base URL for the Google Gemini API.
const DEFAULT_GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default gateway bind address.
const DEFAULT_GATEWAY_BIND: &str = "127.0.0.1";

/// Default gateway port.
const DEFAULT_GATEWAY_PORT: u16 = 7700;

/// Default max tokens for model responses.
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Default observer token threshold before firing.
pub(crate) const DEFAULT_OBSERVER_THRESHOLD: usize = 30_000;

/// Default reflector token threshold before compressing.
pub(crate) const DEFAULT_REFLECTOR_THRESHOLD: usize = 40_000;

/// Default observer cooldown period in seconds before observation fires.
pub(crate) const DEFAULT_OBSERVER_COOLDOWN_SECS: u64 = 120;

/// Default force-observe token threshold (bypasses cooldown).
pub(crate) const DEFAULT_OBSERVER_FORCE_THRESHOLD: usize = 60_000;

/// Minimal config.toml written on first run — user edits this.
const MINIMAL_CONFIG: &str = "# IronClaw configuration. See config.example.toml for all options.\n\
    \n\
    # timezone = \"America/New_York\"  # REQUIRED: IANA timezone name\n\
    \n\
    [models]\n\
    main = \"anthropic/claude-sonnet-4-6\"\n";

/// Full reference config always regenerated on startup.
///
/// Every option is shown with its default and a brief comment.
const EXAMPLE_CONFIG: &str = include_str!("../assets/config.example.toml");

/// Resolved provider configuration for a specific role.
///
/// Every role (main, observer, reflector, pulse, cron) gets a fully resolved
/// `ProviderSpec` at config load time — no `Option` chains at use sites.
#[derive(Clone)]
pub struct ProviderSpec {
    /// Human-readable identifier (provider entry name or implicit kind).
    pub name: String,
    /// Model spec for this role.
    pub model: ModelSpec,
    /// Base URL for the provider.
    pub provider_url: String,
    /// API key (redacted in Debug output).
    pub api_key: Option<String>,
}

impl fmt::Debug for ProviderSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderSpec")
            .field("name", &self.name)
            .field("model", &self.model)
            .field("provider_url", &self.provider_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Validated runtime configuration.
///
/// All provider roles are fully resolved at load time. Consumers read fields
/// directly — no fallback chains needed.
#[derive(Clone)]
pub struct Config {
    /// Fully resolved main agent provider.
    pub main: ProviderSpec,
    /// Fully resolved observer provider.
    pub observer: ProviderSpec,
    /// Fully resolved reflector provider.
    pub reflector: ProviderSpec,
    /// Fully resolved pulse provider.
    pub pulse: ProviderSpec,
    /// Fully resolved cron provider.
    pub cron: ProviderSpec,
    /// Path to the workspace root directory.
    pub workspace_dir: PathBuf,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum tokens for model responses.
    pub max_tokens: u32,
    /// Memory subsystem configuration (thresholds only).
    pub memory: MemoryConfig,
    /// Whether the pulse system is enabled.
    pub pulse_enabled: bool,
    /// Whether the cron system is enabled.
    pub cron_enabled: bool,
    /// WebSocket gateway configuration.
    pub gateway: GatewayConfig,
    /// IANA timezone for the agent (e.g. `America/New_York`).
    pub timezone: chrono_tz::Tz,
    /// Discord bot configuration (None if `[discord]` section absent or no token).
    pub discord: Option<DiscordConfig>,
    /// Webhook endpoint configuration.
    pub webhook: WebhookConfig,
    /// Skills subsystem configuration.
    pub skills: SkillsConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("main", &self.main)
            .field("observer", &self.observer)
            .field("reflector", &self.reflector)
            .field("pulse", &self.pulse)
            .field("cron", &self.cron)
            .field("workspace_dir", &self.workspace_dir)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_tokens", &self.max_tokens)
            .field("memory", &self.memory)
            .field("pulse_enabled", &self.pulse_enabled)
            .field("cron_enabled", &self.cron_enabled)
            .field("gateway", &self.gateway)
            .field("timezone", &self.timezone)
            .field("discord", &self.discord.as_ref().map(|_| "[configured]"))
            .field("webhook", &self.webhook)
            .field("skills", &self.skills)
            .finish()
    }
}

/// Validated gateway configuration.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Address to bind the WebSocket server to.
    pub bind: String,
    /// Port for the WebSocket server.
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_GATEWAY_BIND.to_string(),
            port: DEFAULT_GATEWAY_PORT,
        }
    }
}

impl GatewayConfig {
    /// Build from the raw TOML section and environment variables.
    fn from_file_and_env(section: Option<&GatewayConfigFile>) -> Self {
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

        Self { bind, port }
    }

    /// The full socket address string (e.g. `"127.0.0.1:7700"`).
    #[must_use]
    pub fn addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}

/// Validated memory subsystem configuration (thresholds only).
///
/// Provider assignments for observer/reflector are on `Config` directly.
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Token threshold before the observer fires.
    pub observer_threshold_tokens: usize,
    /// Token threshold before the reflector compresses.
    pub reflector_threshold_tokens: usize,
    /// Cooldown period in seconds after the soft threshold is crossed.
    pub observer_cooldown_secs: u64,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    pub observer_force_threshold_tokens: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            observer_threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            reflector_threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
            observer_cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            observer_force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
        }
    }
}

/// Validated Discord bot configuration.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Bot token for the Discord API.
    pub token: String,
}

/// Validated webhook endpoint configuration.
#[derive(Debug, Clone, Default)]
pub struct WebhookConfig {
    /// Whether the webhook endpoint is enabled.
    pub enabled: bool,
    /// Optional bearer token for authenticating incoming requests.
    pub secret: Option<String>,
}

/// Validated skills subsystem configuration.
#[derive(Debug, Clone)]
pub struct SkillsConfig {
    /// Directories to scan for skills (resolved, expanded paths).
    pub dirs: Vec<PathBuf>,
}

impl Config {
    /// Write default config files to `~/.ironclaw/` if not already present.
    ///
    /// - `config.toml` is created only if absent (minimal template for the user to edit).
    /// - `config.example.toml` is always regenerated (kept in sync with the current schema).
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the config directory or files cannot be written.
    pub fn bootstrap_config_dir() -> Result<(), IronclawError> {
        let dir = default_config_dir()?;
        bootstrap_at(&dir)
    }

    /// Load configuration from the default config file and environment.
    ///
    /// Priority: env vars > config file > defaults.
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the config file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub fn load() -> Result<Self, IronclawError> {
        let config_dir = default_config_dir()?;
        let config_path = config_dir.join("config.toml");

        let file_config = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).map_err(|e| {
                IronclawError::Config(format!(
                    "failed to read config at {}: {e}",
                    config_path.display()
                ))
            })?;
            Some(toml::from_str::<ConfigFile>(&contents).map_err(|e| {
                IronclawError::Config(format!(
                    "failed to parse config at {}: {e}",
                    config_path.display()
                ))
            })?)
        } else {
            None
        };

        Self::from_file_and_env(file_config.as_ref())
    }

    /// Build a `Config` from an optional config file and environment variables.
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the model spec cannot be parsed or
    /// the workspace directory cannot be determined.
    #[expect(
        clippy::too_many_lines,
        reason = "config resolution is a single sequential pipeline; splitting would obscure the precedence chain"
    )]
    fn from_file_and_env(file: Option<&ConfigFile>) -> Result<Self, IronclawError> {
        warn_deprecated_env_vars();

        let providers_map = file.and_then(|f| f.providers.as_ref());
        let models = file.and_then(|f| f.models.as_ref());

        // Resolve main: IRONCLAW_MODEL env > models.main > default
        let main_model_str = std::env::var("IRONCLAW_MODEL")
            .ok()
            .or_else(|| models.and_then(|m| m.main.clone()))
            .unwrap_or_else(|| "anthropic/claude-sonnet-4-5".to_string());

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

        let gateway = GatewayConfig::from_file_and_env(file.and_then(|f| f.gateway.as_ref()));

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

        Ok(Self {
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
}

/// Parsed model specification from `"provider/model"` format.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// The provider kind.
    pub kind: ProviderKind,
    /// The model name (e.g. `"claude-sonnet-4-5"`).
    pub model: String,
}

impl fmt::Display for ModelSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind, self.model)
    }
}

impl FromStr for ModelSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider_str, model) = s
            .split_once('/')
            .ok_or_else(|| format!("expected 'provider/model' format, got '{s}'"))?;

        let kind = ProviderKind::from_str(provider_str)?;

        if model.is_empty() {
            return Err("model name cannot be empty".to_string());
        }

        Ok(Self {
            kind,
            model: model.to_string(),
        })
    }
}

/// Supported model provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Anthropic Messages API.
    Anthropic,
    /// Google Gemini `generateContent` API.
    Gemini,
    /// Ollama local inference.
    Ollama,
    /// OpenAI-compatible chat completions (also vLLM, LM Studio, etc.).
    OpenAi,
}

impl ProviderKind {
    /// Default base URL for this provider.
    #[must_use]
    fn default_url(self) -> &'static str {
        match self {
            Self::Anthropic => DEFAULT_ANTHROPIC_URL,
            Self::Gemini => DEFAULT_GEMINI_URL,
            Self::Ollama => DEFAULT_OLLAMA_URL,
            Self::OpenAi => DEFAULT_OPENAI_URL,
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::Gemini => write!(f, "gemini"),
            Self::Ollama => write!(f, "ollama"),
            Self::OpenAi => write!(f, "openai"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown provider '{other}', expected one of: anthropic, gemini, ollama, openai"
            )),
        }
    }
}

// ── TOML deserialization structs ─────────────────────────────────────────────

/// Raw TOML config file structure (deserialized directly).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// IANA timezone name (e.g. `"America/New_York"`).
    timezone: Option<String>,
    /// Named provider definitions.
    providers: Option<HashMap<String, ProviderEntryFile>>,
    /// Role → model string assignments.
    models: Option<ModelsConfigFile>,
    /// Workspace root directory path.
    workspace_dir: Option<String>,
    /// Request timeout in seconds.
    timeout_secs: Option<u64>,
    /// Maximum tokens for model responses.
    max_tokens: Option<u32>,
    /// Memory subsystem configuration.
    memory: Option<MemoryConfigFile>,
    /// Pulse subsystem configuration.
    pulse: Option<PulseConfigFile>,
    /// Cron subsystem configuration.
    cron: Option<CronConfigFile>,
    /// Gateway configuration.
    gateway: Option<GatewayConfigFile>,
    /// Discord bot configuration.
    discord: Option<DiscordConfigFile>,
    /// Webhook endpoint configuration.
    webhook: Option<WebhookConfigFile>,
    /// Skills subsystem configuration.
    skills: Option<SkillsConfigFile>,
}

/// A named provider entry under `[providers.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderEntryFile {
    /// Provider protocol type (e.g. `"openai"`, `"anthropic"`).
    #[serde(rename = "type")]
    kind: String,
    /// API key.
    api_key: Option<String>,
    /// Override base URL.
    url: Option<String>,
}

/// Raw TOML `[models]` section — maps roles to `"provider/model"` strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsConfigFile {
    /// Main agent model (required for operation).
    main: Option<String>,
    /// Default fallback for unset roles.
    default: Option<String>,
    /// Memory observer model.
    observer: Option<String>,
    /// Memory reflector model.
    reflector: Option<String>,
    /// Pulse agent model.
    pulse: Option<String>,
    /// Cron agent model.
    cron: Option<String>,
}

/// Raw TOML `[memory]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryConfigFile {
    /// Token threshold before the observer fires.
    observer_threshold_tokens: Option<usize>,
    /// Token threshold before the reflector compresses.
    reflector_threshold_tokens: Option<usize>,
    /// Cooldown period in seconds after the soft threshold is crossed.
    observer_cooldown_secs: Option<u64>,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    observer_force_threshold_tokens: Option<usize>,
}

/// Raw TOML `[pulse]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PulseConfigFile {
    /// Whether the pulse system is enabled.
    enabled: Option<bool>,
}

/// Raw TOML `[cron]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CronConfigFile {
    /// Whether the cron system is enabled.
    enabled: Option<bool>,
}

/// Raw TOML `[gateway]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayConfigFile {
    /// Address to bind the WebSocket server to.
    bind: Option<String>,
    /// Port for the WebSocket server.
    port: Option<u16>,
}

/// Raw TOML `[discord]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscordConfigFile {
    /// Bot token (supports `${ENV_VAR}` syntax).
    token: Option<String>,
}

/// Raw TOML `[webhook]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookConfigFile {
    /// Whether the webhook endpoint is enabled.
    enabled: Option<bool>,
    /// Optional bearer token for authentication.
    secret: Option<String>,
}

/// Raw TOML `[skills]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillsConfigFile {
    /// Additional directories to scan for skills.
    dirs: Option<Vec<String>>,
}

// ── Resolution logic ─────────────────────────────────────────────────────────

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

/// Get the default config directory (`~/.ironclaw/`).
fn default_config_dir() -> Result<PathBuf, IronclawError> {
    dirs::home_dir()
        .map(|h| h.join(".ironclaw"))
        .ok_or_else(|| IronclawError::Config("could not determine home directory".to_string()))
}

/// Get the default workspace directory (`~/.ironclaw/workspace/`).
fn default_workspace_dir() -> Result<PathBuf, IronclawError> {
    default_config_dir().map(|d| d.join("workspace"))
}

/// Write bootstrap config files to `dir`.
///
/// Creates the directory if absent, writes `config.toml` only if absent,
/// and always regenerates `config.example.toml`.
fn bootstrap_at(dir: &Path) -> Result<(), IronclawError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| {
            IronclawError::Config(format!(
                "failed to create config directory {}: {e}",
                dir.display()
            ))
        })?;
    }

    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        std::fs::write(&config_path, MINIMAL_CONFIG).map_err(|e| {
            IronclawError::Config(format!(
                "failed to write config.toml at {}: {e}",
                config_path.display()
            ))
        })?;
        tracing::info!(path = %config_path.display(), "wrote initial config.toml");
    }

    let example_path = dir.join("config.example.toml");
    std::fs::write(&example_path, EXAMPLE_CONFIG).map_err(|e| {
        IronclawError::Config(format!(
            "failed to write config.example.toml at {}: {e}",
            example_path.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── ModelSpec parsing (unchanged) ────────────────────────────────────────

    #[test]
    fn model_spec_parse_valid() {
        let spec = ModelSpec::from_str("anthropic/claude-sonnet-4-5").unwrap();
        assert_eq!(spec.kind, ProviderKind::Anthropic, "provider should parse");
        assert_eq!(spec.model, "claude-sonnet-4-5", "model name should parse");
    }

    #[test]
    fn model_spec_parse_ollama() {
        let spec = ModelSpec::from_str("ollama/llama3").unwrap();
        assert_eq!(spec.kind, ProviderKind::Ollama, "ollama should parse");
        assert_eq!(spec.model, "llama3", "model should parse");
    }

    #[test]
    fn model_spec_parse_openai() {
        let spec = ModelSpec::from_str("openai/gpt-4o").unwrap();
        assert_eq!(spec.kind, ProviderKind::OpenAi, "openai should parse");
        assert_eq!(spec.model, "gpt-4o", "model should parse");
    }

    #[test]
    fn model_spec_parse_no_slash() {
        let result = ModelSpec::from_str("just-a-model");
        assert!(result.is_err(), "should require provider/model format");
    }

    #[test]
    fn model_spec_parse_empty_model() {
        let result = ModelSpec::from_str("anthropic/");
        assert!(result.is_err(), "should reject empty model name");
    }

    #[test]
    fn model_spec_parse_unknown_provider() {
        let result = ModelSpec::from_str("unknown/model");
        assert!(result.is_err(), "should reject unknown provider");
    }

    #[test]
    fn model_spec_display() {
        let spec = ModelSpec {
            kind: ProviderKind::Anthropic,
            model: "claude-sonnet-4-5".to_string(),
        };
        assert_eq!(
            spec.to_string(),
            "anthropic/claude-sonnet-4-5",
            "display should round-trip"
        );
    }

    #[test]
    fn model_spec_parse_gemini() {
        let spec = ModelSpec::from_str("gemini/gemini-2.0-flash").unwrap();
        assert_eq!(spec.kind, ProviderKind::Gemini, "gemini should parse");
        assert_eq!(spec.model, "gemini-2.0-flash", "model should parse");
    }

    #[test]
    fn provider_kind_case_insensitive() {
        assert_eq!(
            ProviderKind::from_str("Anthropic").unwrap(),
            ProviderKind::Anthropic,
            "should be case-insensitive"
        );
        assert_eq!(
            ProviderKind::from_str("OLLAMA").unwrap(),
            ProviderKind::Ollama,
            "should be case-insensitive"
        );
    }

    #[test]
    fn config_defaults_provider_kind() {
        assert_eq!(
            ProviderKind::Anthropic.default_url(),
            DEFAULT_ANTHROPIC_URL,
            "anthropic default URL"
        );
        assert_eq!(
            ProviderKind::Ollama.default_url(),
            DEFAULT_OLLAMA_URL,
            "ollama default URL"
        );
        assert_eq!(
            ProviderKind::OpenAi.default_url(),
            DEFAULT_OPENAI_URL,
            "openai default URL"
        );
        assert_eq!(
            ProviderKind::Gemini.default_url(),
            DEFAULT_GEMINI_URL,
            "gemini default URL"
        );
    }

    // ── Bootstrap tests ──────────────────────────────────────────────────────

    #[test]
    fn bootstrap_creates_config_dir() {
        let base = tempdir().unwrap();
        let dir = base.path().join("newdir");
        assert!(!dir.exists(), "dir should not exist before bootstrap");
        bootstrap_at(&dir).unwrap();
        assert!(dir.exists(), "dir should be created by bootstrap");
    }

    #[test]
    fn bootstrap_writes_minimal_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists(), "config.toml should not exist yet");
        bootstrap_at(dir.path()).unwrap();
        assert!(config_path.exists(), "config.toml should be written");
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            body.contains("[models]"),
            "config.toml should contain [models] section"
        );
        assert!(body.contains("main"), "config.toml should contain main key");
    }

    #[test]
    fn bootstrap_skips_existing_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "# user customization").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            body, "# user customization",
            "existing config.toml should not be overwritten"
        );
    }

    #[test]
    fn bootstrap_always_writes_example() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(
            body, "# old content",
            "config.example.toml should be regenerated"
        );
        assert!(
            body.contains("[models]"),
            "example should contain [models] section"
        );
    }

    #[test]
    fn bootstrap_example_contains_all_sections() {
        let dir = tempdir().unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
        assert!(
            body.contains("[providers]") || body.contains("providers"),
            "example should document providers"
        );
        assert!(
            body.contains("[models]"),
            "example should contain models section"
        );
        assert!(
            body.contains("[memory]"),
            "example should contain memory section"
        );
        assert!(
            body.contains("[pulse]"),
            "example should contain pulse section"
        );
        assert!(
            body.contains("[cron]"),
            "example should contain cron section"
        );
        assert!(
            body.contains("[gateway]"),
            "example should contain gateway section"
        );
        assert!(
            body.contains("[discord]"),
            "example should document discord section"
        );
        assert!(
            body.contains("[webhook]"),
            "example should document webhook section"
        );
        assert!(
            body.contains("[skills]"),
            "example should document skills section"
        );
    }

    // ── New provider / model resolution tests ────────────────────────────────

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

    #[test]
    fn default_model_fallback() {
        let toml_str = r#"
timezone = "UTC"

[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let result = Config::from_file_and_env(None);
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let result = Config::from_file_and_env(Some(&file));
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
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
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
        assert!(!cfg.pulse_enabled);
        assert!(!cfg.cron_enabled);
    }
}

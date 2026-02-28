//! Configuration loading and validation.
//!
//! Uses a two-type pattern: raw TOML deserialization structs (in `deserialize`)
//! are validated into `Config` (runtime-safe values). Providers are defined in
//! `[providers]`, models are assigned to roles in `[models]`, and everything
//! resolves at load time into fully-built `ProviderSpec` values.

mod bootstrap;
mod constants;
mod deserialize;
mod provider;
mod resolve;
mod secrets;
mod types;
pub mod wizard;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::IronclawError;
use crate::models::retry::RetryConfig;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub(crate) use constants::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD,
};
pub use provider::{ModelSpec, ProviderKind, ProviderSpec};
pub use secrets::SecretStore;
pub use types::{
    BackgroundConfig, BackgroundModelTier, BackgroundModelsConfig, DiscordConfig,
    ExternalChannelConfig, ExternalChannelKind, GatewayConfig, McpConfig, MemoryConfig,
    NotificationsConfig, SearchConfig, SkillsConfig, WebhookConfig,
};

// ── Config struct ─────────────────────────────────────────────────────────────

/// Validated runtime configuration.
///
/// All provider roles are fully resolved at load time. Consumers read fields
/// directly — no fallback chains needed.
#[derive(Clone)]
pub struct Config {
    /// User's display name (what the agent calls them).
    pub name: Option<String>,
    /// Fully resolved main agent provider chain (failover).
    pub main: Vec<ProviderSpec>,
    /// Fully resolved observer provider chain (failover).
    pub observer: Vec<ProviderSpec>,
    /// Fully resolved reflector provider chain (failover).
    pub reflector: Vec<ProviderSpec>,
    /// Fully resolved pulse provider chain (failover).
    pub pulse: Vec<ProviderSpec>,
    /// Fully resolved embedding provider (None if not configured).
    pub embedding: Option<ProviderSpec>,
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
    /// MCP server configuration (global servers).
    pub mcp: McpConfig,
    /// Retry configuration for model provider calls.
    pub retry: RetryConfig,
    /// Notification channel configuration.
    pub notifications: NotificationsConfig,
    /// Background task configuration.
    pub background: BackgroundConfig,
    /// Directory this config was loaded from.
    pub config_dir: PathBuf,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("name", &self.name)
            .field("main", &self.main)
            .field("observer", &self.observer)
            .field("reflector", &self.reflector)
            .field("pulse", &self.pulse)
            .field("embedding", &self.embedding)
            .field("workspace_dir", &self.workspace_dir)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_tokens", &self.max_tokens)
            .field("memory", &self.memory)
            .field("pulse_enabled", &self.pulse_enabled)
            .field("gateway", &self.gateway)
            .field("timezone", &self.timezone)
            .field("discord", &self.discord.as_ref().map(|_| "[configured]"))
            .field("webhook", &self.webhook)
            .field("skills", &self.skills)
            .field("mcp", &self.mcp)
            .field("retry", &self.retry)
            .field("notifications", &self.notifications)
            .field("background", &self.background)
            .field("config_dir", &self.config_dir)
            .finish()
    }
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
        let dir = bootstrap::default_config_dir()?;
        bootstrap::bootstrap_at(&dir)
    }

    /// Write default config files to an arbitrary directory.
    ///
    /// Same as [`bootstrap_config_dir`](Self::bootstrap_config_dir) but targets
    /// a caller-specified path instead of `~/.ironclaw/`.
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the directory or files cannot be written.
    pub fn bootstrap_at_dir(dir: &Path) -> Result<(), IronclawError> {
        bootstrap::bootstrap_at(dir)
    }

    /// Get the default config directory path (`~/.ironclaw/`).
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the home directory cannot be determined.
    pub fn config_dir() -> Result<PathBuf, IronclawError> {
        bootstrap::default_config_dir()
    }

    /// Load configuration from the default config file and environment.
    ///
    /// Priority: env vars > config file > defaults.
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the config file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub fn load() -> Result<Self, IronclawError> {
        let config_dir = bootstrap::default_config_dir()?;
        let mut cfg = Self::load_at(&config_dir)?;
        cfg.config_dir = config_dir;
        Ok(cfg)
    }

    /// Load configuration from a specific directory.
    ///
    /// Same as [`load`](Self::load) but reads `config.toml` from the given
    /// directory instead of the default `~/.ironclaw/`.
    ///
    /// # Errors
    /// Returns `IronclawError::Config` if the config file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub fn load_at(config_dir: &std::path::Path) -> Result<Self, IronclawError> {
        let config_path = config_dir.join("config.toml");

        let file_config = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).map_err(|e| {
                IronclawError::Config(format!(
                    "failed to read config at {}: {e}",
                    config_path.display()
                ))
            })?;
            Some(
                toml::from_str::<deserialize::ConfigFile>(&contents).map_err(|e| {
                    IronclawError::Config(format!(
                        "failed to parse config at {}: {e}",
                        config_path.display()
                    ))
                })?,
            )
        } else {
            None
        };

        let mut cfg = resolve::from_file_and_env(file_config.as_ref(), config_dir)?;
        cfg.config_dir = config_dir.to_path_buf();
        Ok(cfg)
    }

    /// Validate a TOML string as a config file without saving it.
    ///
    /// Parses the TOML into the raw config structure, then runs full resolution
    /// to catch semantic errors (bad provider names, missing timezone, etc.).
    /// Uses an empty secret store since validation doesn't need real secrets.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_toml(contents: &str) -> Result<(), String> {
        let file = toml::from_str::<deserialize::ConfigFile>(contents)
            .map_err(|e| format!("TOML parse error: {e}"))?;

        // Use a temp dir for validation — secrets aren't needed for structural checks
        let temp_dir = std::env::temp_dir().join("ironclaw-validate");
        resolve::from_file_and_env(Some(&file), &temp_dir).map_err(|e| format!("{e}"))?;
        Ok(())
    }
}

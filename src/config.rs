//! Configuration loading and validation.
//!
//! Uses a two-type pattern: `ConfigFile` (raw TOML deserialization) is validated
//! into `Config` (runtime-safe values).

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

/// Minimal config.toml written on first run — user edits this.
const MINIMAL_CONFIG: &str = "# IronClaw configuration. See config.example.toml for all options.\n\
    \n\
    model = \"anthropic/claude-sonnet-4-6\"\n\
    api_key = \"sk-ant-REPLACE_ME\"\n";

/// Full reference config always regenerated on startup.
///
/// Every option is shown with its default and a brief comment.
const EXAMPLE_CONFIG: &str = "# IronClaw example configuration — all options with defaults shown.\n\
    # Copy to config.toml and edit as needed. Unknown keys are ignored.\n\
    \n\
    # ── Main agent provider ────────────────────────────────────────────────────\n\
    model = \"anthropic/claude-sonnet-4-6\"\n\
    # Override base URL (default depends on provider prefix):\n\
    #   anthropic → https://api.anthropic.com\n\
    #   openai    → https://api.openai.com/v1\n\
    #   ollama    → http://localhost:11434\n\
    # provider_url = \"https://api.anthropic.com\"\n\
    api_key = \"sk-ant-REPLACE_ME\"\n\
    # workspace_dir = \"~/.ironclaw/workspace\"\n\
    timeout_secs = 120\n\
    max_tokens = 8192\n\
    \n\
    # ── Named providers (optional, DRY alternative to per-section inline config)\n\
    # Define reusable provider entries, then reference them in [roles].\n\
    #\n\
    # [providers.main]\n\
    # model = \"anthropic/claude-sonnet-4-6\"\n\
    # api_key = \"sk-ant-...\"\n\
    #\n\
    # [providers.fast]\n\
    # model = \"anthropic/claude-haiku-4-5\"\n\
    # api_key = \"sk-ant-...\"\n\
    #\n\
    # [providers.local]\n\
    # model = \"ollama/llama3\"\n\
    # provider_url = \"http://localhost:11434\"\n\
    \n\
    # ── Role assignments (optional, references entries in [providers]) ──────────\n\
    # Override which provider each subsystem uses.\n\
    # Inline model/api_key fields in [memory]/[pulse]/[cron] also work.\n\
    #\n\
    # [roles]\n\
    # agent     = \"main\"    # top-level agent\n\
    # observer  = \"fast\"    # memory observer\n\
    # reflector = \"fast\"    # memory reflector\n\
    # pulse     = \"fast\"    # pulse (heartbeat) checks\n\
    # cron      = \"fast\"    # cron job agent turns\n\
    \n\
    # ── Memory subsystem ────────────────────────────────────────────────────────\n\
    [memory]\n\
    # observer_model = \"anthropic/claude-haiku-4-5\"\n\
    # observer_provider_url = \"https://api.anthropic.com\"\n\
    # observer_api_key = \"sk-ant-...\"\n\
    observer_threshold_tokens = 30000\n\
    # reflector_model = \"anthropic/claude-haiku-4-5\"\n\
    # reflector_provider_url = \"https://api.anthropic.com\"\n\
    # reflector_api_key = \"sk-ant-...\"\n\
    reflector_threshold_tokens = 40000\n\
    \n\
    # ── Pulse (ambient monitoring) ──────────────────────────────────────────────\n\
    [pulse]\n\
    enabled = true\n\
    # model = \"anthropic/claude-haiku-4-5\"\n\
    # provider_url = \"https://api.anthropic.com\"\n\
    # api_key = \"sk-ant-...\"\n\
    \n\
    # ── Cron (scheduled tasks) ──────────────────────────────────────────────────\n\
    [cron]\n\
    enabled = true\n\
    # model = \"anthropic/claude-haiku-4-5\"\n\
    # provider_url = \"https://api.anthropic.com\"\n\
    # api_key = \"sk-ant-...\"\n\
    \n\
    # ── WebSocket gateway ───────────────────────────────────────────────────────\n\
    [gateway]\n\
    bind = \"127.0.0.1\"\n\
    port = 7700\n";

/// Resolved provider configuration for a specific role.
///
/// The `name` is the TOML map key for named providers (e.g. `"cerebras"` from
/// `[providers.cerebras]`) or the role label (e.g. `"pulse"`) for inline
/// section overrides. It is used for logging and error attribution.
#[derive(Clone)]
pub struct ProviderSpec {
    /// Human-readable identifier for this provider entry.
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
#[derive(Clone)]
pub struct Config {
    /// Which model to use (parsed from `"provider/model"` format).
    pub model: ModelSpec,
    /// Base URL for the provider API.
    pub provider_url: String,
    /// API key for the provider (required for Anthropic/OpenAI, optional for Ollama).
    pub api_key: Option<String>,
    /// Path to the workspace root directory.
    pub workspace_dir: PathBuf,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum tokens for model responses.
    pub max_tokens: u32,
    /// Memory subsystem configuration.
    pub memory: MemoryConfig,
    /// Pulse (ambient monitoring) configuration.
    pub pulse: PulseConfig,
    /// Cron (scheduled tasks) configuration.
    pub cron: CronConfig,
    /// WebSocket gateway configuration.
    pub gateway: GatewayConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("model", &self.model)
            .field("provider_url", &self.provider_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("workspace_dir", &self.workspace_dir)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_tokens", &self.max_tokens)
            .field("memory", &self.memory)
            .field("pulse", &self.pulse)
            .field("cron", &self.cron)
            .field("gateway", &self.gateway)
            .finish()
    }
}

/// Validated pulse subsystem configuration.
#[derive(Debug, Clone)]
pub struct PulseConfig {
    /// Whether the pulse system is enabled.
    pub enabled: bool,
    /// Optional provider override for pulse agent turns.
    pub provider: Option<ProviderSpec>,
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: None,
        }
    }
}

/// Validated cron subsystem configuration.
#[derive(Debug, Clone)]
pub struct CronConfig {
    /// Whether the cron system is enabled.
    pub enabled: bool,
    /// Optional provider override for cron agent turns.
    pub provider: Option<ProviderSpec>,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: None,
        }
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

        let port = std::env::var("IRONCLAW_GATEWAY_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
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

/// Validated memory subsystem configuration.
#[derive(Clone)]
pub struct MemoryConfig {
    /// Model spec for the observer (None = use main model).
    pub observer_model: Option<ModelSpec>,
    /// Base URL for the observer's provider (None = use main provider URL).
    pub observer_provider_url: Option<String>,
    /// API key for the observer's provider (None = use main API key).
    pub observer_api_key: Option<String>,
    /// Token threshold before the observer fires.
    pub observer_threshold_tokens: usize,
    /// Model spec for the reflector (None = fall back to observer, then main).
    pub reflector_model: Option<ModelSpec>,
    /// Base URL for the reflector's provider (None = fall back to observer, then main).
    pub reflector_provider_url: Option<String>,
    /// API key for the reflector's provider (None = fall back to observer, then main).
    pub reflector_api_key: Option<String>,
    /// Token threshold before the reflector compresses.
    pub reflector_threshold_tokens: usize,
}

impl fmt::Debug for MemoryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryConfig")
            .field("observer_model", &self.observer_model)
            .field("observer_provider_url", &self.observer_provider_url)
            .field(
                "observer_api_key",
                &self.observer_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("observer_threshold_tokens", &self.observer_threshold_tokens)
            .field("reflector_model", &self.reflector_model)
            .field("reflector_provider_url", &self.reflector_provider_url)
            .field(
                "reflector_api_key",
                &self.reflector_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "reflector_threshold_tokens",
                &self.reflector_threshold_tokens,
            )
            .finish()
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            observer_model: None,
            observer_provider_url: None,
            observer_api_key: None,
            observer_threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            reflector_model: None,
            reflector_provider_url: None,
            reflector_api_key: None,
            reflector_threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
        }
    }
}

impl MemoryConfig {
    /// Build from the raw TOML section and environment variables.
    fn from_file_and_env(section: Option<&MemoryConfigFile>) -> Self {
        let observer_model_str = std::env::var("IRONCLAW_OBSERVER_MODEL")
            .ok()
            .or_else(|| section.and_then(|s| s.observer_model.clone()));

        let observer_model = observer_model_str.and_then(|s| match ModelSpec::from_str(&s) {
            Ok(spec) => Some(spec),
            Err(e) => {
                tracing::warn!(value = %s, error = %e, "invalid observer model spec, falling back to main model");
                None
            }
        });

        let observer_provider_url = section.and_then(|s| s.observer_provider_url.clone());

        let observer_api_key = std::env::var("IRONCLAW_OBSERVER_API_KEY")
            .ok()
            .or_else(|| section.and_then(|s| s.observer_api_key.clone()));

        let observer_threshold_tokens = section
            .and_then(|s| s.observer_threshold_tokens)
            .unwrap_or(DEFAULT_OBSERVER_THRESHOLD);

        // Reflector fields — env vars override config file, fall back to observer via gateway
        let reflector_model_str = std::env::var("IRONCLAW_REFLECTOR_MODEL")
            .ok()
            .or_else(|| section.and_then(|s| s.reflector_model.clone()));

        let reflector_model = reflector_model_str.and_then(|s| match ModelSpec::from_str(&s) {
            Ok(spec) => Some(spec),
            Err(e) => {
                tracing::warn!(value = %s, error = %e, "invalid reflector model spec, falling back to observer model");
                None
            }
        });

        let reflector_provider_url = section.and_then(|s| s.reflector_provider_url.clone());

        let reflector_api_key = std::env::var("IRONCLAW_REFLECTOR_API_KEY")
            .ok()
            .or_else(|| section.and_then(|s| s.reflector_api_key.clone()));

        let reflector_threshold_tokens = section
            .and_then(|s| s.reflector_threshold_tokens)
            .unwrap_or(DEFAULT_REFLECTOR_THRESHOLD);

        Self {
            observer_model,
            observer_provider_url,
            observer_api_key,
            observer_threshold_tokens,
            reflector_model,
            reflector_provider_url,
            reflector_api_key,
            reflector_threshold_tokens,
        }
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
        reason = "wires up all config sections sequentially; splitting would obscure the priority chain"
    )]
    fn from_file_and_env(file: Option<&ConfigFile>) -> Result<Self, IronclawError> {
        // Model: env > file > default
        let model_str = std::env::var("IRONCLAW_MODEL")
            .ok()
            .or_else(|| file.and_then(|f| f.model.clone()))
            .unwrap_or_else(|| "anthropic/claude-sonnet-4-5".to_string());

        let mut model = ModelSpec::from_str(&model_str)
            .map_err(|e| IronclawError::Config(format!("invalid model spec: {e}")))?;

        // Provider URL: env > file > default per provider
        let mut provider_url = std::env::var("IRONCLAW_PROVIDER_URL")
            .ok()
            .or_else(|| file.and_then(|f| f.provider_url.clone()))
            .unwrap_or_else(|| model.kind.default_url().to_string());

        // API key: provider-specific env > generic env > file
        let mut api_key = provider_api_key_env(model.kind)
            .or_else(|| std::env::var("IRONCLAW_API_KEY").ok())
            .or_else(|| file.and_then(|f| f.api_key.clone()));

        // roles.agent → override the main provider fields
        let providers = file.and_then(|f| f.providers.as_ref());
        if let Some(agent_role) = file
            .and_then(|f| f.roles.as_ref())
            .and_then(|r| r.agent.as_deref())
        {
            let entry = providers.and_then(|p| p.get(agent_role)).ok_or_else(|| {
                IronclawError::Config(format!(
                    "roles.agent references unknown provider '{agent_role}'"
                ))
            })?;

            if let Some(m) = &entry.model {
                let new_model = ModelSpec::from_str(m).map_err(|e| {
                    IronclawError::Config(format!("invalid model in provider '{agent_role}': {e}"))
                })?;
                // Re-derive URL from new model's default only if URL was not explicitly set
                if entry.provider_url.is_none()
                    && std::env::var("IRONCLAW_PROVIDER_URL").is_err()
                    && file.and_then(|f| f.provider_url.as_ref()).is_none()
                {
                    provider_url = new_model.kind.default_url().to_string();
                }
                model = new_model;
            }
            if let Some(url) = &entry.provider_url {
                provider_url.clone_from(url);
            }
            if let Some(key) = &entry.api_key {
                api_key = Some(key.clone());
            }
        }

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

        let mut memory = MemoryConfig::from_file_and_env(file.and_then(|f| f.memory.as_ref()));

        // roles.observer → override memory observer fields (named provider beats inline)
        if let Some(obs_role) = file
            .and_then(|f| f.roles.as_ref())
            .and_then(|r| r.observer.as_deref())
            && let Some(spec) = resolve_role_provider(
                Some(obs_role),
                "observer",
                None,
                None,
                None,
                providers,
                &model,
                &provider_url,
                api_key.as_deref(),
            )?
        {
            memory.observer_model = Some(spec.model);
            memory.observer_provider_url = Some(spec.provider_url);
            memory.observer_api_key = spec.api_key;
        }

        // roles.reflector → override memory reflector fields (named provider beats inline)
        if let Some(ref_role) = file
            .and_then(|f| f.roles.as_ref())
            .and_then(|r| r.reflector.as_deref())
            && let Some(spec) = resolve_role_provider(
                Some(ref_role),
                "reflector",
                None,
                None,
                None,
                providers,
                &model,
                &provider_url,
                api_key.as_deref(),
            )?
        {
            memory.reflector_model = Some(spec.model);
            memory.reflector_provider_url = Some(spec.provider_url);
            memory.reflector_api_key = spec.api_key;
        }

        // Pulse provider resolution
        let pulse_section = file.and_then(|f| f.pulse.as_ref());
        let pulse_enabled = pulse_section.and_then(|s| s.enabled).unwrap_or(true);
        let pulse_provider = resolve_role_provider(
            file.and_then(|f| f.roles.as_ref())
                .and_then(|r| r.pulse.as_deref()),
            "pulse",
            pulse_section.and_then(|s| s.model.as_deref()),
            pulse_section.and_then(|s| s.provider_url.as_deref()),
            pulse_section.and_then(|s| s.api_key.as_deref()),
            providers,
            &model,
            &provider_url,
            api_key.as_deref(),
        )?;
        let pulse = PulseConfig {
            enabled: pulse_enabled,
            provider: pulse_provider,
        };

        // Cron provider resolution
        let cron_section = file.and_then(|f| f.cron.as_ref());
        let cron_enabled = cron_section.and_then(|s| s.enabled).unwrap_or(true);
        let cron_provider = resolve_role_provider(
            file.and_then(|f| f.roles.as_ref())
                .and_then(|r| r.cron.as_deref()),
            "cron",
            cron_section.and_then(|s| s.model.as_deref()),
            cron_section.and_then(|s| s.provider_url.as_deref()),
            cron_section.and_then(|s| s.api_key.as_deref()),
            providers,
            &model,
            &provider_url,
            api_key.as_deref(),
        )?;
        let cron = CronConfig {
            enabled: cron_enabled,
            provider: cron_provider,
        };

        let gateway = GatewayConfig::from_file_and_env(file.and_then(|f| f.gateway.as_ref()));

        Ok(Self {
            model,
            provider_url,
            api_key,
            workspace_dir,
            timeout_secs,
            max_tokens,
            memory,
            pulse,
            cron,
            gateway,
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
            Self::Ollama => DEFAULT_OLLAMA_URL,
            Self::OpenAi => DEFAULT_OPENAI_URL,
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
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
            "ollama" => Ok(Self::Ollama),
            "openai" => Ok(Self::OpenAi),
            other => Err(format!(
                "unknown provider '{other}', expected one of: anthropic, ollama, openai"
            )),
        }
    }
}

/// Raw TOML config file structure (deserialized directly).
#[derive(Debug, Deserialize)]
struct ConfigFile {
    /// Model in `"provider/model"` format.
    model: Option<String>,
    /// Override provider base URL.
    provider_url: Option<String>,
    /// API key for the provider.
    api_key: Option<String>,
    /// Workspace root directory path.
    workspace_dir: Option<String>,
    /// Request timeout in seconds.
    timeout_secs: Option<u64>,
    /// Maximum tokens for model responses.
    max_tokens: Option<u32>,
    /// Named provider definitions.
    providers: Option<HashMap<String, ProviderEntryFile>>,
    /// Role → provider name assignments.
    roles: Option<RolesConfigFile>,
    /// Memory subsystem configuration.
    memory: Option<MemoryConfigFile>,
    /// Pulse subsystem configuration.
    pulse: Option<PulseConfigFile>,
    /// Cron subsystem configuration.
    cron: Option<CronConfigFile>,
    /// Gateway configuration.
    gateway: Option<GatewayConfigFile>,
}

/// A named provider entry under `[providers.<name>]`.
#[derive(Debug, Deserialize)]
struct ProviderEntryFile {
    /// Model in `"provider/model"` format.
    model: Option<String>,
    /// Override base URL.
    provider_url: Option<String>,
    /// API key.
    api_key: Option<String>,
}

/// Raw TOML `[roles]` section.
#[derive(Debug, Deserialize)]
struct RolesConfigFile {
    /// Named provider for the main agent.
    agent: Option<String>,
    /// Named provider for the memory observer.
    observer: Option<String>,
    /// Named provider for the memory reflector.
    reflector: Option<String>,
    /// Named provider for pulse checks.
    pulse: Option<String>,
    /// Named provider for cron agent turns.
    cron: Option<String>,
}

/// Raw TOML `[pulse]` section.
#[derive(Debug, Deserialize)]
struct PulseConfigFile {
    /// Whether the pulse system is enabled.
    enabled: Option<bool>,
    /// Model override in `"provider/model"` format.
    model: Option<String>,
    /// Provider URL override.
    provider_url: Option<String>,
    /// API key override.
    api_key: Option<String>,
}

/// Raw TOML `[cron]` section.
#[derive(Debug, Deserialize)]
struct CronConfigFile {
    /// Whether the cron system is enabled.
    enabled: Option<bool>,
    /// Model override in `"provider/model"` format.
    model: Option<String>,
    /// Provider URL override.
    provider_url: Option<String>,
    /// API key override.
    api_key: Option<String>,
}

/// Raw TOML `[gateway]` section.
#[derive(Debug, Deserialize)]
struct GatewayConfigFile {
    /// Address to bind the WebSocket server to.
    bind: Option<String>,
    /// Port for the WebSocket server.
    port: Option<u16>,
}

/// Raw TOML `[memory]` section.
#[derive(Debug, Deserialize)]
struct MemoryConfigFile {
    /// Model for the observer in `"provider/model"` format.
    observer_model: Option<String>,
    /// Base URL for the observer's provider.
    observer_provider_url: Option<String>,
    /// API key for the observer's provider.
    observer_api_key: Option<String>,
    /// Token threshold before the observer fires.
    observer_threshold_tokens: Option<usize>,
    /// Model for the reflector in `"provider/model"` format.
    reflector_model: Option<String>,
    /// Base URL for the reflector's provider.
    reflector_provider_url: Option<String>,
    /// API key for the reflector's provider.
    reflector_api_key: Option<String>,
    /// Token threshold before the reflector compresses.
    reflector_threshold_tokens: Option<usize>,
}

/// Resolve a per-role provider from named provider or inline fields.
///
/// Priority: named provider (via `role_name`) > inline fields > `main_*` fallbacks.
/// Returns `None` if neither a role name nor any inline field is present — meaning
/// the role should use the main provider unchanged.
///
/// `role_label` is the name embedded in the returned `ProviderSpec` when no
/// named provider is used (i.e. for inline section overrides). When a named
/// provider is used, the TOML map key (`role_name`) becomes the name.
///
/// # Errors
/// Returns `IronclawError::Config` if `role_name` is set but does not match any
/// entry in `providers`, or if a model string cannot be parsed.
#[expect(
    clippy::too_many_arguments,
    reason = "provider resolution requires both inline fields and main-config fallbacks; a struct would add indirection without clarity"
)]
fn resolve_role_provider(
    role_name: Option<&str>,
    role_label: &str,
    inline_model: Option<&str>,
    inline_url: Option<&str>,
    inline_key: Option<&str>,
    providers: Option<&HashMap<String, ProviderEntryFile>>,
    main_model: &ModelSpec,
    main_url: &str,
    main_key: Option<&str>,
) -> Result<Option<ProviderSpec>, IronclawError> {
    if let Some(provider_name) = role_name {
        let entry = providers
            .and_then(|p| p.get(provider_name))
            .ok_or_else(|| {
                IronclawError::Config(format!(
                    "role references unknown provider '{provider_name}'"
                ))
            })?;

        let model = if let Some(m) = entry.model.as_deref() {
            ModelSpec::from_str(m).map_err(|e| {
                IronclawError::Config(format!("invalid model in provider '{provider_name}': {e}"))
            })?
        } else {
            main_model.clone()
        };

        let provider_url = entry
            .provider_url
            .as_deref()
            .unwrap_or(main_url)
            .to_string();

        let api_key = entry.api_key.as_deref().or(main_key).map(str::to_owned);

        return Ok(Some(ProviderSpec {
            name: provider_name.to_owned(),
            model,
            provider_url,
            api_key,
        }));
    }

    if inline_model.is_some() || inline_url.is_some() || inline_key.is_some() {
        let model = if let Some(m) = inline_model {
            ModelSpec::from_str(m)
                .map_err(|e| IronclawError::Config(format!("invalid inline model spec: {e}")))?
        } else {
            main_model.clone()
        };

        let provider_url = inline_url.unwrap_or(main_url).to_string();
        let api_key = inline_key.or(main_key).map(str::to_owned);

        return Ok(Some(ProviderSpec {
            name: role_label.to_owned(),
            model,
            provider_url,
            api_key,
        }));
    }

    Ok(None)
}

/// Get the provider-specific API key from environment variables.
fn provider_api_key_env(kind: ProviderKind) -> Option<String> {
    match kind {
        ProviderKind::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
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
        // Test default provider URL mapping
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
    }

    #[test]
    fn config_toml_parsing() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-test"
workspace_dir = "/tmp/ironclaw-test"
timeout_secs = 90
max_tokens = 16384
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            file.model.as_deref(),
            Some("anthropic/claude-sonnet-4-5"),
            "model should parse"
        );
        assert_eq!(
            file.api_key.as_deref(),
            Some("sk-test"),
            "api_key should parse"
        );
        assert_eq!(file.timeout_secs, Some(90), "timeout should parse");
        assert_eq!(file.max_tokens, Some(16384), "max_tokens should parse");
    }

    #[test]
    fn config_toml_with_ollama() {
        let toml_str = r#"
model = "ollama/llama3"
provider_url = "http://localhost:11434"
timeout_secs = 60
max_tokens = 4096
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            file.model.as_deref(),
            Some("ollama/llama3"),
            "model should parse"
        );
        assert_eq!(
            file.provider_url.as_deref(),
            Some("http://localhost:11434"),
            "provider_url should parse"
        );
        assert_eq!(file.timeout_secs, Some(60), "timeout should parse");
        assert_eq!(file.max_tokens, Some(4096), "max_tokens should parse");
    }

    #[test]
    fn config_toml_empty() {
        let toml_str = "";
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(file.model.is_none(), "empty toml should have no model");
        assert!(file.api_key.is_none(), "empty toml should have no api_key");
        assert!(
            file.timeout_secs.is_none(),
            "empty toml should have no timeout"
        );
    }

    #[test]
    fn config_toml_with_memory_section() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-test"

[memory]
observer_model = "anthropic/claude-haiku-3-5"
observer_threshold_tokens = 20000
reflector_threshold_tokens = 50000
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let mem = file.memory.as_ref();
        assert!(mem.is_some(), "memory section should parse");

        let mem = mem.unwrap();
        assert_eq!(
            mem.observer_model.as_deref(),
            Some("anthropic/claude-haiku-3-5"),
            "observer model should parse"
        );
        assert_eq!(
            mem.observer_threshold_tokens,
            Some(20000),
            "observer threshold should parse"
        );
        assert_eq!(
            mem.reflector_threshold_tokens,
            Some(50000),
            "reflector threshold should parse"
        );
    }

    #[test]
    fn memory_config_defaults_when_absent() {
        let cfg = MemoryConfig::from_file_and_env(None);
        assert!(
            cfg.observer_model.is_none(),
            "default observer model should be None"
        );
        assert_eq!(
            cfg.observer_threshold_tokens, DEFAULT_OBSERVER_THRESHOLD,
            "default observer threshold"
        );
        assert_eq!(
            cfg.reflector_threshold_tokens, DEFAULT_REFLECTOR_THRESHOLD,
            "default reflector threshold"
        );
    }

    #[test]
    fn memory_config_from_file() {
        let section = MemoryConfigFile {
            observer_model: Some("anthropic/claude-haiku-3-5".to_string()),
            observer_provider_url: None,
            observer_api_key: Some("sk-observer".to_string()),
            observer_threshold_tokens: Some(15000),
            reflector_model: None,
            reflector_provider_url: None,
            reflector_api_key: None,
            reflector_threshold_tokens: Some(45000),
        };
        let cfg = MemoryConfig::from_file_and_env(Some(&section));

        assert!(cfg.observer_model.is_some(), "observer model should be set");
        assert_eq!(
            cfg.observer_model.as_ref().map(|m| m.model.as_str()),
            Some("claude-haiku-3-5"),
            "observer model name should match"
        );
        assert_eq!(
            cfg.observer_api_key.as_deref(),
            Some("sk-observer"),
            "observer api key should match"
        );
        assert_eq!(
            cfg.observer_threshold_tokens, 15000,
            "observer threshold should match"
        );
        assert_eq!(
            cfg.reflector_threshold_tokens, 45000,
            "reflector threshold should match"
        );
    }

    #[test]
    fn config_toml_without_memory_section() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(
            file.memory.is_none(),
            "missing memory section should be None"
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
        let contents = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            contents.contains("model"),
            "config.toml should contain model key"
        );
    }

    #[test]
    fn bootstrap_skips_existing_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "# user customization").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let contents = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            contents, "# user customization",
            "existing config.toml should not be overwritten"
        );
    }

    #[test]
    fn bootstrap_always_writes_example() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let contents = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(
            contents, "# old content",
            "config.example.toml should be regenerated"
        );
        assert!(
            contents.contains("[memory]"),
            "example should contain memory section"
        );
    }

    #[test]
    fn bootstrap_example_contains_all_sections() {
        let dir = tempdir().unwrap();
        bootstrap_at(dir.path()).unwrap();
        let contents = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
        assert!(
            contents.contains("[providers"),
            "example should document providers table"
        );
        assert!(
            contents.contains("[roles]"),
            "example should document roles table"
        );
        assert!(
            contents.contains("[memory]"),
            "example should contain memory section"
        );
        assert!(
            contents.contains("[pulse]"),
            "example should contain pulse section"
        );
        assert!(
            contents.contains("[cron]"),
            "example should contain cron section"
        );
        assert!(
            contents.contains("[gateway]"),
            "example should contain gateway section"
        );
    }

    // ── Provider / role resolution tests ────────────────────────────────────

    #[test]
    fn named_providers_no_roles_ignored() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-main"

[providers.fast]
model = "anthropic/claude-haiku-4-5"
api_key = "sk-fast"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
        assert_eq!(
            cfg.model.model, "claude-sonnet-4-5",
            "providers table alone should not change main model"
        );
    }

    #[test]
    fn roles_override_agent_provider() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-main"

[providers.fast]
model = "anthropic/claude-haiku-4-5"
api_key = "sk-fast"

[roles]
agent = "fast"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
        assert_eq!(
            cfg.model.model, "claude-haiku-4-5",
            "roles.agent should override main model"
        );
        assert_eq!(
            cfg.api_key.as_deref(),
            Some("sk-fast"),
            "roles.agent should override main api_key"
        );
    }

    #[test]
    fn roles_assign_observer_to_named_provider() {
        let main_model = ModelSpec::from_str("anthropic/claude-sonnet-4-5").unwrap();
        let mut providers = HashMap::new();
        providers.insert(
            "fast".to_string(),
            ProviderEntryFile {
                model: Some("anthropic/claude-haiku-4-5".to_string()),
                provider_url: None,
                api_key: Some("sk-fast".to_string()),
            },
        );

        let spec = resolve_role_provider(
            Some("fast"),
            "observer",
            None,
            None,
            None,
            Some(&providers),
            &main_model,
            DEFAULT_ANTHROPIC_URL,
            Some("sk-main"),
        )
        .unwrap();

        assert!(
            spec.is_some(),
            "named provider should yield Some(ProviderSpec)"
        );
        let spec = spec.unwrap();
        assert_eq!(spec.name, "fast", "named provider key becomes the name");
        assert_eq!(
            spec.model.model, "claude-haiku-4-5",
            "named provider model should be used"
        );
        assert_eq!(
            spec.api_key.as_deref(),
            Some("sk-fast"),
            "named provider key should be used"
        );
    }

    #[test]
    fn pulse_provider_from_inline_section() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-main"

[pulse]
enabled = true
model = "anthropic/claude-haiku-4-5"
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
        assert!(
            cfg.pulse.provider.is_some(),
            "inline pulse model should yield Some provider"
        );
        let pulse_prov = cfg.pulse.provider.as_ref().unwrap();
        assert_eq!(
            pulse_prov.name, "pulse",
            "inline provider name should use the role label"
        );
        assert_eq!(
            pulse_prov.model.model, "claude-haiku-4-5",
            "pulse provider model should match"
        );
    }

    #[test]
    fn cron_provider_absent_is_none() {
        let toml_str = r#"
model = "anthropic/claude-sonnet-4-5"
api_key = "sk-main"

[cron]
enabled = true
"#;
        let file: ConfigFile = toml::from_str(toml_str).unwrap();
        let cfg = Config::from_file_and_env(Some(&file)).unwrap();
        assert!(
            cfg.cron.provider.is_none(),
            "empty [cron] section should yield None provider"
        );
    }

    #[test]
    fn reflector_separate_from_observer() {
        let section = MemoryConfigFile {
            observer_model: Some("anthropic/claude-haiku-3-5".to_string()),
            observer_provider_url: None,
            observer_api_key: Some("sk-observer".to_string()),
            observer_threshold_tokens: None,
            reflector_model: Some("anthropic/claude-sonnet-4-5".to_string()),
            reflector_provider_url: None,
            reflector_api_key: Some("sk-reflector".to_string()),
            reflector_threshold_tokens: None,
        };
        let cfg = MemoryConfig::from_file_and_env(Some(&section));
        assert_eq!(
            cfg.reflector_model.as_ref().map(|m| m.model.as_str()),
            Some("claude-sonnet-4-5"),
            "reflector model should be independently configured"
        );
        assert_eq!(
            cfg.reflector_api_key.as_deref(),
            Some("sk-reflector"),
            "reflector api key should be independently configured"
        );
    }

    #[test]
    fn reflector_fallback_to_observer() {
        let section = MemoryConfigFile {
            observer_model: Some("anthropic/claude-haiku-3-5".to_string()),
            observer_provider_url: None,
            observer_api_key: Some("sk-observer".to_string()),
            observer_threshold_tokens: None,
            reflector_model: None,
            reflector_provider_url: None,
            reflector_api_key: None,
            reflector_threshold_tokens: None,
        };
        let cfg = MemoryConfig::from_file_and_env(Some(&section));
        // MemoryConfig itself just stores None for missing reflector fields;
        // the gateway resolves the fallback chain at build time.
        assert!(
            cfg.reflector_model.is_none(),
            "absent reflector_model stored as None (gateway falls back to observer)"
        );
        assert!(
            cfg.observer_model.is_some(),
            "observer model still populated"
        );
    }

    #[test]
    fn reflector_fallback_chain_to_main() {
        let cfg = MemoryConfig::from_file_and_env(None);
        assert!(
            cfg.reflector_model.is_none(),
            "no reflector model → gateway falls back to main"
        );
        assert!(
            cfg.observer_model.is_none(),
            "no observer model → gateway falls back to main"
        );
    }
}

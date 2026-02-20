//! Configuration loading and validation.
//!
//! Uses a two-type pattern: `ConfigFile` (raw TOML deserialization) is validated
//! into `Config` (runtime-safe values).

use std::fmt;
use std::path::PathBuf;
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

/// Default max tokens for model responses.
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Default observer token threshold before firing.
pub(crate) const DEFAULT_OBSERVER_THRESHOLD: usize = 30_000;

/// Default reflector token threshold before compressing.
pub(crate) const DEFAULT_REFLECTOR_THRESHOLD: usize = 40_000;

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
            .finish()
    }
}

/// Validated pulse subsystem configuration.
#[derive(Debug, Clone)]
pub struct PulseConfig {
    /// Whether the pulse system is enabled.
    pub enabled: bool,
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl PulseConfig {
    /// Build from the raw TOML section.
    fn from_file(section: Option<&PulseConfigFile>) -> Self {
        Self {
            enabled: section.and_then(|s| s.enabled).unwrap_or(true),
        }
    }
}

/// Validated cron subsystem configuration.
#[derive(Debug, Clone)]
pub struct CronConfig {
    /// Whether the cron system is enabled.
    pub enabled: bool,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl CronConfig {
    /// Build from the raw TOML section.
    fn from_file(section: Option<&CronConfigFile>) -> Self {
        Self {
            enabled: section.and_then(|s| s.enabled).unwrap_or(true),
        }
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

        let reflector_threshold_tokens = section
            .and_then(|s| s.reflector_threshold_tokens)
            .unwrap_or(DEFAULT_REFLECTOR_THRESHOLD);

        Self {
            observer_model,
            observer_provider_url,
            observer_api_key,
            observer_threshold_tokens,
            reflector_threshold_tokens,
        }
    }
}

impl Config {
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
    fn from_file_and_env(file: Option<&ConfigFile>) -> Result<Self, IronclawError> {
        // Model: env > file > default
        let model_str = std::env::var("IRONCLAW_MODEL")
            .ok()
            .or_else(|| file.and_then(|f| f.model.clone()))
            .unwrap_or_else(|| "anthropic/claude-sonnet-4-5".to_string());

        let model = ModelSpec::from_str(&model_str)
            .map_err(|e| IronclawError::Config(format!("invalid model spec: {e}")))?;

        // Provider URL: env > file > default per provider
        let provider_url = std::env::var("IRONCLAW_PROVIDER_URL")
            .ok()
            .or_else(|| file.and_then(|f| f.provider_url.clone()))
            .unwrap_or_else(|| model.kind.default_url().to_string());

        // API key: provider-specific env > generic env > file
        let api_key = provider_api_key_env(model.kind)
            .or_else(|| std::env::var("IRONCLAW_API_KEY").ok())
            .or_else(|| file.and_then(|f| f.api_key.clone()));

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

        let memory = MemoryConfig::from_file_and_env(file.and_then(|f| f.memory.as_ref()));
        let pulse = PulseConfig::from_file(file.and_then(|f| f.pulse.as_ref()));
        let cron = CronConfig::from_file(file.and_then(|f| f.cron.as_ref()));

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
    /// Memory subsystem configuration.
    memory: Option<MemoryConfigFile>,
    /// Pulse subsystem configuration.
    pulse: Option<PulseConfigFile>,
    /// Cron subsystem configuration.
    cron: Option<CronConfigFile>,
}

/// Raw TOML `[pulse]` section.
#[derive(Debug, Deserialize)]
struct PulseConfigFile {
    /// Whether the pulse system is enabled.
    enabled: Option<bool>,
}

/// Raw TOML `[cron]` section.
#[derive(Debug, Deserialize)]
struct CronConfigFile {
    /// Whether the cron system is enabled.
    enabled: Option<bool>,
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
    /// Token threshold before the reflector compresses.
    reflector_threshold_tokens: Option<usize>,
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

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
}

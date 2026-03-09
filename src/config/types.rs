//! Validated runtime configuration structs for each subsystem.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::models::retry::RetryConfig;

use super::constants::{
    DEFAULT_AGENT_MODIFY_CHANNELS, DEFAULT_AGENT_MODIFY_MCP, DEFAULT_GATEWAY_BIND,
    DEFAULT_GATEWAY_PORT, DEFAULT_IDLE_TIMEOUT_MINUTES, DEFAULT_MAX_CONCURRENT_BACKGROUND,
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD, DEFAULT_SEARCH_CANDIDATE_MULTIPLIER, DEFAULT_SEARCH_MIN_SCORE,
    DEFAULT_SEARCH_TEMPORAL_DECAY, DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS,
    DEFAULT_SEARCH_TEXT_WEIGHT, DEFAULT_SEARCH_VECTOR_WEIGHT, DEFAULT_TRANSCRIPT_RETENTION_DAYS,
};
use super::provider::ProviderSpec;

/// Validated gateway configuration.
#[derive(Debug, Clone, PartialEq)]
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
    /// The full socket address string (e.g. `"127.0.0.1:7700"`).
    #[must_use]
    pub fn addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}

/// Validated memory subsystem configuration (thresholds only).
///
/// Provider assignments for observer/reflector are on `Config` directly.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryConfig {
    /// Token threshold before the observer fires.
    pub observer_threshold_tokens: usize,
    /// Token threshold before the reflector compresses.
    pub reflector_threshold_tokens: usize,
    /// Cooldown period in seconds after the soft threshold is crossed.
    pub observer_cooldown_secs: u64,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    pub observer_force_threshold_tokens: usize,
    /// Hybrid search configuration.
    pub search: SearchConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            observer_threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            reflector_threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
            observer_cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            observer_force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
            search: SearchConfig::default(),
        }
    }
}

/// Validated hybrid search configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchConfig {
    /// Weight for vector similarity scores in hybrid merge (0.0–1.0).
    pub vector_weight: f64,
    /// Weight for BM25 text scores in hybrid merge (0.0–1.0).
    pub text_weight: f64,
    /// Minimum hybrid score threshold for results.
    pub min_score: f64,
    /// Multiplier on limit for candidate retrieval before merge.
    pub candidate_multiplier: usize,
    /// Whether temporal decay is enabled (default: false).
    pub temporal_decay: bool,
    /// Half-life in days for temporal decay (default: 30.0).
    pub temporal_decay_half_life_days: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: DEFAULT_SEARCH_VECTOR_WEIGHT,
            text_weight: DEFAULT_SEARCH_TEXT_WEIGHT,
            min_score: DEFAULT_SEARCH_MIN_SCORE,
            candidate_multiplier: DEFAULT_SEARCH_CANDIDATE_MULTIPLIER,
            temporal_decay: DEFAULT_SEARCH_TEMPORAL_DECAY,
            temporal_decay_half_life_days: DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS,
        }
    }
}

/// Validated cloud tunnel configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudConfig {
    /// Relay server WebSocket URL.
    pub relay_url: String,
    /// Authentication token.
    pub token: String,
    /// Local port to forward requests to.
    pub local_port: u16,
}

/// Validated Discord bot configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscordConfig {
    /// Bot token for the Discord API.
    pub token: String,
}

/// Validated Telegram bot configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct TelegramConfig {
    /// Bot token for the Telegram API.
    pub token: String,
}

/// Validated webhook endpoint configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebhookConfig {
    /// Whether the webhook endpoint is enabled.
    pub enabled: bool,
    /// Optional bearer token for authenticating incoming requests.
    pub secret: Option<String>,
}

/// Validated skills subsystem configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsConfig {
    /// Directories to scan for skills (resolved, expanded paths).
    pub dirs: Vec<PathBuf>,
}

/// Validated agent ability gates.
///
/// Controls what the agent is allowed to modify at runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentAbilitiesConfig {
    /// Whether the agent can add/remove MCP servers.
    pub modify_mcp: bool,
    /// Whether the agent can add/remove notification channels.
    pub modify_channels: bool,
}

impl Default for AgentAbilitiesConfig {
    fn default() -> Self {
        Self {
            modify_mcp: DEFAULT_AGENT_MODIFY_MCP,
            modify_channels: DEFAULT_AGENT_MODIFY_CHANNELS,
        }
    }
}

/// Validated idle system configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct IdleConfig {
    /// Inactivity timeout. `Duration::ZERO` means disabled.
    pub timeout: Duration,
    /// Interface to switch to when idle. `None` = keep current. (Phase 2)
    pub idle_channel: Option<String>,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_MINUTES * 60),
            idle_channel: None,
        }
    }
}

/// Validated background task configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundConfig {
    /// Maximum number of concurrent background tasks.
    pub max_concurrent: usize,
    /// Number of days to retain background task transcripts.
    pub transcript_retention_days: u64,
    /// Model tier assignments for background tasks.
    pub models: BackgroundModelsConfig,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_BACKGROUND,
            transcript_retention_days: DEFAULT_TRANSCRIPT_RETENTION_DAYS,
            models: BackgroundModelsConfig::default(),
        }
    }
}

/// Model tier assignments for background tasks.
///
/// Each tier can be explicitly assigned a model chain (failover). Unset tiers
/// fall back to the next tier up, ultimately falling back to main.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackgroundModelsConfig {
    /// Small/fast model chain for simple tasks.
    pub small: Option<Vec<ProviderSpec>>,
    /// Medium model chain for typical tasks (default tier).
    pub medium: Option<Vec<ProviderSpec>>,
    /// Large model chain for complex tasks.
    pub large: Option<Vec<ProviderSpec>>,
}

impl BackgroundModelsConfig {
    /// Resolve a specific tier to a concrete provider chain.
    ///
    /// Fallback chain: tier → next tier up → main.
    #[must_use]
    pub fn resolve_tier(
        &self,
        tier: &BackgroundModelTier,
        main: &[ProviderSpec],
    ) -> Vec<ProviderSpec> {
        match tier {
            BackgroundModelTier::Small => self
                .small
                .as_ref()
                .or(self.medium.as_ref())
                .or(self.large.as_ref())
                .cloned()
                .unwrap_or_else(|| main.to_vec()),
            BackgroundModelTier::Medium => self
                .medium
                .as_ref()
                .or(self.large.as_ref())
                .cloned()
                .unwrap_or_else(|| main.to_vec()),
            BackgroundModelTier::Large => self.large.clone().unwrap_or_else(|| main.to_vec()),
        }
    }
}

/// Which model tier a background task requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundModelTier {
    /// Small/fast model for simple tasks.
    Small,
    /// Medium model for typical tasks.
    #[default]
    Medium,
    /// Large model for complex tasks.
    Large,
}

impl std::fmt::Display for BackgroundModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Small => f.write_str("small"),
            Self::Medium => f.write_str("medium"),
            Self::Large => f.write_str("large"),
        }
    }
}

impl std::str::FromStr for BackgroundModelTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            other => Err(format!(
                "invalid model tier '{other}': must be small, medium, or large"
            )),
        }
    }
}

/// Per-role overrides for temperature and thinking.
///
/// When set, these override the global `temperature` / `thinking` values
/// from `config.toml` for a specific role.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleOverrides {
    /// Sampling temperature override (0.0–2.0).
    pub temperature: Option<f32>,
    /// Thinking/reasoning configuration override.
    pub thinking: Option<crate::models::ThinkingConfig>,
}

/// Validated web search configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebSearchConfig {
    /// Provider-native search config (set when main provider supports it).
    pub provider_native: Option<ProviderNativeSearchConfig>,
    /// Standalone backend name for MCP resolution (e.g. `"brave"`, `"tavily"`, `"ollama"`).
    pub standalone_backend: Option<StandaloneBackendConfig>,
}

/// Provider-native search overrides (Anthropic, `OpenAI`, Gemini).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderNativeSearchConfig {
    /// Maximum web search invocations per request (Anthropic).
    pub max_uses: Option<u32>,
    /// Restrict search to these domains (Anthropic).
    pub allowed_domains: Option<Vec<String>>,
    /// Exclude these domains from search (Anthropic).
    pub blocked_domains: Option<Vec<String>>,
    /// Search context size (`OpenAI`: `"low"`, `"medium"`, `"high"`).
    pub search_context_size: Option<String>,
    /// Domains to exclude from Google Search grounding (Gemini).
    pub exclude_domains: Option<Vec<String>>,
}

/// Standalone web search backend configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct StandaloneBackendConfig {
    /// Backend name (`"brave"`, `"tavily"`, or `"ollama"`).
    pub name: String,
    /// API key for the backend.
    pub api_key: String,
    /// Base URL (only used by Ollama Cloud).
    pub base_url: Option<String>,
}

/// Validated runtime configuration.
///
/// All provider roles are fully resolved at load time. Consumers read fields
/// directly — no fallback chains needed.
#[derive(Clone, PartialEq)]
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
    /// Cloud tunnel configuration (None if `[cloud]` section absent or disabled).
    pub cloud: Option<CloudConfig>,
    /// Discord bot configuration (None if `[discord]` section absent or no token).
    pub discord: Option<DiscordConfig>,
    /// Telegram bot configuration (None if `[telegram]` section absent or no token).
    pub telegram: Option<TelegramConfig>,
    /// Webhook endpoint configuration.
    pub webhook: WebhookConfig,
    /// Skills subsystem configuration.
    pub skills: SkillsConfig,
    /// Retry configuration for model provider calls.
    pub retry: RetryConfig,
    /// Background task configuration.
    pub background: BackgroundConfig,
    /// Agent ability gates.
    pub agent: AgentAbilitiesConfig,
    /// Idle system configuration.
    pub idle: IdleConfig,
    /// Sampling temperature for model completions.
    pub temperature: Option<f32>,
    /// Thinking/reasoning configuration for model completions.
    pub thinking: Option<crate::models::ThinkingConfig>,
    /// Web search configuration.
    pub web_search: WebSearchConfig,
    /// Per-role overrides for temperature and thinking.
    pub role_overrides: HashMap<String, RoleOverrides>,
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
            .field("cloud", &self.cloud.as_ref().map(|_| "[configured]"))
            .field("discord", &self.discord.as_ref().map(|_| "[configured]"))
            .field("telegram", &self.telegram.as_ref().map(|_| "[configured]"))
            .field("webhook", &self.webhook)
            .field("skills", &self.skills)
            .field("retry", &self.retry)
            .field("background", &self.background)
            .field("agent", &self.agent)
            .field("idle", &self.idle)
            .field("temperature", &self.temperature)
            .field("thinking", &self.thinking)
            .field("web_search", &self.web_search)
            .field("role_overrides", &self.role_overrides)
            .field("config_dir", &self.config_dir)
            .finish()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn from_str_valid_tiers() {
        assert_eq!(
            "small".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Small
        );
        assert_eq!(
            "medium".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Medium
        );
        assert_eq!(
            "large".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Large
        );
    }

    #[test]
    fn from_str_invalid() {
        assert!("invalid".parse::<BackgroundModelTier>().is_err());
        assert!("SMALL".parse::<BackgroundModelTier>().is_err());
        assert!("".parse::<BackgroundModelTier>().is_err());
    }

    #[test]
    fn display_round_trips() {
        for tier in [
            BackgroundModelTier::Small,
            BackgroundModelTier::Medium,
            BackgroundModelTier::Large,
        ] {
            let s = tier.to_string();
            assert_eq!(s.parse::<BackgroundModelTier>().unwrap(), tier);
        }
    }

    #[test]
    fn resolve_tier_tests() {
        use super::super::provider::{ModelSpec, ProviderKind};

        let p_small = ProviderSpec {
            name: "dummy-small".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "small-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
        };
        let p_medium = ProviderSpec {
            name: "dummy-medium".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "medium-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
        };
        let p_large = ProviderSpec {
            name: "dummy-large".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "large-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
        };
        let p_main = ProviderSpec {
            name: "dummy-main".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "main-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
        };

        let main_slice = std::slice::from_ref(&p_main);

        // All present -> resolves to specific tier
        let config_full = BackgroundModelsConfig {
            small: Some(vec![p_small.clone()]),
            medium: Some(vec![p_medium.clone()]),
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_small.clone()]
        );
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_medium.clone()]
        );
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_large.clone()]
        );

        // Missing small -> small falls back to medium
        let config_no_small = BackgroundModelsConfig {
            small: None,
            medium: Some(vec![p_medium.clone()]),
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_no_small.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_medium.clone()]
        );

        // Missing small and medium -> small and medium fall back to large
        let config_only_large = BackgroundModelsConfig {
            small: None,
            medium: None,
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_only_large.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_large.clone()]
        );
        assert_eq!(
            config_only_large.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_large.clone()]
        );

        // Empty config -> all fall back to main
        let config_empty = BackgroundModelsConfig::default();
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_main.clone()]
        );
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_main.clone()]
        );
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_main.clone()]
        );
    }
}

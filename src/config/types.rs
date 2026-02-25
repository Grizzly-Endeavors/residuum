//! Validated runtime configuration structs for each subsystem.

use std::path::PathBuf;

use super::constants::{
    DEFAULT_GATEWAY_BIND, DEFAULT_GATEWAY_PORT, DEFAULT_MAX_CONCURRENT_BACKGROUND,
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD, DEFAULT_SEARCH_CANDIDATE_MULTIPLIER, DEFAULT_SEARCH_MIN_SCORE,
    DEFAULT_SEARCH_TEMPORAL_DECAY, DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS,
    DEFAULT_SEARCH_TEXT_WEIGHT, DEFAULT_SEARCH_VECTOR_WEIGHT, DEFAULT_TRANSCRIPT_RETENTION_DAYS,
};
use super::provider::ProviderSpec;

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
#[derive(Debug, Clone)]
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

/// Validated MCP server configuration.
#[derive(Debug, Clone, Default)]
pub struct McpConfig {
    /// Global MCP servers to start on gateway boot.
    pub servers: Vec<crate::projects::types::McpServerEntry>,
}

/// Validated notification channel configuration.
#[derive(Debug, Clone, Default)]
pub struct NotificationsConfig {
    /// External channel definitions resolved from config.
    pub channels: Vec<ExternalChannelConfig>,
}

/// A single resolved external channel configuration.
#[derive(Debug, Clone)]
pub struct ExternalChannelConfig {
    /// Channel name (key from `[notifications.channels.<name>]`).
    pub name: String,
    /// Channel type and type-specific settings.
    pub kind: ExternalChannelKind,
}

/// Channel type with type-specific configuration.
#[derive(Debug, Clone)]
pub enum ExternalChannelKind {
    /// Ntfy push notification channel.
    Ntfy {
        /// Ntfy server URL.
        url: String,
        /// Topic to publish to.
        topic: String,
        /// Message priority (default: `"default"`).
        priority: Option<String>,
    },
    /// Webhook HTTP channel.
    Webhook {
        /// Endpoint URL.
        url: String,
        /// HTTP method (default: `"POST"`).
        method: Option<String>,
        /// Additional headers.
        headers: Vec<(String, String)>,
    },
}

/// Validated background task configuration.
#[derive(Debug, Clone)]
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
/// Each tier can be explicitly assigned a model. Unset tiers fall back
/// to the next tier up, ultimately falling back to main.
#[derive(Debug, Clone, Default)]
pub struct BackgroundModelsConfig {
    /// Small/fast model for simple tasks.
    pub small: Option<ProviderSpec>,
    /// Medium model for typical tasks (default tier).
    pub medium: Option<ProviderSpec>,
    /// Large model for complex tasks.
    pub large: Option<ProviderSpec>,
}

impl BackgroundModelsConfig {
    /// Resolve a specific tier to a concrete `ProviderSpec`.
    ///
    /// Fallback chain: tier → next tier up → main.
    #[must_use]
    pub fn resolve_tier(&self, tier: &BackgroundModelTier, main: &ProviderSpec) -> ProviderSpec {
        match tier {
            BackgroundModelTier::Small => self
                .small
                .clone()
                .or_else(|| self.medium.clone())
                .or_else(|| self.large.clone())
                .unwrap_or_else(|| main.clone()),
            BackgroundModelTier::Medium => self
                .medium
                .clone()
                .or_else(|| self.large.clone())
                .unwrap_or_else(|| main.clone()),
            BackgroundModelTier::Large => self.large.clone().unwrap_or_else(|| main.clone()),
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

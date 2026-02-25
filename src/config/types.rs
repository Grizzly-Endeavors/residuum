//! Validated runtime configuration structs for each subsystem.

use std::path::PathBuf;

use super::constants::{
    DEFAULT_GATEWAY_BIND, DEFAULT_GATEWAY_PORT, DEFAULT_OBSERVER_COOLDOWN_SECS,
    DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    DEFAULT_SEARCH_CANDIDATE_MULTIPLIER, DEFAULT_SEARCH_MIN_SCORE, DEFAULT_SEARCH_TEMPORAL_DECAY,
    DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS, DEFAULT_SEARCH_TEXT_WEIGHT,
    DEFAULT_SEARCH_VECTOR_WEIGHT,
};

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

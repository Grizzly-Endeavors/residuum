//! Validated runtime configuration structs for each subsystem.

use std::path::PathBuf;

use super::constants::{
    DEFAULT_GATEWAY_BIND, DEFAULT_GATEWAY_PORT, DEFAULT_OBSERVER_COOLDOWN_SECS,
    DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
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

/// Validated MCP server configuration.
#[derive(Debug, Clone, Default)]
pub struct McpConfig {
    /// Global MCP servers to start on gateway boot.
    pub servers: Vec<crate::projects::types::McpServerEntry>,
}

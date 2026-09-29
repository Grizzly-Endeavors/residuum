//! Validated hub-level configuration: the settings that belong to the
//! process rather than to any one agent, loaded from `hub/config.toml`.

use std::path::PathBuf;

use super::constants::{
    DEFAULT_HOP_HARD_LIMIT, DEFAULT_HOP_SOFT_LIMIT, DEFAULT_MAX_CONCURRENT_BACKGROUND,
};
use super::types::{CloudConfig, GatewayConfig, TracingConfig};

/// The A2A settings that are hub-level: who the process listens as. Per-agent
/// visibility (`[a2a] visibility`) lives on the agent's own config instead —
/// see [`crate::config::A2aConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubA2aConfig {
    /// Whether the A2A listener runs at all.
    pub enabled: bool,
    /// Port for the dedicated A2A protocol listener (bound on the gateway's address).
    pub port: u16,
    /// Public URL other agents should use to reach this instance's A2A
    /// interfaces, when this instance runs its own tunnel/reverse proxy
    /// rather than relying on the relay's origin.
    pub public_url: Option<String>,
}

impl Default for HubA2aConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: super::constants::DEFAULT_A2A_PORT,
            public_url: None,
        }
    }
}

/// The background-task settings that are hub-level: the shared session
/// budget and the hop limits a message chain is checked against as it
/// crosses agents. Per-agent knobs (idle timeouts, the subagent depth cap,
/// model tiers) live on the agent's own config instead — see
/// [`crate::config::BackgroundConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubBackgroundConfig {
    /// Maximum number of concurrent running session turns, shared by every
    /// agent's session runtime.
    pub max_concurrent: usize,
    /// Hop count at or above which a delivered agent message carries a note
    /// asking the receiver to reply only if a reply is actually needed.
    pub hop_soft_limit: u32,
    /// Hop count at or above which agent message delivery is refused
    /// outright, to bound message loops.
    pub hop_hard_limit: u32,
}

impl Default for HubBackgroundConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_BACKGROUND,
            hop_soft_limit: DEFAULT_HOP_SOFT_LIMIT,
            hop_hard_limit: DEFAULT_HOP_HARD_LIMIT,
        }
    }
}

/// Validated hub-level runtime configuration, loaded from `hub/config.toml`.
///
/// Shared by every agent the hub hosts. It hot-reloads independently of the
/// agent's own config and keeps its own last-known-good fallback.
#[derive(Debug, Clone, PartialEq)]
pub struct HubConfig {
    /// IANA timezone for the whole hub (e.g. `America/New_York`), shared by
    /// every agent.
    pub timezone: chrono_tz::Tz,
    /// WebSocket gateway configuration.
    pub gateway: GatewayConfig,
    /// Cloud tunnel configuration (None if `[cloud]` section absent or disabled).
    pub cloud: Option<CloudConfig>,
    /// `Agent2Agent` (A2A) listener configuration.
    pub a2a: HubA2aConfig,
    /// Tracing and observability configuration.
    pub tracing: TracingConfig,
    /// Shared background-task limits.
    pub background: HubBackgroundConfig,
    /// Directory this config was loaded from (`~/.residuum/hub`).
    pub config_dir: PathBuf,
    /// User-facing notices describing what was skipped or degraded while
    /// loading this config. Populated the same way as
    /// [`crate::config::Config::load_notices`].
    pub load_notices: Vec<String>,
}

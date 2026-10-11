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

/// The Web Push settings that are hub-level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubPushConfig {
    /// Contact URI (`mailto:` or `https:`) that push services can reach the
    /// operator at, sent as the VAPID `sub` claim. `None` when unset or
    /// invalid, in which case the sender uses the project's default.
    pub contact: Option<String>,
}

/// Which service hosts the System 1 (decision model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOneProvider {
    /// `TypeSafe`'s hosted Jev models.
    TypeSafe,
    /// A local Ollama (0.35+) serving decision models.
    Ollama,
    /// Any other endpoint serving the `/v1/systemone` API.
    Other,
}

impl SystemOneProvider {
    /// The name shown to the user in messages.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::TypeSafe => "TypeSafe",
            Self::Ollama => "Ollama",
            Self::Other => "the decision model service",
        }
    }

    /// The base URL used when the config names none.
    #[must_use]
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            Self::TypeSafe => Some(super::constants::DEFAULT_TYPESAFE_URL),
            Self::Ollama => Some(super::constants::DEFAULT_OLLAMA_URL),
            Self::Other => None,
        }
    }

    /// The model used when the config names none.
    #[must_use]
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::TypeSafe => Some(super::constants::DEFAULT_TYPESAFE_MODEL),
            Self::Ollama | Self::Other => None,
        }
    }
}

impl std::str::FromStr for SystemOneProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "typesafe" => Ok(Self::TypeSafe),
            "ollama" => Ok(Self::Ollama),
            "other" => Ok(Self::Other),
            other => Err(format!(
                "unknown provider \"{other}\", expected typesafe, ollama, or other"
            )),
        }
    }
}

/// The resolved System 1 endpoint every agent shares.
#[derive(Clone, PartialEq, Eq)]
pub struct SystemOneConfig {
    pub provider: SystemOneProvider,
    /// Base URL, without the `/v1/...` path.
    pub url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub keep_alive: Option<String>,
}

impl SystemOneConfig {
    /// The name shown in messages: the provider's, or the custom URL's host.
    #[must_use]
    pub fn display_name(&self) -> String {
        if self.provider == SystemOneProvider::Other {
            url::Url::parse(&self.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| self.provider.display_name().to_string())
        } else {
            self.provider.display_name().to_string()
        }
    }
}

impl std::fmt::Debug for SystemOneConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemOneConfig")
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("keep_alive", &self.keep_alive)
            .finish()
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
    /// Web Push settings.
    pub push: HubPushConfig,
    /// The System 1 (decision model) endpoint. `None` when `[system_one]` is
    /// absent or too incomplete to call (a load notice says why).
    pub system_one: Option<SystemOneConfig>,
    /// Directory this config was loaded from (`~/.residuum/hub`).
    pub config_dir: PathBuf,
    /// User-facing notices describing what was skipped or degraded while
    /// loading this config. Populated the same way as
    /// [`crate::config::Config::load_notices`].
    pub load_notices: Vec<String>,
}

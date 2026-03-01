//! Raw TOML deserialization structs.
//!
//! These types map 1:1 to the config file sections. They are private to the
//! config module — callers always receive validated `Config` values.

use std::collections::HashMap;

use serde::Deserialize;

/// Raw TOML config file structure (deserialized directly).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigFile {
    /// User's display name (what the agent calls them).
    pub(super) name: Option<String>,
    /// IANA timezone name (e.g. `"America/New_York"`).
    pub(super) timezone: Option<String>,
    /// Named provider definitions.
    pub(super) providers: Option<HashMap<String, ProviderEntryFile>>,
    /// Role → model string assignments.
    pub(super) models: Option<ModelsConfigFile>,
    /// Workspace root directory path.
    pub(super) workspace_dir: Option<String>,
    /// Request timeout in seconds.
    pub(super) timeout_secs: Option<u64>,
    /// Maximum tokens for model responses.
    pub(super) max_tokens: Option<u32>,
    /// Memory subsystem configuration.
    pub(super) memory: Option<MemoryConfigFile>,
    /// Pulse subsystem configuration.
    pub(super) pulse: Option<PulseConfigFile>,
    /// Gateway configuration.
    pub(super) gateway: Option<GatewayConfigFile>,
    /// Discord bot configuration.
    pub(super) discord: Option<DiscordConfigFile>,
    /// Telegram bot configuration.
    pub(super) telegram: Option<TelegramConfigFile>,
    /// Webhook endpoint configuration.
    pub(super) webhook: Option<WebhookConfigFile>,
    /// A2A (Agent-to-Agent) protocol configuration.
    pub(super) a2a: Option<A2aConfigFile>,
    /// Skills subsystem configuration.
    pub(super) skills: Option<SkillsConfigFile>,
    /// MCP server configuration.
    pub(super) mcp: Option<McpConfigFile>,
    /// Retry configuration.
    pub(super) retry: Option<RetryConfigFile>,
    /// Notification channel configuration.
    pub(super) notifications: Option<NotificationsConfigFile>,
    /// Background task configuration.
    pub(super) background: Option<BackgroundConfigFile>,
}

/// A named provider entry under `[providers.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderEntryFile {
    /// Provider protocol type (e.g. `"openai"`, `"anthropic"`).
    #[serde(rename = "type")]
    pub(super) kind: String,
    /// API key.
    pub(super) api_key: Option<String>,
    /// Override base URL.
    pub(super) url: Option<String>,
}

/// A model string that can be either a single string or a list (for failover).
///
/// Accepts both `main = "anthropic/claude-sonnet-4-6"` and
/// `main = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum ModelStringOrList {
    /// Single model string.
    Single(String),
    /// Ordered list of model strings (failover chain).
    List(Vec<String>),
}

impl ModelStringOrList {
    /// Convert into a `Vec<String>` regardless of variant.
    #[must_use]
    pub(super) fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::List(v) => v,
        }
    }
}

/// Raw TOML `[models]` section — maps roles to `"provider/model"` strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelsConfigFile {
    /// Main agent model (required for operation). Supports failover arrays.
    pub(super) main: Option<ModelStringOrList>,
    /// Default fallback for unset roles. Supports failover arrays.
    pub(super) default: Option<ModelStringOrList>,
    /// Memory observer model. Supports failover arrays.
    pub(super) observer: Option<ModelStringOrList>,
    /// Memory reflector model. Supports failover arrays.
    pub(super) reflector: Option<ModelStringOrList>,
    /// Pulse agent model. Supports failover arrays.
    pub(super) pulse: Option<ModelStringOrList>,
    /// Embedding model (no failover — single string only).
    pub(super) embedding: Option<String>,
}

/// Raw TOML `[memory]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryConfigFile {
    /// Token threshold before the observer fires.
    pub(super) observer_threshold_tokens: Option<usize>,
    /// Token threshold before the reflector compresses.
    pub(super) reflector_threshold_tokens: Option<usize>,
    /// Cooldown period in seconds after the soft threshold is crossed.
    pub(super) observer_cooldown_secs: Option<u64>,
    /// Token threshold that forces immediate observation (bypasses cooldown).
    pub(super) observer_force_threshold_tokens: Option<usize>,
    /// Hybrid search tuning parameters.
    pub(super) search: Option<SearchConfigFile>,
}

/// Raw TOML `[memory.search]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchConfigFile {
    /// Weight for vector similarity scores in hybrid merge (0.0–1.0).
    pub(super) vector_weight: Option<f64>,
    /// Weight for BM25 text scores in hybrid merge (0.0–1.0).
    pub(super) text_weight: Option<f64>,
    /// Minimum hybrid score threshold for results.
    pub(super) min_score: Option<f64>,
    /// Multiplier on limit for candidate retrieval before merge.
    pub(super) candidate_multiplier: Option<usize>,
    /// Whether temporal decay is enabled for search scoring.
    pub(super) temporal_decay: Option<bool>,
    /// Half-life in days for temporal decay scoring.
    pub(super) temporal_decay_half_life_days: Option<f64>,
}

/// Raw TOML `[pulse]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PulseConfigFile {
    /// Whether the pulse system is enabled.
    pub(super) enabled: Option<bool>,
}

/// Raw TOML `[gateway]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayConfigFile {
    /// Address to bind the WebSocket server to.
    pub(super) bind: Option<String>,
    /// Port for the WebSocket server.
    pub(super) port: Option<u16>,
}

/// Raw TOML `[discord]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscordConfigFile {
    /// Bot token (supports `${ENV_VAR}` syntax).
    pub(super) token: Option<String>,
}

/// Raw TOML `[telegram]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TelegramConfigFile {
    /// Bot token (supports `${ENV_VAR}` syntax).
    pub(super) token: Option<String>,
}

/// Raw TOML `[webhook]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebhookConfigFile {
    /// Whether the webhook endpoint is enabled.
    pub(super) enabled: Option<bool>,
    /// Optional bearer token for authentication.
    pub(super) secret: Option<String>,
}

/// Raw TOML `[a2a]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct A2aConfigFile {
    /// Whether the A2A endpoint is enabled.
    pub(super) enabled: Option<bool>,
    /// Optional bearer token for authentication.
    pub(super) secret: Option<String>,
    /// Agent description for the Agent Card.
    pub(super) description: Option<String>,
}

/// Raw TOML `[skills]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillsConfigFile {
    /// Additional directories to scan for skills.
    pub(super) dirs: Option<Vec<String>>,
}

/// Raw TOML `[mcp]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct McpConfigFile {
    /// Named MCP server definitions.
    pub(super) servers: Option<HashMap<String, McpServerConfigEntry>>,
}

/// Raw TOML `[retry]` section.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryConfigFile {
    /// Maximum number of retry attempts (0 = no retries).
    pub(super) max_retries: Option<u32>,
    /// Initial delay before first retry in milliseconds.
    pub(super) initial_delay_ms: Option<u64>,
    /// Maximum delay between retries in milliseconds.
    pub(super) max_delay_ms: Option<u64>,
    /// Multiplier for exponential backoff.
    pub(super) backoff_multiplier: Option<f64>,
}

/// Raw TOML `[notifications]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NotificationsConfigFile {
    /// Named external channel definitions.
    pub(super) channels: Option<HashMap<String, ChannelConfigEntry>>,
}

/// A single channel entry under `[notifications.channels.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChannelConfigEntry {
    /// Channel type: `"ntfy"` or `"webhook"`.
    #[serde(rename = "type")]
    pub(super) kind: String,
    /// URL for the channel endpoint.
    pub(super) url: Option<String>,
    /// Ntfy topic name.
    pub(super) topic: Option<String>,
    /// Ntfy priority (default: `"default"`).
    pub(super) priority: Option<String>,
    /// HTTP method for webhooks (default: `"POST"`).
    pub(super) method: Option<String>,
    /// Additional HTTP headers for webhooks.
    pub(super) headers: Option<HashMap<String, String>>,
}

/// Raw TOML `[background]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackgroundConfigFile {
    /// Maximum number of concurrent background tasks.
    pub(super) max_concurrent: Option<usize>,
    /// Number of days to retain background task transcripts.
    pub(super) transcript_retention_days: Option<u64>,
    /// Model tier assignments for background tasks.
    pub(super) models: Option<BackgroundModelsFile>,
}

/// Raw TOML `[background.models]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackgroundModelsFile {
    /// Small/fast model for simple tasks. Supports failover arrays.
    pub(super) small: Option<ModelStringOrList>,
    /// Medium model for typical tasks (default tier). Supports failover arrays.
    pub(super) medium: Option<ModelStringOrList>,
    /// Large model for complex tasks. Supports failover arrays.
    pub(super) large: Option<ModelStringOrList>,
}

/// A single MCP server entry under `[mcp.servers.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct McpServerConfigEntry {
    /// Command (stdio) or URL (http) for the server.
    pub(super) command: String,
    /// Command-line arguments (only used for stdio transport).
    pub(super) args: Option<Vec<String>>,
    /// Environment variables to pass to the server process (only used for stdio transport).
    pub(super) env: Option<HashMap<String, String>>,
    /// Transport type: `"stdio"` (default) or `"http"`.
    pub(super) transport: Option<String>,
}

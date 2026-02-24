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
    /// Cron subsystem configuration.
    pub(super) cron: Option<CronConfigFile>,
    /// Gateway configuration.
    pub(super) gateway: Option<GatewayConfigFile>,
    /// Discord bot configuration.
    pub(super) discord: Option<DiscordConfigFile>,
    /// Webhook endpoint configuration.
    pub(super) webhook: Option<WebhookConfigFile>,
    /// Skills subsystem configuration.
    pub(super) skills: Option<SkillsConfigFile>,
    /// MCP server configuration.
    pub(super) mcp: Option<McpConfigFile>,
    /// Retry configuration.
    pub(super) retry: Option<RetryConfigFile>,
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

/// Raw TOML `[models]` section — maps roles to `"provider/model"` strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelsConfigFile {
    /// Main agent model (required for operation).
    pub(super) main: Option<String>,
    /// Default fallback for unset roles.
    pub(super) default: Option<String>,
    /// Memory observer model.
    pub(super) observer: Option<String>,
    /// Memory reflector model.
    pub(super) reflector: Option<String>,
    /// Pulse agent model.
    pub(super) pulse: Option<String>,
    /// Cron agent model.
    pub(super) cron: Option<String>,
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
}

/// Raw TOML `[pulse]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PulseConfigFile {
    /// Whether the pulse system is enabled.
    pub(super) enabled: Option<bool>,
}

/// Raw TOML `[cron]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CronConfigFile {
    /// Whether the cron system is enabled.
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

/// Raw TOML `[webhook]` section.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebhookConfigFile {
    /// Whether the webhook endpoint is enabled.
    pub(super) enabled: Option<bool>,
    /// Optional bearer token for authentication.
    pub(super) secret: Option<String>,
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

/// A single MCP server entry under `[mcp.servers.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct McpServerConfigEntry {
    /// Command to start the server.
    pub(super) command: String,
    /// Command-line arguments.
    pub(super) args: Option<Vec<String>>,
    /// Environment variables to pass to the server process.
    pub(super) env: Option<HashMap<String, String>>,
}

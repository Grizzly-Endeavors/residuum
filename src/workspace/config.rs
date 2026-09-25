//! Workspace config loaders: MCP servers and notification channels.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use anyhow::Context;

use crate::mcp::types::{McpServerEntry, McpTransport};
use crate::notify::types::{ExternalChannelConfig, ExternalChannelKind};

pub use super::channel_builder::build_external_channels;

// ── MCP loader ───────────────────────────────────────────────────────────────

/// Raw JSON structure for the MCP config file (Claude Code format).
#[derive(Deserialize)]
struct McpConfigFile {
    /// Map of server name → server definition.
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, McpServerRaw>,
}

/// Raw JSON server entry before conversion to `McpServerEntry`.
///
/// Supports multiple config formats:
/// - Residuum native: `transport` field with `"stdio"` or `"http"`
/// - Claude Code/Desktop: `type` field with `"stdio"`, `"streamable-http"`, or `"http"`
/// - `url` field alias for HTTP server address (falls back to `command`)
#[derive(Deserialize)]
struct McpServerRaw {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Claude Code/Desktop standard: `"stdio"`, `"streamable-http"`, `"http"`, or `"sse"`.
    #[serde(rename = "type")]
    type_: Option<String>,
    /// Residuum extension: `"stdio"` (default) or `"http"`.
    transport: Option<String>,
    /// HTTP server URL (alternative to putting the URL in `command`).
    url: Option<String>,
    /// HTTP headers to send with requests (only used for http transport).
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// Resolve one raw `mcp.json` server entry into a usable [`McpServerEntry`],
/// or the plain-language reason it can't be used.
///
/// Shared by [`load_mcp_servers_map`] (which drops the entry with a
/// `tracing::warn!` naming the reason) and
/// [`diagnose_mcp_json`] (which reports the same reason as a diagnostic), so
/// the two can never disagree about which entries load.
fn resolve_mcp_server(name: &str, raw: McpServerRaw) -> Result<McpServerEntry, String> {
    // Resolve transport: check `type` first (Claude standard), then `transport` (Residuum)
    let transport_str = raw.type_.as_deref().or(raw.transport.as_deref());
    let transport = match transport_str {
        Some("streamable-http" | "http") => McpTransport::Http,
        None | Some("stdio") => McpTransport::Stdio,
        Some("sse") => {
            return Err(format!(
                "server '{name}': SSE transport is deprecated by the MCP spec, server will not load"
            ));
        }
        Some(unknown) => {
            return Err(format!(
                "server '{name}': unrecognized transport '{unknown}', server will not load"
            ));
        }
    };

    // Resolve command/url based on transport
    let command = match transport {
        McpTransport::Http => {
            if let Some(url) = raw.url.filter(|u| !u.is_empty()) {
                url
            } else if let Some(cmd) = raw.command.filter(|c| !c.is_empty()) {
                cmd
            } else {
                return Err(format!(
                    "server '{name}': HTTP server has no url or command, server will not load"
                ));
            }
        }
        McpTransport::Stdio => {
            if let Some(cmd) = raw.command.filter(|c| !c.is_empty()) {
                cmd
            } else {
                return Err(format!(
                    "server '{name}': stdio server has no command, server will not load"
                ));
            }
        }
    };

    Ok(McpServerEntry {
        name: name.to_string(),
        command,
        args: raw.args,
        env: raw.env,
        transport,
        headers: raw.headers,
    })
}

/// Load MCP server definitions from a JSON file as a name → entry map.
///
/// Returns an empty map if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub fn load_mcp_servers_map(path: &Path) -> anyhow::Result<HashMap<String, McpServerEntry>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read mcp.json at {}", path.display()))?;

    let file: McpConfigFile = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse mcp.json at {}", path.display()))?;

    let servers: HashMap<String, McpServerEntry> = file
        .mcp_servers
        .into_iter()
        .filter_map(|(name, raw)| match resolve_mcp_server(&name, raw) {
            Ok(entry) => Some((name, entry)),
            Err(message) => {
                tracing::warn!(server = %name, %message, "skipping MCP server");
                None
            }
        })
        .collect();

    tracing::debug!(count = servers.len(), path = %path.display(), "loaded MCP servers");
    Ok(servers)
}

/// Diagnostics for `content` as `config/mcp.json`.
///
/// Reuses [`resolve_mcp_server`], the same per-entry check
/// [`load_mcp_servers_map`] applies, so a diagnostic can never disagree with
/// what loading skips. A JSON syntax error carries `serde_json`'s own
/// line/column.
#[must_use]
pub fn diagnose_mcp_json(content: &str) -> Vec<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{Diagnostic, Location};

    let file: McpConfigFile = match serde_json::from_str(content) {
        Ok(f) => f,
        Err(e) => {
            return vec![Diagnostic::error_at(
                e.to_string(),
                Location::LineColumn {
                    line: u32::try_from(e.line()).unwrap_or(u32::MAX),
                    column: u32::try_from(e.column()).unwrap_or(u32::MAX),
                },
            )];
        }
    };

    file.mcp_servers
        .into_iter()
        .filter_map(|(name, raw)| {
            resolve_mcp_server(&name, raw).err().map(|message| {
                Diagnostic::error_at(
                    message,
                    Location::Path {
                        path: format!("mcpServers.{name}"),
                    },
                )
            })
        })
        .collect()
}

/// Load MCP server definitions from a JSON file.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_mcp_servers(path: &Path) -> anyhow::Result<Vec<McpServerEntry>> {
    Ok(load_mcp_servers_map(path)?.into_values().collect())
}

// ── Channel loader ───────────────────────────────────────────────────────────

/// Raw TOML structure for the channels config file.
#[derive(Deserialize)]
struct ChannelsFile {
    #[serde(default)]
    channels: HashMap<String, ChannelEntryRaw>,
}

/// Raw TOML channel entry before conversion to `ExternalChannelConfig`.
#[derive(Deserialize)]
struct ChannelEntryRaw {
    /// Channel type: `"ntfy"`, `"webhook"`, `"macos"`, or `"windows"`.
    #[serde(rename = "type")]
    type_: String,
    url: Option<String>,
    topic: Option<String>,
    priority: Option<String>,
    method: Option<String>,
    headers: Option<HashMap<String, String>>,
    // macOS / Windows channel fields
    /// Retired. Retained so a stale config warns rather than being silently ignored.
    default_category: Option<String>,
    default_priority: Option<String>,
    throttle_window_secs: Option<u64>,
    sound: Option<bool>,
    app_name: Option<String>,
    web_url: Option<String>,
    // Windows-specific fields
    /// Retired. Retained so a stale config warns rather than being silently ignored.
    default_scenario: Option<String>,
    app_id: Option<String>,
}

/// Retired channel option keys present on `raw`, kept only so a stale config
/// warns rather than being silently ignored. The channel still loads with
/// these fields present — they just do nothing.
fn retired_channel_keys(raw: &ChannelEntryRaw) -> Vec<&'static str> {
    let mut keys = Vec::new();
    if raw.default_category.is_some() {
        keys.push("default_category");
    }
    if raw.default_scenario.is_some() {
        keys.push("default_scenario");
    }
    keys
}

/// Resolve one raw `channels.toml` entry's type-specific fields into an
/// [`ExternalChannelKind`], or the plain-language reason the channel can't
/// be used.
///
/// Shared by [`load_channel_configs`] (which drops the channel with a
/// `tracing::warn!` naming the reason) and [`diagnose_channels_toml`] (which
/// reports the same reason as a diagnostic), so the two can never disagree
/// about which channels load.
fn resolve_channel_kind(name: &str, raw: &ChannelEntryRaw) -> Result<ExternalChannelKind, String> {
    match raw.type_.as_str() {
        "ntfy" => {
            let url = raw.url.clone().filter(|u| !u.is_empty()).ok_or_else(|| {
                format!("channel '{name}': ntfy channel is missing required 'url' field")
            })?;
            let topic = raw.topic.clone().filter(|t| !t.is_empty()).ok_or_else(|| {
                format!("channel '{name}': ntfy channel is missing required 'topic' field")
            })?;
            Ok(ExternalChannelKind::Ntfy {
                url,
                topic,
                priority: raw.priority.clone(),
            })
        }
        "macos" => Ok(ExternalChannelKind::Macos {
            default_priority: raw.default_priority.clone(),
            throttle_window_secs: raw.throttle_window_secs,
            sound: raw.sound,
            app_name: raw.app_name.clone(),
            web_url: raw.web_url.clone(),
        }),
        "windows" => Ok(ExternalChannelKind::Windows {
            throttle_window_secs: raw.throttle_window_secs,
            sound: raw.sound,
            app_name: raw.app_name.clone(),
            app_id: raw.app_id.clone(),
        }),
        "webhook" => {
            // Keep in sync with the methods `WebhookChannel::deliver` actually
            // supports (src/notify/external.rs) — validate here, at config load,
            // so a typo surfaces at startup instead of at first delivery attempt.
            const SUPPORTED_METHODS: [&str; 2] = ["POST", "PUT"];

            let url = raw.url.clone().filter(|u| !u.is_empty()).ok_or_else(|| {
                format!("channel '{name}': webhook channel is missing required 'url' field")
            })?;
            if let Some(method) = &raw.method
                && !SUPPORTED_METHODS.contains(&method.to_uppercase().as_str())
            {
                return Err(format!(
                    "channel '{name}': webhook channel has unsupported 'method' field '{method}' \
                     (supported: {SUPPORTED_METHODS:?}), skipping channel"
                ));
            }
            Ok(ExternalChannelKind::Webhook {
                url,
                method: raw.method.clone(),
                headers: raw
                    .headers
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            })
        }
        unknown => Err(format!(
            "channel '{name}': unrecognized channel type '{unknown}', skipping"
        )),
    }
}

/// Load external channel configs from a TOML file.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub fn load_channel_configs(path: &Path) -> anyhow::Result<Vec<ExternalChannelConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read channels.toml at {}", path.display()))?;

    let file: ChannelsFile = toml::from_str(&contents)
        .with_context(|| format!("failed to parse channels.toml at {}", path.display()))?;

    let configs = file
        .channels
        .into_iter()
        .filter_map(|(name, raw)| {
            for key in retired_channel_keys(&raw) {
                tracing::warn!(
                    channel = %name,
                    key,
                    "ignoring retired channel option; notification categories were removed \
                     because nothing ever selected between them — delete this line"
                );
            }

            match resolve_channel_kind(&name, &raw) {
                Ok(kind) => Some(ExternalChannelConfig { name, kind }),
                Err(message) => {
                    tracing::warn!(%message, "skipping channel");
                    None
                }
            }
        })
        .collect();

    Ok(configs)
}

/// Diagnostics for `content` as `config/channels.toml`.
///
/// Reuses [`resolve_channel_kind`] and [`retired_channel_keys`], the same
/// per-entry checks [`load_channel_configs`] applies, so a diagnostic can
/// never disagree with what loading skips or ignores. A TOML syntax error
/// carries the parser's own line/column.
#[must_use]
pub fn diagnose_channels_toml(content: &str) -> Vec<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{Diagnostic, Location};

    let file: ChannelsFile = match toml::from_str(content) {
        Ok(f) => f,
        Err(e) => {
            let location = e
                .span()
                .map(|span| Location::from_byte_offset(content, span.start));
            return vec![match location {
                Some(loc) => Diagnostic::error_at(e.message().to_string(), loc),
                None => Diagnostic::error(e.message().to_string()),
            }];
        }
    };

    let mut diagnostics = Vec::new();
    for (name, raw) in &file.channels {
        for key in retired_channel_keys(raw) {
            diagnostics.push(Diagnostic::warning_at(
                format!(
                    "channel '{name}': '{key}' was retired and is ignored; notification \
                     categories were removed because nothing ever selected between them — \
                     delete this line"
                ),
                Location::Path {
                    path: format!("channels.{name}.{key}"),
                },
            ));
        }
        if let Err(message) = resolve_channel_kind(name, raw) {
            diagnostics.push(Diagnostic::error_at(
                message,
                Location::Path {
                    path: format!("channels.{name}"),
                },
            ));
        }
    }
    diagnostics
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    // ── MCP loader tests ─────────────────────────────────────────────────

    #[test]
    fn load_mcp_servers_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "filesystem": {
                        "command": "mcp-server-filesystem",
                        "args": ["/home/user"],
                        "env": { "DEBUG": "1" }
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s.name, "filesystem");
        assert_eq!(s.command, "mcp-server-filesystem");
        assert_eq!(s.args, vec!["/home/user"]);
        assert_eq!(s.env.get("DEBUG").map(String::as_str), Some("1"));
        assert_eq!(s.transport, McpTransport::Stdio);
    }

    #[test]
    fn load_mcp_servers_multiple_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "fs": { "command": "mcp-fs", "args": [] },
                    "git": { "command": "mcp-git", "args": ["--repo", "."] }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 2);
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fs"), "should have fs server");
        assert!(names.contains(&"git"), "should have git server");
    }

    #[test]
    fn load_mcp_servers_http_transport() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "remote": {
                        "command": "http://10.0.0.5:8080/mcp",
                        "transport": "http"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].transport, McpTransport::Http);
        assert_eq!(servers[0].command, "http://10.0.0.5:8080/mcp");
    }

    #[test]
    fn load_mcp_servers_claude_desktop_type_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "remote-api": {
                        "type": "streamable-http",
                        "url": "https://mcp.example.com/v1"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1, "should parse claude desktop style config");
        assert_eq!(servers[0].transport, McpTransport::Http);
        assert_eq!(servers[0].command, "https://mcp.example.com/v1");
    }

    #[test]
    fn load_mcp_servers_type_takes_priority_over_transport() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "conflict": {
                        "type": "http",
                        "transport": "stdio",
                        "url": "http://localhost:8080/mcp"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].transport,
            McpTransport::Http,
            "type field should take priority over transport"
        );
    }

    #[test]
    fn load_mcp_servers_url_field_preferred_over_command_for_http() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "remote": {
                        "transport": "http",
                        "url": "http://preferred.example.com/mcp",
                        "command": "http://fallback.example.com/mcp"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].command, "http://preferred.example.com/mcp",
            "url field should be preferred over command for http"
        );
    }

    #[test]
    fn load_mcp_servers_sse_transport_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "sse-server": {
                        "type": "sse",
                        "url": "http://sse.example.com/mcp"
                    },
                    "good-server": {
                        "command": "mcp-server"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1, "SSE server should be skipped");
        assert_eq!(servers[0].name, "good-server");
    }

    #[test]
    fn load_mcp_servers_http_missing_url_and_command_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "broken": {
                        "type": "http"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert!(
            servers.is_empty(),
            "HTTP server with no url or command should be skipped"
        );
    }

    #[test]
    fn load_mcp_servers_stdio_missing_command_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "broken-stdio": {
                        "type": "stdio"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert!(
            servers.is_empty(),
            "stdio server with no command should be skipped"
        );
    }

    #[test]
    fn load_mcp_servers_with_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "authed": {
                        "type": "http",
                        "url": "http://api.example.com/mcp",
                        "headers": {
                            "Authorization": "Bearer token123",
                            "X-Custom": "value"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].headers.len(), 2, "should preserve headers");
        assert_eq!(
            servers[0].headers.get("Authorization").map(String::as_str),
            Some("Bearer token123"),
            "should have auth header"
        );
    }

    #[test]
    fn load_mcp_servers_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{ "mcpServers": {} }"#).unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn load_mcp_servers_missing_file() {
        let path = Path::new("/tmp/nonexistent/mcp.json");
        let servers = load_mcp_servers(path).unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn load_mcp_servers_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "not valid json {{{").unwrap();

        let result = load_mcp_servers(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse mcp.json"), "got: {err}");
    }

    // ── Channel loader tests ─────────────────────────────────────────────

    #[test]
    fn load_channel_configs_ntfy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.my-ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "residuum"
priority = "high"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1);
        let c = &configs[0];
        assert_eq!(c.name, "my-ntfy");
        let ExternalChannelKind::Ntfy {
            url,
            topic,
            priority,
        } = &c.kind
        else {
            unreachable!("expected Ntfy kind");
        };
        assert_eq!(url, "https://ntfy.sh");
        assert_eq!(topic, "residuum");
        assert_eq!(priority.as_deref(), Some("high"));
    }

    #[test]
    fn load_channel_configs_webhook() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.ops-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
method = "PUT"

[channels.ops-hook.headers]
Authorization = "Bearer token123"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1);
        let c = &configs[0];
        assert_eq!(c.name, "ops-hook");
        let ExternalChannelKind::Webhook {
            url,
            method,
            headers,
        } = &c.kind
        else {
            unreachable!("expected Webhook kind");
        };
        assert_eq!(url, "https://hooks.example.com/notify");
        assert_eq!(method.as_deref(), Some("PUT"));
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer token123"),
            "should have auth header"
        );
    }

    #[test]
    fn load_channel_configs_mixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.ntfy-alerts]
type = "ntfy"
url = "https://ntfy.sh"
topic = "alerts"

[channels.slack-hook]
type = "webhook"
url = "https://hooks.slack.com/services/xxx"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 2);
        let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
        assert!(
            names.contains(&"ntfy-alerts"),
            "should have ntfy-alerts channel"
        );
        assert!(
            names.contains(&"slack-hook"),
            "should have slack-hook channel"
        );
    }

    #[test]
    fn load_channel_configs_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(&path, "").unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn load_channel_configs_missing_file() {
        let path = Path::new("/tmp/nonexistent/channels.toml");
        let configs = load_channel_configs(path).unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn load_channel_configs_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(&path, "not valid toml [[[").unwrap();

        let result = load_channel_configs(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse channels.toml"), "got: {err}");
    }

    // ── Channel error path tests ────────────────────────────────────────

    #[test]
    fn load_channel_configs_ntfy_missing_url_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.bad-ntfy]
type = "ntfy"
topic = "alerts"

[channels.good-ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "alerts"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "bad ntfy entry should be excluded");
        assert_eq!(configs[0].name, "good-ntfy");
    }

    #[test]
    fn load_channel_configs_ntfy_missing_topic_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.bad-ntfy]
type = "ntfy"
url = "https://ntfy.sh"

[channels.good-ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "alerts"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "ntfy without topic should be excluded");
        assert_eq!(configs[0].name, "good-ntfy");
    }

    #[test]
    fn load_channel_configs_webhook_missing_url_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.bad-hook]
type = "webhook"

[channels.good-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "webhook without url should be excluded");
        assert_eq!(configs[0].name, "good-hook");
    }

    #[test]
    fn load_channel_configs_webhook_unsupported_method_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.bad-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
method = "DELETE"

[channels.good-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
method = "PUT"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(
            configs.len(),
            1,
            "webhook with unsupported method should be excluded at load time"
        );
        assert_eq!(configs[0].name, "good-hook");
    }

    #[test]
    fn load_channel_configs_webhook_method_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.lower-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
method = "put"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(
            configs.len(),
            1,
            "lowercase but otherwise-valid method should be accepted"
        );
    }

    #[test]
    fn load_channel_configs_unknown_type_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.bad-channel]
type = "carrier-pigeon"

[channels.good-ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "alerts"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "unknown channel type should be excluded");
        assert_eq!(configs[0].name, "good-ntfy");
    }

    // ── MCP empty-string boundary tests ────────────────────────────────

    #[test]
    fn load_mcp_servers_http_empty_url_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "empty-url": {
                        "type": "http",
                        "url": ""
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert!(
            servers.is_empty(),
            "HTTP server with empty url should be skipped"
        );
    }

    #[test]
    fn load_mcp_servers_stdio_empty_command_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "empty-cmd": {
                        "command": ""
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        assert!(
            servers.is_empty(),
            "stdio server with empty command should be skipped"
        );
    }

    // ── macOS channel config tests ─────────────────────────────────────

    #[test]
    fn load_channel_configs_macos_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.macos]
type = "macos"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "should load one channel");
        let c = &configs[0];
        assert_eq!(c.name, "macos");
        let ExternalChannelKind::Macos {
            default_priority,
            throttle_window_secs,
            sound,
            app_name,
            web_url,
        } = &c.kind
        else {
            unreachable!("expected Macos kind");
        };
        assert!(default_priority.is_none(), "minimal config has no priority");
        assert!(
            throttle_window_secs.is_none(),
            "minimal config has no throttle"
        );
        assert!(sound.is_none(), "minimal config has no sound");
        assert!(app_name.is_none(), "minimal config has no app_name");
        assert!(web_url.is_none(), "minimal config has no web_url");
    }

    #[test]
    fn load_channel_configs_macos_fully_specified() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        std::fs::write(
            &path,
            r#"
[channels.macos_alerts]
type = "macos"
default_category = "alerts"
default_priority = "time_sensitive"
throttle_window_secs = 10
sound = true
app_name = "Residuum"
web_url = "http://localhost:3000"
"#,
        )
        .unwrap();

        let configs = load_channel_configs(&path).unwrap();
        assert_eq!(configs.len(), 1, "should load one channel");
        let c = &configs[0];
        assert_eq!(c.name, "macos_alerts");
        let ExternalChannelKind::Macos {
            default_priority,
            throttle_window_secs,
            sound,
            app_name,
            web_url,
        } = &c.kind
        else {
            unreachable!("expected Macos kind");
        };
        assert_eq!(default_priority.as_deref(), Some("time_sensitive"));
        assert_eq!(*throttle_window_secs, Some(10));
        assert_eq!(*sound, Some(true));
        assert_eq!(app_name.as_deref(), Some("Residuum"));
        assert_eq!(web_url.as_deref(), Some("http://localhost:3000"));
    }

    // ── Channel diagnostics ──────────────────────────────────────────────

    #[test]
    fn diagnose_channels_toml_clean_file_has_no_diagnostics() {
        let content = "[channels.my-ntfy]\ntype = \"ntfy\"\nurl = \"https://ntfy.sh\"\ntopic = \"residuum\"\n";
        assert!(diagnose_channels_toml(content).is_empty());
    }

    #[test]
    fn diagnose_channels_toml_reports_syntax_error_with_line_column() {
        use crate::diagnostics::Location;

        let diagnostics = diagnose_channels_toml("not valid toml [[[");
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            diagnostics[0].location,
            Some(Location::LineColumn { .. })
        ));
    }

    #[test]
    fn diagnose_channels_toml_reports_retired_field_as_warning() {
        let content = "[channels.macos]\ntype = \"macos\"\ndefault_category = \"alerts\"\n";
        let diagnostics = diagnose_channels_toml(content);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].severity,
            crate::diagnostics::Severity::Warning
        );
        assert!(diagnostics[0].message.contains("default_category"));
    }

    #[test]
    fn diagnose_channels_toml_matches_loader_on_same_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channels.toml");
        let content = r#"
[channels.bad-hook]
type = "webhook"

[channels.good-hook]
type = "webhook"
url = "https://hooks.example.com/notify"
"#;
        std::fs::write(&path, content).unwrap();

        let configs = load_channel_configs(&path).unwrap();
        let diagnostics = diagnose_channels_toml(content);

        assert_eq!(configs.len(), 1, "one channel should load");
        assert_eq!(
            diagnostics.len(),
            1,
            "one diagnostic for the dropped channel"
        );
        assert!(diagnostics[0].message.contains("bad-hook"));
    }

    // ── MCP diagnostics ──────────────────────────────────────────────────

    #[test]
    fn diagnose_mcp_json_clean_file_has_no_diagnostics() {
        let content = r#"{"mcpServers": {"fs": {"command": "mcp-fs"}}}"#;
        assert!(diagnose_mcp_json(content).is_empty());
    }

    #[test]
    fn diagnose_mcp_json_reports_syntax_error_with_line_column() {
        use crate::diagnostics::Location;

        let diagnostics = diagnose_mcp_json("not valid json {{{");
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            diagnostics[0].location,
            Some(Location::LineColumn { .. })
        ));
    }

    #[test]
    fn diagnose_mcp_json_reports_dropped_entry() {
        let content = r#"{"mcpServers": {"broken": {"type": "sse", "url": "http://x"}}}"#;
        let diagnostics = diagnose_mcp_json(content);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, crate::diagnostics::Severity::Error);
        assert!(diagnostics[0].message.contains("broken"));
    }

    #[test]
    fn diagnose_mcp_json_matches_loader_on_same_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let content = r#"{
            "mcpServers": {
                "good": { "command": "mcp-good" },
                "sse-server": { "type": "sse", "url": "http://x" }
            }
        }"#;
        std::fs::write(&path, content).unwrap();

        let servers = load_mcp_servers(&path).unwrap();
        let diagnostics = diagnose_mcp_json(content);

        assert_eq!(servers.len(), 1, "one server should load");
        assert_eq!(
            diagnostics.len(),
            1,
            "one diagnostic for the dropped server"
        );
        assert!(diagnostics[0].message.contains("sse-server"));
    }

    // ── MCP map + resolution tests ──────────────────────────────────────

    #[test]
    fn load_mcp_servers_map_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "filesystem": {
                        "command": "mcp-server-filesystem",
                        "args": ["/home/user"]
                    },
                    "git": {
                        "command": "mcp-git",
                        "args": ["--repo", "."]
                    }
                }
            }"#,
        )
        .unwrap();

        let map = load_mcp_servers_map(&path).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("filesystem"));
        assert!(map.contains_key("git"));
        assert_eq!(map["filesystem"].command, "mcp-server-filesystem");
    }
}

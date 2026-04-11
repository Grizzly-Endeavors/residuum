//! Workspace config loaders: MCP servers and notification channels.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use anyhow::Context;

use crate::notify::types::{ExternalChannelConfig, ExternalChannelKind};
use crate::projects::types::{McpServerEntry, McpTransport};

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
        .filter_map(|(name, raw)| {
            // Resolve transport: check `type` first (Claude standard), then `transport` (Residuum)
            let transport_str = raw.type_.as_deref().or(raw.transport.as_deref());
            let transport = match transport_str {
                Some("streamable-http" | "http") => McpTransport::Http,
                None | Some("stdio") => McpTransport::Stdio,
                Some("sse") => {
                    tracing::warn!(
                        server = %name,
                        "SSE transport is deprecated by the MCP spec, skipping server"
                    );
                    return None;
                }
                Some(unknown) => {
                    tracing::warn!(
                        server = %name,
                        transport = %unknown,
                        "unrecognized MCP transport, skipping server"
                    );
                    return None;
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
                        tracing::warn!(
                            server = %name,
                            "HTTP MCP server has no url or command, skipping"
                        );
                        return None;
                    }
                }
                McpTransport::Stdio => {
                    if let Some(cmd) = raw.command.filter(|c| !c.is_empty()) {
                        cmd
                    } else {
                        tracing::warn!(
                            server = %name,
                            "stdio MCP server has no command, skipping"
                        );
                        return None;
                    }
                }
            };

            let entry = McpServerEntry {
                name: name.clone(),
                command,
                args: raw.args,
                env: raw.env,
                transport,
                headers: raw.headers,
            };
            Some((name, entry))
        })
        .collect();

    tracing::debug!(count = servers.len(), path = %path.display(), "loaded MCP servers");
    Ok(servers)
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

/// Resolve MCP server name references against project-local and global `mcp.json` files.
///
/// For each reference, the project-local map is checked first, then the global map.
/// Project-local entries override same-name global entries.
///
/// Returns an empty vec if `references` is empty (without loading any files).
///
/// # Errors
/// Returns an error if any reference cannot be found in either map.
#[tracing::instrument(skip_all, fields(project = %project_name, count = references.len()))]
pub fn resolve_mcp_references(
    references: &[String],
    project_mcp_json: &Path,
    global_mcp_json: &Path,
    project_name: &str,
) -> anyhow::Result<Vec<McpServerEntry>> {
    if references.is_empty() {
        return Ok(Vec::new());
    }

    let local_map = load_mcp_servers_map(project_mcp_json)?;
    let global_map = load_mcp_servers_map(global_mcp_json)?;

    let mut resolved = Vec::with_capacity(references.len());
    for name in references {
        if let Some(entry) = local_map.get(name) {
            resolved.push(entry.clone());
        } else if let Some(entry) = global_map.get(name) {
            resolved.push(entry.clone());
        } else {
            anyhow::bail!(
                "mcp server '{name}' referenced in project '{project_name}' not found in project-local or global mcp.json"
            );
        }
    }

    Ok(resolved)
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
    default_category: Option<String>,
    default_priority: Option<String>,
    throttle_window_secs: Option<u64>,
    sound: Option<bool>,
    app_name: Option<String>,
    web_url: Option<String>,
    // Windows-specific fields
    default_scenario: Option<String>,
    app_id: Option<String>,
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
            let kind = match raw.type_.as_str() {
                "ntfy" => {
                    let Some(url) = raw.url.filter(|u| !u.is_empty()) else {
                        tracing::warn!(channel = %name, "ntfy channel is missing required 'url' field");
                        return None;
                    };
                    let Some(topic) = raw.topic.filter(|t| !t.is_empty()) else {
                        tracing::warn!(channel = %name, "ntfy channel is missing required 'topic' field");
                        return None;
                    };
                    ExternalChannelKind::Ntfy {
                        url,
                        topic,
                        priority: raw.priority,
                    }
                }
                "macos" => ExternalChannelKind::Macos {
                    default_category: raw.default_category,
                    default_priority: raw.default_priority,
                    throttle_window_secs: raw.throttle_window_secs,
                    sound: raw.sound,
                    app_name: raw.app_name,
                    web_url: raw.web_url,
                },
                "windows" => ExternalChannelKind::Windows {
                    default_category: raw.default_category,
                    default_scenario: raw.default_scenario,
                    throttle_window_secs: raw.throttle_window_secs,
                    sound: raw.sound,
                    app_name: raw.app_name,
                    app_id: raw.app_id,
                },
                "webhook" => {
                    let Some(url) = raw.url.filter(|u| !u.is_empty()) else {
                        tracing::warn!(channel = %name, "webhook channel is missing required 'url' field");
                        return None;
                    };
                    ExternalChannelKind::Webhook {
                        url,
                        method: raw.method,
                        headers: raw.headers.unwrap_or_default().into_iter().collect(),
                    }
                }
                unknown => {
                    tracing::warn!(
                        channel = %name,
                        type_ = %unknown,
                        "unrecognized channel type, skipping"
                    );
                    return None;
                }
            };
            Some(ExternalChannelConfig { name, kind })
        })
        .collect();

    Ok(configs)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
            default_category,
            default_priority,
            throttle_window_secs,
            sound,
            app_name,
            web_url,
        } = &c.kind
        else {
            unreachable!("expected Macos kind");
        };
        assert!(default_category.is_none(), "minimal config has no category");
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
            default_category,
            default_priority,
            throttle_window_secs,
            sound,
            app_name,
            web_url,
        } = &c.kind
        else {
            unreachable!("expected Macos kind");
        };
        assert_eq!(default_category.as_deref(), Some("alerts"));
        assert_eq!(default_priority.as_deref(), Some("time_sensitive"));
        assert_eq!(*throttle_window_secs, Some(10));
        assert_eq!(*sound, Some(true));
        assert_eq!(app_name.as_deref(), Some("Residuum"));
        assert_eq!(web_url.as_deref(), Some("http://localhost:3000"));
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

    #[test]
    fn resolve_references_from_global_only() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global-mcp.json");
        std::fs::write(
            &global,
            r#"{ "mcpServers": { "fs": { "command": "mcp-fs" } } }"#,
        )
        .unwrap();

        let project_local = dir.path().join("nonexistent-mcp.json");
        let refs = vec!["fs".to_string()];
        let resolved = resolve_mcp_references(&refs, &project_local, &global, "test-proj").unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "fs");
        assert_eq!(resolved[0].command, "mcp-fs");
    }

    #[test]
    fn resolve_references_project_overrides_global() {
        let dir = tempfile::tempdir().unwrap();

        let global = dir.path().join("global-mcp.json");
        std::fs::write(
            &global,
            r#"{ "mcpServers": { "fs": { "command": "global-fs" } } }"#,
        )
        .unwrap();

        let local = dir.path().join("local-mcp.json");
        std::fs::write(
            &local,
            r#"{ "mcpServers": { "fs": { "command": "local-fs" } } }"#,
        )
        .unwrap();

        let refs = vec!["fs".to_string()];
        let resolved = resolve_mcp_references(&refs, &local, &global, "test-proj").unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].command, "local-fs",
            "project-local should override global"
        );
    }

    #[test]
    fn resolve_references_mixed_sources() {
        let dir = tempfile::tempdir().unwrap();

        let global = dir.path().join("global-mcp.json");
        std::fs::write(
            &global,
            r#"{ "mcpServers": { "git": { "command": "mcp-git" } } }"#,
        )
        .unwrap();

        let local = dir.path().join("local-mcp.json");
        std::fs::write(
            &local,
            r#"{ "mcpServers": { "fs": { "command": "mcp-fs" } } }"#,
        )
        .unwrap();

        let refs = vec!["fs".to_string(), "git".to_string()];
        let resolved = resolve_mcp_references(&refs, &local, &global, "test-proj").unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name, "fs", "first should come from local");
        assert_eq!(resolved[1].name, "git", "second should come from global");
    }

    #[test]
    fn resolve_references_not_found_errors() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global-mcp.json");
        std::fs::write(&global, r#"{ "mcpServers": {} }"#).unwrap();

        let local = dir.path().join("nonexistent.json");
        let refs = vec!["missing-server".to_string()];
        let result = resolve_mcp_references(&refs, &local, &global, "my-project");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing-server"),
            "error should name the server: {err}"
        );
        assert!(
            err.contains("my-project"),
            "error should name the project: {err}"
        );
    }

    #[test]
    fn resolve_references_empty_list() {
        let nonexistent = Path::new("/tmp/does-not-exist/mcp.json");
        let resolved = resolve_mcp_references(&[], nonexistent, nonexistent, "test-proj").unwrap();
        assert!(
            resolved.is_empty(),
            "empty references should return empty vec"
        );
    }

    #[test]
    fn resolve_references_missing_both_files() {
        let local = Path::new("/tmp/no-local/mcp.json");
        let global = Path::new("/tmp/no-global/mcp.json");
        let refs = vec!["some-server".to_string()];
        let result = resolve_mcp_references(&refs, local, global, "test-proj");
        assert!(result.is_err(), "should error when server not found");
    }
}

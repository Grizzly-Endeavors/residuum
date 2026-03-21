//! Workspace config loaders: MCP servers and notification channels.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::ResiduumError;
use crate::notify::channels::NotificationChannel;
use crate::notify::external::{NtfyChannel, WebhookChannel};
use crate::notify::types::{ExternalChannelConfig, ExternalChannelKind};
use crate::projects::types::{McpServerEntry, McpTransport};

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
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Claude Code/Desktop standard: `"stdio"`, `"streamable-http"`, `"http"`, or `"sse"`.
    #[serde(rename = "type", default)]
    type_: Option<String>,
    /// Residuum extension: `"stdio"` (default) or `"http"`.
    #[serde(default)]
    transport: Option<String>,
    /// HTTP server URL (alternative to putting the URL in `command`).
    #[serde(default)]
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
pub fn load_mcp_servers_map(path: &Path) -> Result<HashMap<String, McpServerEntry>, ResiduumError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to read mcp.json at {}: {e}",
            path.display()
        ))
    })?;

    let file: McpConfigFile = serde_json::from_str(&contents).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to parse mcp.json at {}: {e}",
            path.display()
        ))
    })?;

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
pub fn load_mcp_servers(path: &Path) -> Result<Vec<McpServerEntry>, ResiduumError> {
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
pub fn resolve_mcp_references(
    references: &[String],
    project_mcp_json: &Path,
    global_mcp_json: &Path,
    project_name: &str,
) -> Result<Vec<McpServerEntry>, ResiduumError> {
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
            return Err(ResiduumError::Projects(format!(
                "mcp server '{name}' referenced in project '{project_name}' not found in project-local or global mcp.json"
            )));
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
    /// Channel type: `"ntfy"`, `"webhook"`, or `"macos"`.
    #[serde(rename = "type")]
    type_: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    // macOS channel fields
    #[serde(default)]
    default_category: Option<String>,
    #[serde(default)]
    default_priority: Option<String>,
    #[serde(default)]
    throttle_window_secs: Option<u64>,
    #[serde(default)]
    sound: Option<bool>,
    #[serde(default)]
    app_name: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
}

/// Load external channel configs from a TOML file.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_channel_configs(path: &Path) -> Result<Vec<ExternalChannelConfig>, ResiduumError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to read channels.toml at {}: {e}",
            path.display()
        ))
    })?;

    // Empty file → empty vec (no channels section)
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }

    let file: ChannelsFile = toml::from_str(&contents).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to parse channels.toml at {}: {e}",
            path.display()
        ))
    })?;

    let configs = file
        .channels
        .into_iter()
        .map(|(name, raw)| {
            let kind = match raw.type_.as_str() {
                "ntfy" => {
                    let url = raw.url.unwrap_or_default();
                    let topic = raw.topic.unwrap_or_default();
                    if url.is_empty() {
                        tracing::warn!(channel = %name, "ntfy channel is missing required 'url' field");
                    }
                    if topic.is_empty() {
                        tracing::warn!(channel = %name, "ntfy channel is missing required 'topic' field");
                    }
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
                "webhook" => {
                    let url = raw.url.unwrap_or_default();
                    if url.is_empty() {
                        tracing::warn!(channel = %name, "webhook channel is missing required 'url' field");
                    }
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
                        "unrecognized channel type, falling back to webhook; this channel will likely fail at send time"
                    );
                    let url = raw.url.unwrap_or_default();
                    if url.is_empty() {
                        tracing::warn!(channel = %name, "channel is missing required 'url' field");
                    }
                    ExternalChannelKind::Webhook {
                        url,
                        method: raw.method,
                        headers: raw.headers.unwrap_or_default().into_iter().collect(),
                    }
                }
            };
            ExternalChannelConfig { name, kind }
        })
        .collect();

    Ok(configs)
}

// ── Channel builder ──────────────────────────────────────────────────────────

/// Build external channel implementations from configs.
pub async fn build_external_channels(
    configs: &[ExternalChannelConfig],
    client: &reqwest::Client,
) -> HashMap<String, Box<dyn NotificationChannel>> {
    let mut channels: HashMap<String, Box<dyn NotificationChannel>> = HashMap::new();

    for cfg in configs {
        let channel: Option<Box<dyn NotificationChannel>> = match &cfg.kind {
            ExternalChannelKind::Ntfy {
                url,
                topic,
                priority,
            } => Some(Box::new(NtfyChannel::new(
                cfg.name.clone(),
                client.clone(),
                url.clone(),
                topic.clone(),
                priority.clone(),
            ))),
            ExternalChannelKind::Webhook {
                url,
                method,
                headers,
            } => Some(Box::new(WebhookChannel::new(
                cfg.name.clone(),
                client.clone(),
                url.clone(),
                method.clone(),
                headers.clone(),
            ))),
            ExternalChannelKind::Macos {
                default_category,
                default_priority,
                throttle_window_secs,
                sound,
                app_name,
                web_url,
            } => {
                build_macos_channel(
                    &cfg.name,
                    default_category.as_ref(),
                    default_priority.as_ref(),
                    throttle_window_secs.as_ref(),
                    sound.as_ref(),
                    app_name.as_ref(),
                    web_url.as_ref(),
                )
                .await
            }
        };
        if let Some(ch) = channel {
            channels.insert(cfg.name.clone(), ch);
        }
    }

    tracing::debug!(count = channels.len(), "built external channels");
    channels
}

/// Build a macOS notification channel from raw config fields.
///
/// On non-macOS platforms, logs a warning and returns `None`.
#[cfg(target_os = "macos")]
async fn build_macos_channel(
    name: &str,
    default_category: Option<&String>,
    default_priority: Option<&String>,
    throttle_window_secs: Option<&u64>,
    sound: Option<&bool>,
    app_name: Option<&String>,
    web_url: Option<&String>,
) -> Option<Box<dyn NotificationChannel>> {
    use crate::notify::macos::MacosChannelConfig;
    use crate::notify::macos::categories::{parse_category, parse_priority};

    let mut config = MacosChannelConfig::default();

    if let Some(cat) = default_category {
        match parse_category(cat) {
            Ok(c) => config.default_category = c,
            Err(e) => {
                tracing::warn!(channel = name, error = %e, "invalid macOS channel config, skipping");
                return None;
            }
        }
    }

    if let Some(pri) = default_priority {
        match parse_priority(pri) {
            Ok(p) => config.default_priority = p,
            Err(e) => {
                tracing::warn!(channel = name, error = %e, "invalid macOS channel config, skipping");
                return None;
            }
        }
    }

    if let Some(secs) = throttle_window_secs {
        config.throttle_window_secs = *secs;
    }
    if let Some(s) = sound {
        config.sound = *s;
    }
    if let Some(n) = app_name {
        config.app_name = n.clone();
    }
    config.web_url = web_url.cloned();

    match crate::notify::macos::MacosNativeChannel::new(name, config).await {
        Ok((channel, _handle)) => {
            tracing::info!(channel = name, "macOS notification channel initialized");
            Some(Box::new(channel))
        }
        Err(e) => {
            tracing::warn!(channel = name, error = %e, "failed to initialize macOS channel, skipping");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[expect(
    clippy::unused_async,
    reason = "signature must match the async macOS variant"
)]
async fn build_macos_channel(
    name: &str,
    _default_category: Option<&String>,
    _default_priority: Option<&String>,
    _throttle_window_secs: Option<&u64>,
    _sound: Option<&bool>,
    _app_name: Option<&String>,
    _web_url: Option<&String>,
) -> Option<Box<dyn NotificationChannel>> {
    tracing::warn!(
        channel = name,
        "macOS notification channel configured but not available on this platform"
    );
    None
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

    // ── Channel builder tests ────────────────────────────────────────────

    #[tokio::test]
    async fn build_external_channels_creates_instances() {
        let configs = vec![
            ExternalChannelConfig {
                name: "my-ntfy".to_string(),
                kind: ExternalChannelKind::Ntfy {
                    url: "https://ntfy.sh".to_string(),
                    topic: "test".to_string(),
                    priority: None,
                },
            },
            ExternalChannelConfig {
                name: "my-webhook".to_string(),
                kind: ExternalChannelKind::Webhook {
                    url: "https://hooks.example.com".to_string(),
                    method: None,
                    headers: Vec::new(),
                },
            },
        ];

        let client = reqwest::Client::new();
        let channels = build_external_channels(&configs, &client).await;

        assert_eq!(channels.len(), 2);
        assert!(channels.contains_key("my-ntfy"));
        assert!(channels.contains_key("my-webhook"));
        assert_eq!(channels["my-ntfy"].channel_kind(), "ntfy");
        assert_eq!(channels["my-webhook"].channel_kind(), "webhook");
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

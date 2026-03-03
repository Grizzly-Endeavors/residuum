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
#[derive(Deserialize)]
struct McpServerRaw {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    /// Residuum extension: `"stdio"` (default) or `"http"`.
    #[serde(default)]
    transport: Option<String>,
}

/// Load MCP server definitions from a JSON file.
///
/// Returns an empty vec if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_mcp_servers(path: &Path) -> Result<Vec<McpServerEntry>, ResiduumError> {
    if !path.exists() {
        return Ok(Vec::new());
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

    let servers = file
        .mcp_servers
        .into_iter()
        .map(|(name, raw)| {
            let transport = match raw.transport.as_deref() {
                Some("http") => McpTransport::Http,
                _ => McpTransport::Stdio,
            };
            McpServerEntry {
                name,
                command: raw.command,
                args: raw.args,
                env: raw.env,
                transport,
            }
        })
        .collect();

    Ok(servers)
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
    /// Channel type: `"ntfy"` or `"webhook"`.
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
                "ntfy" => ExternalChannelKind::Ntfy {
                    url: raw.url.unwrap_or_default(),
                    topic: raw.topic.unwrap_or_default(),
                    priority: raw.priority,
                },
                _ => ExternalChannelKind::Webhook {
                    url: raw.url.unwrap_or_default(),
                    method: raw.method,
                    headers: raw.headers.unwrap_or_default().into_iter().collect(),
                },
            };
            ExternalChannelConfig { name, kind }
        })
        .collect();

    Ok(configs)
}

// ── Channel builder ──────────────────────────────────────────────────────────

/// Build external channel implementations from configs.
#[must_use]
pub fn build_external_channels(
    configs: &[ExternalChannelConfig],
    client: &reqwest::Client,
) -> HashMap<String, Box<dyn NotificationChannel>> {
    let mut channels: HashMap<String, Box<dyn NotificationChannel>> = HashMap::new();

    for cfg in configs {
        let channel: Box<dyn NotificationChannel> = match &cfg.kind {
            ExternalChannelKind::Ntfy {
                url,
                topic,
                priority,
            } => Box::new(NtfyChannel::new(
                cfg.name.clone(),
                client.clone(),
                url.clone(),
                topic.clone(),
                priority.clone(),
            )),
            ExternalChannelKind::Webhook {
                url,
                method,
                headers,
            } => Box::new(WebhookChannel::new(
                cfg.name.clone(),
                client.clone(),
                url.clone(),
                method.clone(),
                headers.clone(),
            )),
        };
        channels.insert(cfg.name.clone(), channel);
    }

    channels
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

    #[test]
    fn build_external_channels_creates_instances() {
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
        let channels = build_external_channels(&configs, &client);

        assert_eq!(channels.len(), 2);
        assert!(channels.contains_key("my-ntfy"));
        assert!(channels.contains_key("my-webhook"));
        assert_eq!(channels["my-ntfy"].channel_kind(), "ntfy");
        assert_eq!(channels["my-webhook"].channel_kind(), "webhook");
    }
}

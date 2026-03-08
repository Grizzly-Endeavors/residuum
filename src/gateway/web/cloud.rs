//! Cloud tunnel API endpoints: status polling, OAuth callback, and disconnect.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::secrets::SecretStore;
use crate::gateway::types::ReloadSignal;
use crate::tunnel::TunnelStatus;

/// Shared state for cloud API endpoints.
#[derive(Clone)]
pub(crate) struct CloudApiState {
    pub config_dir: PathBuf,
    pub reload_tx: watch::Sender<ReloadSignal>,
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Serialize)]
pub(crate) struct CloudStatusResponse {
    status: &'static str,
    user_id: Option<String>,
    has_token: bool,
    enabled: bool,
}

/// `GET /api/cloud/status` — return current tunnel status.
pub(crate) async fn api_cloud_status(
    State(state): State<CloudApiState>,
) -> Json<CloudStatusResponse> {
    let tunnel_status = state.tunnel_status_rx.borrow().clone();

    let (status, user_id) = match tunnel_status {
        TunnelStatus::Disconnected => ("disconnected", None),
        TunnelStatus::Connecting => ("connecting", None),
        TunnelStatus::Connected { ref user_id } => ("connected", Some(user_id.clone())),
    };

    // Check config for cloud section presence
    let config_path = state.config_dir.join("config.toml");
    let (has_token, enabled) = match std::fs::read_to_string(&config_path) {
        Ok(raw) => parse_cloud_state(&raw),
        Err(_) => (false, false),
    };

    Json(CloudStatusResponse {
        status,
        user_id,
        has_token,
        enabled,
    })
}

/// Parse `[cloud]` section from raw TOML to extract token presence and enabled state.
fn parse_cloud_state(raw: &str) -> (bool, bool) {
    // Simple TOML parsing for the cloud section
    let mut in_cloud = false;
    let mut has_token = false;
    let mut enabled = true; // default is enabled if section exists

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_cloud = trimmed == "[cloud]";
            continue;
        }
        if !in_cloud {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("token")
            && rest.trim_start().starts_with('=')
        {
            let val = rest.trim_start().trim_start_matches('=').trim();
            has_token = !val.trim_matches('"').is_empty();
        }
        if let Some(rest) = trimmed.strip_prefix("enabled")
            && rest.trim_start().starts_with('=')
        {
            let val = rest.trim_start().trim_start_matches('=').trim();
            enabled = val != "false";
        }
    }

    if !in_cloud && !raw.contains("[cloud]") {
        return (false, false);
    }

    (has_token, enabled)
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    token: Option<String>,
}

/// `GET /cloud/callback?token=...` — localhost OAuth callback.
///
/// Stores the token in the secret store, updates config.toml to enable
/// the cloud tunnel, and triggers a config reload.
pub(crate) async fn cloud_callback(
    State(state): State<CloudApiState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(token) = query.token.filter(|t| !t.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Error</h1><p>Missing token parameter.</p>".to_string()),
        )
            .into_response();
    };

    let config_dir = state.config_dir.clone();
    let secret_lock = Arc::clone(&state.secret_lock);

    // Store the token as a secret
    let store_result = {
        let _guard = secret_lock.lock().await;
        let dir = config_dir.clone();
        let tok = token.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = SecretStore::load(&dir)?;
            store.set("cloud_token", &tok, &dir)?;
            Ok::<(), crate::error::ResiduumError>(())
        })
        .await
    };

    if let Err(e) = store_result {
        tracing::error!(error = %e, "failed to store cloud token");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<h1>Error</h1><p>Failed to store token: {e}</p>")),
        )
            .into_response();
    }
    if let Ok(Err(e)) = store_result {
        tracing::error!(error = %e, "failed to store cloud token");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<h1>Error</h1><p>Failed to store token: {e}</p>")),
        )
            .into_response();
    }

    // Update config.toml to add/update [cloud] section
    let config_path = config_dir.join("config.toml");
    let current = std::fs::read_to_string(&config_path).unwrap_or_default();
    let updated = update_cloud_section(&current, true);

    if let Err(e) = std::fs::write(&config_path, &updated) {
        tracing::error!(error = %e, "failed to write config.toml");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<h1>Error</h1><p>Failed to update config: {e}</p>")),
        )
            .into_response();
    }

    // Trigger reload
    state.reload_tx.send(ReloadSignal::Root).ok();

    (StatusCode::OK, Html(SUCCESS_HTML.to_string())).into_response()
}

/// `POST /api/cloud/disconnect` — disable cloud tunnel without removing the token.
pub(crate) async fn api_cloud_disconnect(
    State(state): State<CloudApiState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config_path = state.config_dir.join("config.toml");
    let current = std::fs::read_to_string(&config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read config.toml: {e}"),
        )
    })?;

    let updated = update_cloud_section(&current, false);
    std::fs::write(&config_path, &updated).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write config.toml: {e}"),
        )
    })?;

    state.reload_tx.send(ReloadSignal::Root).ok();

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Update or insert the `[cloud]` section in config TOML.
///
/// When `enable` is true: sets `enabled = true` and `token = "secret:cloud_token"`.
/// When `enable` is false: sets `enabled = false`, preserves token.
fn update_cloud_section(raw: &str, enable: bool) -> String {
    let mut lines: Vec<String> = raw.lines().map(String::from).collect();
    let mut cloud_start = None;
    let mut cloud_end = None;

    // Find the [cloud] section boundaries
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[cloud]" {
            cloud_start = Some(i);
        } else if cloud_start.is_some() && cloud_end.is_none() && trimmed.starts_with('[') {
            cloud_end = Some(i);
        }
    }

    if let Some(start) = cloud_start {
        let end = cloud_end.unwrap_or(lines.len());
        // Update existing section
        let mut new_section = vec!["[cloud]".to_string()];
        if enable {
            new_section.push("enabled = true".to_string());
        } else {
            new_section.push("enabled = false".to_string());
        }

        // Preserve token line, relay_url, and local_port from original
        for line in lines.get(start + 1..end).unwrap_or_default() {
            let trimmed = line.trim();
            if trimmed.starts_with("token")
                || trimmed.starts_with("relay_url")
                || trimmed.starts_with("local_port")
            {
                new_section.push(line.clone());
            }
        }

        // If enabling and no token line was found, add default
        if enable && !new_section.iter().any(|l| l.trim().starts_with("token")) {
            new_section.push("token = \"secret:cloud_token\"".to_string());
        }

        lines.splice(start..end, new_section);
    } else {
        // Append new section
        if !lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[cloud]".to_string());
        if enable {
            lines.push("enabled = true".to_string());
            lines.push("token = \"secret:cloud_token\"".to_string());
        } else {
            lines.push("enabled = false".to_string());
        }
    }

    let mut result = lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Connected - Residuum</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #0a0a0a; color: #e0e0e0; display: flex; justify-content: center; align-items: center; min-height: 100vh; }
        .card { background: #1a1a1a; border: 1px solid #333; border-radius: 12px; padding: 2.5rem; width: 100%; max-width: 400px; text-align: center; }
        h1 { font-size: 1.5rem; margin-bottom: 0.5rem; color: #4ade80; }
        p { color: #888; font-size: 0.95rem; }
    </style>
</head>
<body>
    <div class="card">
        <h1>Connected successfully!</h1>
        <p>Your cloud tunnel is now active. You can close this tab.</p>
    </div>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cloud_state_no_section() {
        let (has_token, enabled) = parse_cloud_state("timezone = \"UTC\"\n");
        assert!(!has_token);
        assert!(!enabled);
    }

    #[test]
    fn parse_cloud_state_enabled_with_token() {
        let toml = r#"
[cloud]
enabled = true
token = "secret:cloud_token"
"#;
        let (has_token, enabled) = parse_cloud_state(toml);
        assert!(has_token);
        assert!(enabled);
    }

    #[test]
    fn parse_cloud_state_disabled() {
        let toml = r#"
[cloud]
enabled = false
token = "secret:cloud_token"
"#;
        let (has_token, enabled) = parse_cloud_state(toml);
        assert!(has_token);
        assert!(!enabled);
    }

    #[test]
    fn parse_cloud_state_no_token() {
        let toml = "[cloud]\nenabled = true\n";
        let (has_token, enabled) = parse_cloud_state(toml);
        assert!(!has_token);
        assert!(enabled);
    }

    #[test]
    fn update_cloud_section_insert_new() {
        let raw = "timezone = \"UTC\"\n";
        let result = update_cloud_section(raw, true);
        assert!(result.contains("[cloud]"));
        assert!(result.contains("enabled = true"));
        assert!(result.contains("token = \"secret:cloud_token\""));
    }

    #[test]
    fn update_cloud_section_disable_existing() {
        let raw = r#"timezone = "UTC"

[cloud]
enabled = true
token = "secret:cloud_token"
"#;
        let result = update_cloud_section(raw, false);
        assert!(result.contains("enabled = false"));
        assert!(result.contains("token = \"secret:cloud_token\""));
        assert!(!result.contains("enabled = true"));
    }

    #[test]
    fn update_cloud_section_enable_existing() {
        let raw = r#"timezone = "UTC"

[cloud]
enabled = false
token = "secret:cloud_token"
relay_url = "wss://custom.example.com"
"#;
        let result = update_cloud_section(raw, true);
        assert!(result.contains("enabled = true"));
        assert!(result.contains("token = \"secret:cloud_token\""));
        assert!(result.contains("relay_url = \"wss://custom.example.com\""));
        assert!(!result.contains("enabled = false"));
    }

    #[test]
    fn update_cloud_section_preserves_other_sections() {
        let raw = r#"timezone = "UTC"

[gateway]
port = 7700

[cloud]
enabled = true
token = "secret:cloud_token"

[discord]
token = "secret:discord"
"#;
        let result = update_cloud_section(raw, false);
        assert!(result.contains("[gateway]"));
        assert!(result.contains("[discord]"));
        assert!(result.contains("enabled = false"));
    }
}

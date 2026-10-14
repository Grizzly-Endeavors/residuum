//! Cloud tunnel API endpoints: status polling, OAuth callback, and disconnect.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Redirect, Response};
use serde::Serialize;
use tokio::sync::watch;

use crate::config::secrets::SecretStore;
use crate::gateway::types::ReloadSignal;
use crate::tunnel::TunnelStatus;

/// Shared state for cloud API endpoints.
#[derive(Clone)]
pub(crate) struct CloudApiState {
    pub hub_dir: PathBuf,
    pub reload_tx: crate::gateway::types::ReloadSender,
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    /// The sign-ins `POST /api/hub/cloud/connect` started, which the callback
    /// checks its `state` against.
    pub sign_ins: super::cloud_sign_in::PendingSignIns,
}

/// The tunnel's status as `GET /api/hub/cloud/status` and the `tunnel` field
/// of `GET /api/hub/status` report it.
#[derive(Serialize)]
pub(crate) struct CloudStatusResponse {
    status: &'static str,
    user_id: Option<String>,
    has_token: bool,
    enabled: bool,
    /// Whether this response is being served to a browser that's viewing
    /// the gateway through the tunnel rather than directly. The web UI uses
    /// this to hide the disconnect control: disconnecting over the tunnel
    /// is refused server-side (see `gateway::remote_control_guard`), so a
    /// remote viewer should never see a button that can't work.
    viewed_via_tunnel: bool,
    /// Public origin of this user's web UI through the relay, while connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    /// This hub's instance slug on the relay, while connected.
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<String>,
    /// This instance's own public address. The Teams messaging endpoint is
    /// `{instance_origin}/teams/{agent}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_origin: Option<String>,
}

impl CloudStatusResponse {
    /// The status of the tunnel `tunnel_status_rx` watches, with the cloud
    /// settings read from the hub config in `hub_dir`; `viewed_via_tunnel` says
    /// whether the request arrived through Residuum Cloud.
    pub(crate) fn current(
        hub_dir: &std::path::Path,
        tunnel_status_rx: &watch::Receiver<TunnelStatus>,
        viewed_via_tunnel: bool,
    ) -> Self {
        let tunnel_status = tunnel_status_rx.borrow().clone();

        let (status, user_id, origin, instance, instance_origin) = match tunnel_status {
            TunnelStatus::Disconnected => ("disconnected", None, None, None, None),
            TunnelStatus::Connecting => ("connecting", None, None, None, None),
            TunnelStatus::Refused => ("refused", None, None, None, None),
            TunnelStatus::Connected {
                ref user_id,
                ref origin,
                ref instance,
                ref instance_origin,
                ..
            } => (
                "connected",
                Some(user_id.clone()),
                origin.clone(),
                instance.clone(),
                instance_origin.clone(),
            ),
        };

        // Check config for cloud section presence
        let config_path = hub_dir.join("config.toml");
        let (has_token, enabled) = match std::fs::read_to_string(&config_path) {
            Ok(raw) => parse_cloud_state(&raw),
            Err(_) => (false, false),
        };

        Self {
            status,
            user_id,
            has_token,
            enabled,
            viewed_via_tunnel,
            origin,
            instance,
            instance_origin,
        }
    }
}

/// `GET /api/hub/cloud/status` — return current tunnel status.
pub(crate) async fn api_cloud_status(
    State(state): State<CloudApiState>,
    parts: axum::http::request::Parts,
) -> Json<CloudStatusResponse> {
    Json(CloudStatusResponse::current(
        &state.hub_dir,
        &state.tunnel_status_rx,
        crate::pairing::remote::remote_context(&parts.extensions).is_some(),
    ))
}

/// Parse `[cloud]` section from raw TOML to extract token presence and enabled state.
fn parse_cloud_state(raw: &str) -> (bool, bool) {
    let Ok(val) = toml::from_str::<toml::Value>(raw) else {
        return (false, false);
    };
    let Some(cloud) = val.get("cloud").and_then(|v| v.as_table()) else {
        return (false, false);
    };
    let has_token = cloud
        .get("token")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    let enabled = cloud
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    (has_token, enabled)
}

/// Where the sign-in callback sends the browser: Settings → Residuum Cloud,
/// which shows the tunnel's real state and the outcome `cloud=` names.
const CLOUD_SETTINGS_PATH: &str = "/home?settings=_all/cloud";

/// What the sign-in callback tells the app, as `cloud=` on the Settings URL.
/// The web app reads it in `web/src/lib/cloud-handoff.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackOutcome {
    /// The token is saved and the tunnel is starting; the section follows it.
    Connecting,
    /// The relay sent the browser back without a token.
    MissingToken,
    /// The token or the `[cloud]` switch couldn't be saved.
    SaveFailed,
    /// The callback's `state` names no sign-in Connect started here recently,
    /// so its token was not used.
    NotStartedHere,
}

impl CallbackOutcome {
    fn param(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::MissingToken => "missing-token",
            Self::SaveFailed => "save-failed",
            Self::NotStartedHere => "not-started-here",
        }
    }

    /// A `303` into the app, which says what happened in plain language.
    fn hand_off(self) -> Response {
        Redirect::to(&format!("{CLOUD_SETTINGS_PATH}&cloud={}", self.param())).into_response()
    }
}

/// The first non-empty `token` in a raw query string. The query is read by
/// hand so a repeated or malformed parameter still lands in the app rather
/// than on an extractor's plain-text rejection.
fn token_from_query(query: Option<&str>) -> Option<String> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, value)| key == "token" && !value.trim().is_empty())
        .map(|(_, value)| value.into_owned())
}

/// `GET /cloud/callback?token=...&state=...` — where the relay sends the
/// browser after sign-in.
///
/// Refuses a callback whose `state` isn't one Connect started here (see
/// `cloud_sign_in`), saving nothing. Otherwise stores the token in the secret store, switches the tunnel on in the hub's
/// `config.toml`, triggers a config reload, and hands the browser to Settings
/// → Residuum Cloud, which follows the tunnel from there. Every outcome is a
/// redirect into the app; a failure is logged here and named by `cloud=`.
pub(crate) async fn cloud_callback(
    State(state): State<CloudApiState>,
    RawQuery(query): RawQuery,
) -> Response {
    if !state.sign_ins.callback_started_here(query.as_deref()) {
        return CallbackOutcome::NotStartedHere.hand_off();
    }
    let Some(token) = token_from_query(query.as_deref()) else {
        tracing::warn!("the cloud sign-in callback arrived without a token");
        return CallbackOutcome::MissingToken.hand_off();
    };

    let hub_dir = state.hub_dir.clone();
    let store_result = {
        let _guard = state.secret_lock.lock().await;
        let dir = hub_dir.clone();
        crate::util::spawn_blocking_in_span(move || {
            let mut store = SecretStore::load(&dir)?;
            store.set("cloud_token", &token, &dir)?;
            Ok::<(), crate::util::FatalError>(())
        })
        .await
    };
    match store_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::error!(error = %e, hub_dir = %hub_dir.display(), "failed to store the cloud token");
            return CallbackOutcome::SaveFailed.hand_off();
        }
        Err(e) => {
            tracing::error!(error = %e, "the cloud token store task failed");
            return CallbackOutcome::SaveFailed.hand_off();
        }
    }

    let config_path = hub_dir.join("config.toml");
    let current = match std::fs::read_to_string(&config_path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            tracing::error!(error = %e, path = %config_path.display(), "failed to read hub config.toml for the cloud sign-in");
            return CallbackOutcome::SaveFailed.hand_off();
        }
    };
    if let Err(e) = std::fs::write(&config_path, update_cloud_section(&current, true)) {
        tracing::error!(error = %e, path = %config_path.display(), "failed to write hub config.toml for the cloud sign-in");
        return CallbackOutcome::SaveFailed.hand_off();
    }

    tracing::info!("cloud sign-in saved, starting the tunnel");
    state.reload_tx.send(ReloadSignal::Hub).ok();
    CallbackOutcome::Connecting.hand_off()
}

/// `POST /api/hub/cloud/disconnect` — disable cloud tunnel without removing the token.
pub(crate) async fn api_cloud_disconnect(State(state): State<CloudApiState>) -> Response {
    let config_path = state.hub_dir.join("config.toml");
    let current = match std::fs::read_to_string(&config_path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!(error = %e, path = %config_path.display(), "failed to read hub config.toml to disconnect the cloud tunnel");
            return disconnect_failed();
        }
    };
    if let Err(e) = std::fs::write(&config_path, update_cloud_section(&current, false)) {
        tracing::error!(error = %e, path = %config_path.display(), "failed to write hub config.toml to disconnect the cloud tunnel");
        return disconnect_failed();
    }

    tracing::info!("cloud tunnel disconnected");
    state.reload_tx.send(ReloadSignal::Hub).ok();

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// The API's `{ "error" }` body for a disconnect that couldn't change the hub's config.
fn disconnect_failed() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": "Residuum couldn't update its settings file to switch the tunnel off. The details are in Residuum's logs."
        })),
    )
        .into_response()
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

        // Preserve token line and relay_url from original
        for line in lines.get(start + 1..end).unwrap_or_default() {
            let trimmed = line.trim();
            if trimmed.starts_with("token") || trimmed.starts_with("relay_url") {
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
    fn parse_cloud_state_empty_token() {
        let toml = "[cloud]\nenabled = true\ntoken = \"\"\n";
        let (has_token, enabled) = parse_cloud_state(toml);
        assert!(
            !has_token,
            "empty token string should be treated as no token"
        );
        assert!(enabled);
    }

    #[test]
    fn parse_cloud_state_whitespace_token() {
        let toml = "[cloud]\nenabled = true\ntoken = \"   \"\n";
        let (has_token, enabled) = parse_cloud_state(toml);
        assert!(
            !has_token,
            "whitespace-only token should be treated as no token"
        );
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

    #[test]
    fn the_callback_takes_the_first_token_it_can_use() {
        assert_eq!(
            token_from_query(Some("token=rst_a")).as_deref(),
            Some("rst_a")
        );
        assert_eq!(
            token_from_query(Some("token=&token=rst_b&token=rst_c")).as_deref(),
            Some("rst_b")
        );
        assert_eq!(
            token_from_query(Some("token=rst%2Bplus")).as_deref(),
            Some("rst+plus")
        );
        assert_eq!(token_from_query(Some("token=%20%20")), None);
        assert_eq!(token_from_query(Some("%zz&&=x&token")), None);
        assert_eq!(token_from_query(None), None);
    }

    /// Where a callback answer sends the browser.
    fn hand_off_target(resp: &Response) -> &str {
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        resp.headers()["location"].to_str().unwrap()
    }

    #[tokio::test]
    async fn a_callback_that_cannot_save_hands_off_with_save_failed() {
        use axum::body::Body;
        use tower::ServiceExt;

        let hub_dir = tempfile::tempdir().unwrap();
        // A directory where the config file should be: the token saves, the switch can't.
        std::fs::create_dir(hub_dir.path().join("config.toml")).unwrap();
        let (reload_tx, mut reload_rx) = tokio::sync::mpsc::unbounded_channel::<ReloadSignal>();
        let (_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let sign_ins = super::super::cloud_sign_in::PendingSignIns::default();
        let sign_in_state = sign_ins.start().unwrap();
        let app = axum::Router::new()
            .route("/cloud/callback", axum::routing::get(cloud_callback))
            .with_state(CloudApiState {
                hub_dir: hub_dir.path().to_path_buf(),
                reload_tx,
                tunnel_status_rx,
                secret_lock: Arc::new(tokio::sync::Mutex::new(())),
                sign_ins,
            });

        let resp = app
            .oneshot(
                axum::http::Request::get(format!(
                    "/cloud/callback?token=rst_x&state={sign_in_state}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            hand_off_target(&resp),
            "/home?settings=_all/cloud&cloud=save-failed"
        );
        assert!(
            reload_rx.try_recv().is_err(),
            "nothing was switched on, so nothing reloads"
        );
    }

    fn test_state() -> CloudApiState {
        let (reload_tx, _reload_rx) = tokio::sync::mpsc::unbounded_channel::<ReloadSignal>();
        let (_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        CloudApiState {
            hub_dir: std::env::temp_dir(),
            reload_tx,
            tunnel_status_rx,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            sign_ins: super::super::cloud_sign_in::PendingSignIns::default(),
        }
    }

    fn status_router() -> axum::Router {
        axum::Router::new()
            .route("/api/cloud/status", axum::routing::get(api_cloud_status))
            .with_state(test_state())
    }

    #[tokio::test]
    async fn cloud_status_local_request_is_not_marked_as_tunnel() {
        use axum::body::Body;
        use tower::ServiceExt;

        let req = axum::http::Request::get("/api/cloud/status")
            .body(Body::empty())
            .unwrap();
        let resp = status_router().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed.get("viewed_via_tunnel"),
            Some(&serde_json::Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn cloud_status_tunnel_forwarded_request_is_marked_as_tunnel() {
        use axum::body::Body;
        use tower::ServiceExt;

        let req = axum::http::Request::get("/api/cloud/status")
            .extension(crate::pairing::remote::RemoteTransport::default())
            .body(Body::empty())
            .unwrap();
        let resp = status_router().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed.get("viewed_via_tunnel"),
            Some(&serde_json::Value::Bool(true))
        );
    }
}

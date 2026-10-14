//! Cloud tunnel API endpoints: status polling, OAuth callback, and disconnect.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Redirect, Response};
use serde::Serialize;
use tokio::sync::watch;

use super::config::{HubConfigPatchError, patch_hub_config};
use crate::checkpoints::CheckpointEngine;
use crate::config::HubConfig;
use crate::config::secrets::SecretStore;
use crate::gateway::types::ReloadSignal;
use crate::tunnel::TunnelStatus;

/// The `[cloud] token` value that names the sign-in token the callback stores
/// under the `cloud_token` secret.
const CLOUD_TOKEN_REF: &str = "secret:cloud_token";

/// Shared state for cloud API endpoints.
#[derive(Clone)]
pub(crate) struct CloudApiState {
    pub hub_dir: PathBuf,
    pub reload_tx: crate::gateway::types::ReloadSender,
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    /// The hub's checkpoint repositories, so the `config.toml` writes here
    /// can be undone from History like every other Settings save.
    pub checkpoints: Arc<CheckpointEngine>,
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
}

impl CallbackOutcome {
    fn param(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::MissingToken => "missing-token",
            Self::SaveFailed => "save-failed",
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

/// Write a `[cloud]` switch to `hub/config.toml` through the Settings patch
/// path, checkpointed as `summary`. The check is the first-run-tolerant one,
/// so the switch lands on any file the hub loads, including one whose
/// timezone is still unset. A stricter check would stop disconnect from
/// switching the tunnel off on such a file.
async fn switch_tunnel_in_config(
    state: &CloudApiState,
    switch: &serde_json::Value,
    summary: &str,
) -> Result<Option<String>, HubConfigPatchError> {
    patch_hub_config(
        &state.hub_dir,
        &state.checkpoints,
        switch,
        summary,
        |text| HubConfig::validate_toml_for_onboarding(text, &state.hub_dir),
    )
    .await
}

/// `GET /cloud/callback?token=...` — where the relay sends the browser after
/// sign-in.
///
/// Stores the token in the secret store, switches the tunnel on in the hub's
/// `config.toml`, triggers a config reload, and hands the browser to Settings
/// → Residuum Cloud, which follows the tunnel from there. Every outcome is a
/// redirect into the app; a failure is logged here and named by `cloud=`.
pub(crate) async fn cloud_callback(
    State(state): State<CloudApiState>,
    RawQuery(query): RawQuery,
) -> Response {
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

    let switch_on = serde_json::json!({ "cloud": { "enabled": true, "token": CLOUD_TOKEN_REF } });
    if let Err(e) = switch_tunnel_in_config(&state, &switch_on, "cloud sign-in").await {
        tracing::error!(error = %e, hub_dir = %hub_dir.display(), "failed to save the cloud sign-in to hub config.toml");
        return CallbackOutcome::SaveFailed.hand_off();
    }

    tracing::info!("cloud sign-in saved, starting the tunnel");
    state.reload_tx.send(ReloadSignal::Hub).ok();
    CallbackOutcome::Connecting.hand_off()
}

/// `POST /api/hub/cloud/disconnect` — disable cloud tunnel without removing the token.
pub(crate) async fn api_cloud_disconnect(State(state): State<CloudApiState>) -> Response {
    let switch_off = serde_json::json!({ "cloud": { "enabled": false } });
    let checkpoint_id = match switch_tunnel_in_config(&state, &switch_off, "cloud disconnect").await
    {
        Ok(checkpoint_id) => checkpoint_id,
        Err(e) => {
            tracing::error!(error = %e, hub_dir = %state.hub_dir.display(), "failed to switch the cloud tunnel off in hub config.toml");
            return disconnect_failed();
        }
    };

    tracing::info!("cloud tunnel disconnected");
    state.reload_tx.send(ReloadSignal::Hub).ok();

    Json(DisconnectResponse {
        ok: true,
        checkpoint_id,
    })
    .into_response()
}

/// The disconnect's success body. `checkpoint_id` is the checkpoint taken
/// before the switch, so a disconnect can be undone from History; it is
/// absent when that checkpoint could not be recorded.
#[derive(Serialize)]
struct DisconnectResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<String>,
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
        let app = axum::Router::new()
            .route("/cloud/callback", axum::routing::get(cloud_callback))
            .with_state(CloudApiState {
                hub_dir: hub_dir.path().to_path_buf(),
                reload_tx,
                tunnel_status_rx,
                secret_lock: Arc::new(tokio::sync::Mutex::new(())),
                checkpoints: crate::checkpoints::test_engine(),
            });

        let resp = app
            .oneshot(
                axum::http::Request::get("/cloud/callback?token=rst_x")
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
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    /// A hub config with comments and keys the Settings form doesn't model in
    /// `[cloud]`. A sign-in or disconnect must leave all of them as they are.
    const CONFIG_WITH_CLOUD_NOTES: &str = "\
# the hub's own notes
timezone = \"UTC\"

[cloud]
# the relay this hub dials
relay_url = \"wss://relay.example.test/tunnel/v2/register\"
local_port = 9000
enabled = false
";

    /// A hub whose `config.toml` holds `raw`, versioned by a checkpoint engine
    /// over that hub directory, as the running hub's engine is. The returned
    /// root keeps the directory alive.
    fn hub_with_config(raw: &str) -> (tempfile::TempDir, PathBuf, Arc<CheckpointEngine>) {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), raw).unwrap();
        let engine = CheckpointEngine::new(
            "test-agent",
            root.path().join("workspace"),
            &crate::config::paths::TeamPaths::new(root.path().join("team")),
            root.path().join("agent-config"),
            &hub_dir,
            &root.path().join("checkpoints"),
            None,
        )
        .unwrap();
        (root, hub_dir, Arc::new(engine))
    }

    fn state_over(
        hub_dir: &std::path::Path,
        checkpoints: Arc<CheckpointEngine>,
    ) -> (
        CloudApiState,
        tokio::sync::mpsc::UnboundedReceiver<ReloadSignal>,
    ) {
        let (reload_tx, reload_rx) = tokio::sync::mpsc::unbounded_channel::<ReloadSignal>();
        let (_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let state = CloudApiState {
            hub_dir: hub_dir.to_path_buf(),
            reload_tx,
            tunnel_status_rx,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints,
        };
        (state, reload_rx)
    }

    #[tokio::test]
    async fn cloud_sign_in_keeps_comments_and_keys_it_does_not_model() {
        let (_root, hub_dir, checkpoints) = hub_with_config(CONFIG_WITH_CLOUD_NOTES);
        let (state, mut reload_rx) = state_over(&hub_dir, checkpoints);

        let resp = cloud_callback(State(state), RawQuery(Some("token=rst_signed_in".into()))).await;
        assert_eq!(
            hand_off_target(&resp),
            "/home?settings=_all/cloud&cloud=connecting"
        );

        let config = std::fs::read_to_string(hub_dir.join("config.toml")).unwrap();
        assert!(config.contains("# the hub's own notes"), "{config}");
        assert!(config.contains("# the relay this hub dials"), "{config}");
        assert!(config.contains("local_port = 9000"), "{config}");
        assert!(
            config.contains("relay_url = \"wss://relay.example.test/tunnel/v2/register\""),
            "{config}"
        );
        assert!(config.contains("timezone = \"UTC\""), "{config}");
        assert_eq!(config.matches("[cloud]").count(), 1, "{config}");
        assert_eq!(parse_cloud_state(&config), (true, true), "{config}");
        assert!(matches!(reload_rx.try_recv(), Ok(ReloadSignal::Hub)));
    }

    #[tokio::test]
    async fn cloud_sign_in_checkpoints_the_config_as_it_stood() {
        let (_root, hub_dir, checkpoints) = hub_with_config(CONFIG_WITH_CLOUD_NOTES);
        let (state, _reload_rx) = state_over(&hub_dir, Arc::clone(&checkpoints));

        cloud_callback(State(state), RawQuery(Some("token=rst_signed_in".into()))).await;

        let page = checkpoints
            .list_checkpoints(crate::checkpoints::RepoKind::Hub, None, None, None, None)
            .await
            .unwrap();
        let newest = page
            .items
            .first()
            .expect("a sign-in should leave a checkpoint of the config");
        assert_eq!(newest.summary, "cloud sign-in");
        let stored = checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Hub,
                newest.id.clone(),
                "config.toml".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(CONFIG_WITH_CLOUD_NOTES.as_bytes()),
            "the checkpoint must hold config.toml as it was before the sign-in"
        );
    }

    #[tokio::test]
    async fn cloud_disconnect_switches_the_tunnel_off_and_leaves_the_rest_alone() {
        let raw = CONFIG_WITH_CLOUD_NOTES.replace(
            "enabled = false",
            "token = \"secret:cloud_token\"\nenabled = true",
        );
        let (_root, hub_dir, checkpoints) = hub_with_config(&raw);
        let (state, mut reload_rx) = state_over(&hub_dir, Arc::clone(&checkpoints));

        let resp = api_cloud_disconnect(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = parsed
            .get("checkpoint_id")
            .and_then(serde_json::Value::as_str)
            .expect("a disconnect should name the checkpoint taken before it")
            .to_string();

        let config = std::fs::read_to_string(hub_dir.join("config.toml")).unwrap();
        assert_eq!(
            config,
            raw.replace("enabled = true", "enabled = false"),
            "only the enabled switch may change"
        );
        assert_eq!(parse_cloud_state(&config), (true, false));

        let stored = checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Hub,
                id,
                "config.toml".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(raw.as_bytes()),
            "the checkpoint must hold config.toml as it was before the disconnect"
        );
        assert!(matches!(reload_rx.try_recv(), Ok(ReloadSignal::Hub)));
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

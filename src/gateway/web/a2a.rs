//! A2A web API endpoints: caller-key management, the client-side "remote
//! agents" endpoints (`GET /api/a2a/agents` for live status and
//! `GET`/`PUT /api/a2a/agents/raw` for the `config/a2a.json` editor), and
//! the settings page's `GET /api/a2a/status` and `GET /api/a2a/card`.
//!
//! Each request opens its own handle on the key store; writes are serialized
//! across handles and processes by the store's file lock, so the web UI,
//! the CLI, and the running A2A listener never lose each other's updates.

use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use crate::a2a::{
    A2aClientHub, A2aKeyError, A2aKeyInfo, A2aKeys, AUTH_CHECK_PATH, AgentCardFile, AgentSnapshot,
    AgentSource, AgentStatus, CardError, CardRuntime, build_agent_card,
};
use crate::config::{A2aConfig, A2aVisibility, DEFAULT_A2A_PORT};
use crate::workspace::layout::WorkspaceLayout;

use super::ConfigApiState;
use super::config::ValidateResponse;

/// Request body for `POST /api/a2a/keys`.
#[derive(Deserialize)]
pub(super) struct CreateA2aKeyRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response from `POST /api/a2a/keys`. Carries the token — shown only here,
/// once, and never again.
#[derive(Serialize)]
pub(super) struct CreateA2aKeyResponse {
    pub name: String,
    pub token: String,
}

/// Response from `GET /api/a2a/keys`.
#[derive(Serialize)]
pub(super) struct ListA2aKeysResponse {
    pub keys: Vec<A2aKeyInfo>,
}

/// Response from `DELETE /api/a2a/keys/{name}`.
#[derive(Serialize)]
pub(super) struct RevokeA2aKeyResponse {
    pub revoked: bool,
}

fn error_response(e: &A2aKeyError) -> (StatusCode, String) {
    let status = match e {
        A2aKeyError::Invalid(_) => StatusCode::BAD_REQUEST,
        A2aKeyError::NotFound(_) => StatusCode::NOT_FOUND,
        A2aKeyError::AlreadyExists(_) => StatusCode::CONFLICT,
        A2aKeyError::Storage(_) => {
            tracing::error!(error = %e, "A2A caller key store request failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (status, e.to_string())
}

/// `GET /api/a2a/keys` — list caller keys (metadata only, never tokens).
pub(super) async fn api_a2a_keys_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<ListA2aKeysResponse>, (StatusCode, String)> {
    let snapshot = A2aKeys::new(state.config_dir)
        .snapshot()
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(ListA2aKeysResponse {
        keys: snapshot.list(),
    }))
}

/// `POST /api/a2a/keys` — mint a caller key, returning the token once.
pub(super) async fn api_a2a_keys_create(
    State(state): State<ConfigApiState>,
    Json(req): Json<CreateA2aKeyRequest>,
) -> Result<Json<CreateA2aKeyResponse>, (StatusCode, String)> {
    let token = A2aKeys::new(state.config_dir)
        .create(&req.name, req.description.as_deref())
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(CreateA2aKeyResponse {
        name: req.name,
        token,
    }))
}

/// `DELETE /api/a2a/keys/{name}` — revoke a caller key.
pub(super) async fn api_a2a_keys_revoke(
    State(state): State<ConfigApiState>,
    Path(name): Path<String>,
) -> Result<Json<RevokeA2aKeyResponse>, (StatusCode, String)> {
    A2aKeys::new(state.config_dir)
        .revoke(&name)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(RevokeA2aKeyResponse { revoked: true }))
}

// ── Remote agents (client side) ─────────────────────────────────────────

/// Shared state for `GET /api/a2a/agents`.
#[derive(Clone)]
pub(crate) struct A2aAgentsStatusState {
    pub hub: Arc<A2aClientHub>,
}

/// Build the remote-agents status API router.
pub(crate) fn a2a_agents_status_router(state: A2aAgentsStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/a2a/agents", axum::routing::get(api_a2a_agents_list))
        .with_state(state)
}

/// One skill on a remote agent's card, in the shape the web UI wants.
#[derive(Serialize)]
pub(crate) struct A2aAgentSkillView {
    id: String,
    name: String,
}

/// One remote agent's card, in the shape the web UI wants.
#[derive(Serialize)]
pub(crate) struct A2aAgentCardView {
    name: String,
    description: String,
    skills: Vec<A2aAgentSkillView>,
}

/// One entry in `GET /api/a2a/agents`. `error` and `card` are always present
/// (as `null` when absent) rather than omitted, matching the web UI's
/// `string | null` / `Card | null` contract.
#[derive(Serialize)]
pub(crate) struct A2aAgentView {
    name: String,
    url: String,
    source: AgentSource,
    status: &'static str,
    error: Option<String>,
    card: Option<A2aAgentCardView>,
}

impl From<AgentSnapshot> for A2aAgentView {
    fn from(snapshot: AgentSnapshot) -> Self {
        let (status, error, card) = match &snapshot.status {
            AgentStatus::Pending => ("pending", None, None),
            AgentStatus::Ok(card) => (
                "ok",
                None,
                Some(A2aAgentCardView {
                    name: card.name.clone(),
                    description: card.description.clone(),
                    skills: card
                        .skills
                        .iter()
                        .map(|s| A2aAgentSkillView {
                            id: s.id.clone(),
                            name: s.name.clone(),
                        })
                        .collect(),
                }),
            ),
            AgentStatus::Error(e) => ("error", Some(e.clone()), None),
        };
        Self {
            name: snapshot.name,
            url: snapshot.url,
            source: snapshot.source,
            status,
            error,
            card,
        }
    }
}

/// `GET /api/a2a/agents` — every registered remote agent's live status.
pub(crate) async fn api_a2a_agents_list(
    State(state): State<A2aAgentsStatusState>,
) -> Json<Vec<A2aAgentView>> {
    let agents = state.hub.snapshot().await;
    Json(agents.into_iter().map(A2aAgentView::from).collect())
}

/// Default `config/a2a.json` content when the file doesn't exist yet.
const DEFAULT_A2A_AGENTS_JSON: &str = r#"{"agents":{}}"#;

/// `GET /api/a2a/agents/raw` — return raw `config/a2a.json` contents.
pub(super) async fn api_a2a_agents_raw_get(
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let path =
        crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    let contents = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_A2A_AGENTS_JSON.to_string(),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read a2a.json: {e}"),
            ));
        }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(contents))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("response build error: {e}"),
            )
        })
}

/// `PUT /api/a2a/agents/raw` — validate JSON + schema, write `config/a2a.json`
/// atomically, and trigger a workspace reload.
pub(super) async fn api_a2a_agents_raw_put(
    State(state): State<ConfigApiState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    crate::a2a::validate_a2a_agents_json(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ValidateResponse {
                valid: false,
                error: Some(e),
            }),
        )
    })?;

    let path =
        crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    crate::util::fs::atomic_write(&path, body.as_bytes())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write a2a.json: {e}")),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse {
        valid: true,
        error: None,
    }))
}

/// `[a2a]` settings as currently written in `config.toml`, resolved with the
/// same defaults `crate::config::resolve::resolve_a2a_config` uses. Parsed
/// standalone from the raw file — rather than through `Config::load_at`, which
/// validates the whole config — so an unrelated broken section elsewhere in
/// `config.toml` never breaks this status check.
struct A2aStatusConfig {
    enabled: bool,
    port: u16,
    public_url: Option<String>,
    visibility: A2aVisibility,
    gateway_bind: String,
}

impl Default for A2aStatusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_A2A_PORT,
            public_url: None,
            visibility: A2aVisibility::default(),
            gateway_bind: "127.0.0.1".to_string(),
        }
    }
}

fn read_a2a_status_config(config_dir: &FsPath) -> A2aStatusConfig {
    let default = A2aStatusConfig::default();
    let Ok(raw) = std::fs::read_to_string(config_dir.join("config.toml")) else {
        return default;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
        return default;
    };
    let a2a_table = value.get("a2a").and_then(toml::Value::as_table);
    let gateway_table = value.get("gateway").and_then(toml::Value::as_table);
    A2aStatusConfig {
        enabled: a2a_table
            .and_then(|t| t.get("enabled"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(default.enabled),
        port: a2a_table
            .and_then(|t| t.get("port"))
            .and_then(toml::Value::as_integer)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(default.port),
        public_url: a2a_table
            .and_then(|t| t.get("public_url"))
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        visibility: match a2a_table
            .and_then(|t| t.get("visibility"))
            .and_then(toml::Value::as_str)
        {
            Some("private") => A2aVisibility::Private,
            _ => A2aVisibility::Public,
        },
        gateway_bind: gateway_table
            .and_then(|t| t.get("bind"))
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .unwrap_or(default.gateway_bind),
    }
}

impl A2aStatusConfig {
    /// The `A2aConfig` shape [`CardRuntime::from_config`] expects, so the
    /// status/card endpoints compute the base URL exactly the way the running
    /// listener does.
    fn as_a2a_config(&self) -> A2aConfig {
        A2aConfig {
            enabled: self.enabled,
            port: self.port,
            public_url: self.public_url.clone(),
            visibility: self.visibility,
        }
    }
}

/// Whether something currently answers the A2A listener's health-check path
/// on loopback. A direct probe rather than reading in-process adapter state,
/// so this endpoint needs no wiring into the gateway's adapter lifecycle — it
/// answers the same question an outside caller would get.
async fn probe_listener_running(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    client
        .get(format!("http://127.0.0.1:{port}{AUTH_CHECK_PATH}"))
        .send()
        .await
        .is_ok()
}

/// Plain-language explanation of a broken workspace agent-card file, for the
/// settings page — never the raw `CardError` display, which names internal
/// error kinds a non-technical user has no use for.
fn plain_card_error(e: &CardError) -> String {
    match e {
        CardError::Read { path, .. } => format!(
            "Couldn't read the agent card file at {path}. Residuum writes one automatically \
             on first run, so if it's missing something else may have removed it."
        ),
        CardError::Parse { path, message } => {
            format!("The agent card file at {path} isn't valid JSON: {message}")
        }
        CardError::Invalid { path, message } => {
            format!("The agent card file at {path} has a problem: {message}")
        }
    }
}

/// Response body for `GET /api/a2a/status`.
#[derive(Serialize)]
pub(super) struct A2aStatusResponse {
    enabled: bool,
    port: u16,
    visibility: &'static str,
    public_url: Option<String>,
    listener_running: bool,
    card_error: Option<String>,
}

/// State for `GET /api/a2a/status` and `GET /api/a2a/card`: the config API
/// state plus the live tunnel status, so both report the same public URL the
/// listener advertises.
#[derive(Clone)]
pub(crate) struct A2aStatusApiState {
    pub config: ConfigApiState,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
}

/// Build the router for the settings page's A2A status and card endpoints.
pub(crate) fn a2a_status_router(state: A2aStatusApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/a2a/status", axum::routing::get(api_a2a_status))
        .route("/api/a2a/card", axum::routing::get(api_a2a_card))
        .with_state(state)
}

/// `GET /api/a2a/status` — whether A2A is on, how other agents can reach it,
/// and whether the listener or the workspace agent card currently have a
/// problem.
///
/// `public_url` is the configured `[a2a] public_url` when set, and `null`
/// otherwise. Once the relay tunnel can supply an address automatically,
/// filling this in from the tunnel status instead is a one-line change here.
pub(super) async fn api_a2a_status(
    State(state): State<A2aStatusApiState>,
) -> Json<A2aStatusResponse> {
    let cfg = read_a2a_status_config(&state.config.config_dir);
    let listener_running = if cfg.enabled {
        probe_listener_running(cfg.port).await
    } else {
        false
    };
    let card_path = WorkspaceLayout::new(&state.config.workspace_dir).agent_card_json();
    let card_error = AgentCardFile::load(&card_path)
        .err()
        .map(|e| plain_card_error(&e));

    Json(A2aStatusResponse {
        enabled: cfg.enabled,
        port: cfg.port,
        visibility: cfg.visibility.as_str(),
        public_url: crate::a2a::public_url::known_a2a_public_url(
            &cfg.as_a2a_config(),
            &state.tunnel_status_rx.borrow(),
        ),
        listener_running,
        card_error,
    })
}

/// `GET /api/a2a/card` — the Agent Card as the listener would currently serve
/// it, or a `503` with a plain-language error if the workspace
/// `agent-card.json` file is invalid.
pub(super) async fn api_a2a_card(
    State(state): State<A2aStatusApiState>,
) -> Result<Json<a2a::AgentCard>, (StatusCode, String)> {
    let cfg = read_a2a_status_config(&state.config.config_dir);
    let card_path = WorkspaceLayout::new(&state.config.workspace_dir).agent_card_json();
    let file = AgentCardFile::load(&card_path)
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, plain_card_error(&e)))?;
    let runtime = CardRuntime::from_config_and_tunnel(
        &cfg.as_a2a_config(),
        &cfg.gateway_bind,
        &state.tunnel_status_rx.borrow(),
    );
    Ok(Json(build_agent_card(&file, &runtime)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_state(config: ConfigApiState) -> A2aStatusApiState {
        let (_tx, rx) = tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);
        A2aStatusApiState {
            config,
            tunnel_status_rx: rx,
        }
    }

    fn test_state(dir: &std::path::Path) -> ConfigApiState {
        ConfigApiState {
            config_dir: dir.to_path_buf(),
            workspace_dir: dir.join("workspace"),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[tokio::test]
    async fn create_list_revoke_roundtrip_returns_token_only_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let created = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: Some("my other instance".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(created.name, "laptop");
        assert!(created.token.starts_with("rsdm_a2a_"));

        let list = api_a2a_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = serde_json::to_string(&list.0).unwrap();
        assert!(body.contains("my other instance"), "{body}");
        assert!(
            !body.contains(&created.token),
            "listing must never carry the token"
        );

        let revoked =
            api_a2a_keys_revoke(State(test_state(dir.path())), Path("laptop".to_string()))
                .await
                .unwrap();
        assert!(revoked.revoked);
        let after = api_a2a_keys_list(State(test_state(dir.path())))
            .await
            .unwrap();
        assert!(after.keys.is_empty());
    }

    #[tokio::test]
    async fn invalid_name_is_bad_request_and_unknown_revoke_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "Bad Name".to_string(),
                description: None,
            }),
        )
        .await
        else {
            panic!("invalid name should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let Err((revoke_status, _)) =
            api_a2a_keys_revoke(State(test_state(dir.path())), Path("nope".to_string())).await
        else {
            panic!("unknown key revoke should fail");
        };
        assert_eq!(revoke_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_name_is_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let _first = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let Err((status, _)) = api_a2a_keys_create(
            State(test_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        else {
            panic!("duplicate name should be rejected");
        };
        assert_eq!(status, StatusCode::CONFLICT);
    }

    // ── Remote agents ────────────────────────────────────────────────

    #[tokio::test]
    async fn agents_list_reports_status_and_card() {
        let hub = crate::a2a::A2aClientHub::new_shared();
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            crate::a2a::AgentSource::Config,
        )
        .await;
        let Json(agents) = api_a2a_agents_list(State(A2aAgentsStatusState {
            hub: Arc::clone(&hub),
        }))
        .await;
        let [only] = agents.as_slice() else {
            panic!("expected exactly one agent");
        };
        assert_eq!(only.name, "laptop");
        assert_eq!(
            only.status, "error",
            "an unreachable agent reports error status"
        );
        assert!(only.error.is_some());
        assert!(only.card.is_none());
    }

    #[tokio::test]
    async fn agents_raw_get_returns_default_template_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resp = api_a2a_agents_raw_get(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), DEFAULT_A2A_AGENTS_JSON.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_writes_file_and_get_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"laptop":{"url":"https://laptop.example.com"}}}"#;
        let result = api_a2a_agents_raw_put(State(test_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(result.valid);

        let resp = api_a2a_agents_raw_get(State(test_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), content.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_rejects_invalid_json_with_plain_language_error() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, Json(result))) =
            api_a2a_agents_raw_put(State(test_state(dir.path())), "not json".to_string()).await
        else {
            panic!("invalid JSON should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!result.valid);
        assert!(result.error.unwrap().contains("invalid JSON"));
    }

    #[tokio::test]
    async fn agents_raw_put_rejects_bad_agent_name() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"Bad Name":{"url":"https://x.example.com"}}}"#;
        let Err((status, Json(result))) =
            api_a2a_agents_raw_put(State(test_state(dir.path())), content.to_string()).await
        else {
            panic!("bad agent name should be rejected");
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!result.valid);
    }

    fn write_config(dir: &std::path::Path, toml: &str) {
        std::fs::write(dir.join("config.toml"), toml).unwrap();
    }

    fn write_card(state: &ConfigApiState, json: &str) {
        let path = WorkspaceLayout::new(&state.workspace_dir).agent_card_json();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, json).unwrap();
    }

    const VALID_CARD: &str =
        r#"{"name": "Test Agent", "description": "does things", "skills": []}"#;

    async fn free_port() -> u16 {
        tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn status_defaults_when_config_and_card_are_missing() {
        let dir = tempfile::tempdir().unwrap();
        let status = api_a2a_status(State(status_state(test_state(dir.path()))))
            .await
            .0;
        assert!(status.enabled, "a2a is enabled by default");
        assert_eq!(status.port, DEFAULT_A2A_PORT);
        assert_eq!(status.visibility, "public");
        assert_eq!(status.public_url, None);
        // `listener_running` is left unasserted: it probes the real default
        // port, which any Residuum running on this machine may hold.
        assert!(
            status.card_error.is_some(),
            "a missing agent card file should surface as an error, not silently succeed"
        );
    }

    #[tokio::test]
    async fn status_reports_no_listener_when_nothing_answers_the_port() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        write_config(dir.path(), &format!("[a2a]\nport = {port}\n"));
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);

        let status = api_a2a_status(State(status_state(state))).await.0;
        assert!(status.enabled);
        assert!(
            !status.listener_running,
            "nothing listens on a freshly freed port"
        );
    }

    #[tokio::test]
    async fn status_reads_public_url_and_private_visibility_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        write_config(
            dir.path(),
            &format!(
                "[a2a]\nenabled = true\nport = {port}\nvisibility = \"private\"\n\
                 public_url = \"https://example.com/a2a/laptop\"\n"
            ),
        );
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);

        let status = api_a2a_status(State(status_state(state))).await.0;
        assert_eq!(status.port, port);
        assert_eq!(status.visibility, "private");
        assert_eq!(
            status.public_url.as_deref(),
            Some("https://example.com/a2a/laptop")
        );
        assert!(status.card_error.is_none());
    }

    #[tokio::test]
    async fn status_reports_listener_running_when_something_answers_auth_check() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        write_config(
            dir.path(),
            &format!("[a2a]\nenabled = true\nport = {port}\n"),
        );
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);

        let router = axum::Router::new().route(
            AUTH_CHECK_PATH,
            axum::routing::get(|| async { StatusCode::NO_CONTENT }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let status = api_a2a_status(State(status_state(state))).await.0;
        assert!(status.listener_running);
    }

    #[tokio::test]
    async fn status_never_probes_a_disabled_listener() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        // Nothing listens on `port`: if the handler probed it anyway despite
        // `enabled = false`, it would (correctly) report `false` too, so this
        // only exercises the disabled branch's own reported fields.
        write_config(
            dir.path(),
            &format!("[a2a]\nenabled = false\nport = {port}\n"),
        );
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);

        let status = api_a2a_status(State(status_state(state))).await.0;
        assert!(!status.enabled);
        assert!(!status.listener_running);
    }

    #[tokio::test]
    async fn card_endpoint_returns_the_served_card_when_valid() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().await;
        write_config(
            dir.path(),
            &format!("[a2a]\nenabled = true\nport = {port}\n"),
        );
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);

        let card = api_a2a_card(State(status_state(state))).await.unwrap().0;
        assert_eq!(card.name, "Test Agent");
        assert!(card.skills.is_empty());
    }

    #[tokio::test]
    async fn card_endpoint_is_503_with_a_plain_message_when_the_card_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        write_card(&state, "not json");

        let Err((status, message)) = api_a2a_card(State(status_state(state))).await else {
            panic!("an invalid card file should be rejected");
        };
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            message.contains("agent card"),
            "error should name the problem in plain language: {message}"
        );
    }

    #[tokio::test]
    async fn card_endpoint_is_503_when_the_card_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_a2a_card(State(status_state(test_state(dir.path())))).await
        else {
            panic!("a missing card file should be rejected");
        };
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn status_reports_the_relay_url_while_the_tunnel_is_connected() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: Some("https://bear.agent-residuum.com".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: None,
        });
        let state = A2aStatusApiState {
            config: test_state(dir.path()),
            tunnel_status_rx: rx,
        };
        let status = api_a2a_status(State(state)).await.0;
        assert_eq!(
            status.public_url.as_deref(),
            Some("https://bear.agent-residuum.com/a2a/laptop"),
            "the relay URL is the public address while the tunnel is connected"
        );
    }
}

//! A2A web API endpoints: caller-key management, the client-side "remote
//! agents" endpoints (`GET /api/agents/{name}/a2a/agents` for live status and
//! `GET`/`PUT /api/agents/{name}/a2a/agents/raw` for the `config/a2a.json` editor), the
//! Activity's tasks sent to remote agents (`GET /api/agents/{name}/a2a/outbound`
//! and the stop endpoints under it), and the Agent-to-agent settings'
//! `GET /api/agents/{name}/a2a/status` and `GET /api/agents/{name}/a2a/card`.
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
    AgentSource, AgentStatus, CardError, CardRuntime, RemoteTaskTracker, build_agent_card,
};
use crate::config::{A2aConfig, A2aVisibility, DEFAULT_A2A_PORT};
use crate::gateway::protocol::OutboundA2aTaskSummary;
use crate::tunnel::TunnelStatus;
use crate::workspace::layout::WorkspaceLayout;

use super::config::ValidateResponse;
use super::{AgentFilesState, ConfigApiState, HubApiState};

/// Request body for `POST /api/hub/a2a/keys`.
#[derive(Deserialize)]
pub(super) struct CreateA2aKeyRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Response from `POST /api/hub/a2a/keys`. Carries the token — shown only here,
/// once, and never again.
#[derive(Serialize)]
pub(super) struct CreateA2aKeyResponse {
    pub name: String,
    pub token: String,
}

/// Response from `GET /api/hub/a2a/keys`.
#[derive(Serialize)]
pub(super) struct ListA2aKeysResponse {
    pub keys: Vec<A2aKeyInfo>,
}

/// Response from `DELETE /api/hub/a2a/keys/{name}`.
#[derive(Serialize)]
pub(super) struct RevokeA2aKeyResponse {
    pub revoked: bool,
    /// Checkpoint holding the caller-key store as it was before this revoke.
    /// `None` when that checkpoint could not be recorded; the revoke still
    /// succeeded, and the UI should not offer Undo.
    pub checkpoint_id: Option<String>,
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

/// `GET /api/hub/a2a/keys` — list caller keys (metadata only, never tokens).
pub(super) async fn api_a2a_keys_list(
    State(state): State<HubApiState>,
) -> Result<Json<ListA2aKeysResponse>, (StatusCode, String)> {
    let snapshot = A2aKeys::new(state.hub_dir)
        .snapshot()
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(ListA2aKeysResponse {
        keys: snapshot.list(),
    }))
}

/// `POST /api/hub/a2a/keys` — mint a caller key, returning the token once.
pub(super) async fn api_a2a_keys_create(
    State(state): State<HubApiState>,
    Json(req): Json<CreateA2aKeyRequest>,
) -> Result<Json<CreateA2aKeyResponse>, (StatusCode, String)> {
    state
        .checkpoint_config_before_write(format!("create a2a key '{}'", req.name))
        .await;
    let token = A2aKeys::new(state.hub_dir)
        .create(&req.name, req.description.as_deref())
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(CreateA2aKeyResponse {
        name: req.name,
        token,
    }))
}

/// `DELETE /api/hub/a2a/keys/{name}` — revoke a caller key.
pub(super) async fn api_a2a_keys_revoke(
    State(state): State<HubApiState>,
    Path(name): Path<String>,
) -> Result<Json<RevokeA2aKeyResponse>, (StatusCode, String)> {
    let checkpoint_id = state
        .checkpoint_config_id_before_write(format!("revoke a2a key '{name}'"))
        .await;
    A2aKeys::new(state.hub_dir)
        .revoke(&name)
        .await
        .map_err(|e| error_response(&e))?;
    Ok(Json(RevokeA2aKeyResponse {
        revoked: true,
        checkpoint_id,
    }))
}

// ── Remote agents (client side) ─────────────────────────────────────────

/// Shared state for `GET /api/agents/{name}/a2a/agents` and the outbound-task endpoints.
#[derive(Clone)]
pub(crate) struct A2aAgentsStatusState {
    pub hub: Arc<A2aClientHub>,
    pub tracker: Arc<RemoteTaskTracker>,
}

/// Build the remote-agents status and outbound-task API router.
pub(crate) fn a2a_agents_status_router(state: A2aAgentsStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/a2a/agents", axum::routing::get(api_a2a_agents_list))
        .route(
            "/api/a2a/outbound",
            axum::routing::get(api_a2a_outbound_list),
        )
        .route(
            "/api/a2a/outbound/{task_id}/stop",
            axum::routing::post(api_a2a_outbound_stop),
        )
        .route(
            "/api/a2a/outbound/{task_id}/stop-watching",
            axum::routing::post(api_a2a_outbound_stop_watching),
        )
        .with_state(state)
}

/// Error body for the outbound-task endpoints: a message for the user and a
/// machine-readable `code` (`not_open`, or `unreachable` when the agent
/// couldn't be reached to cancel the task — the web UI then offers
/// stop-watching instead).
#[derive(Debug, Serialize)]
pub(crate) struct OutboundTaskError {
    error: String,
    code: &'static str,
}

fn outbound_not_open(task_id: &str) -> (StatusCode, Json<OutboundTaskError>) {
    (
        StatusCode::NOT_FOUND,
        Json(OutboundTaskError {
            error: format!("Task {task_id} isn't running anymore, so there's nothing to stop."),
            code: "not_open",
        }),
    )
}

/// `GET /api/agents/{name}/a2a/outbound` — every open task sent to a remote agent, newest
/// first.
pub(crate) async fn api_a2a_outbound_list(
    State(state): State<A2aAgentsStatusState>,
) -> Json<Vec<OutboundA2aTaskSummary>> {
    let tasks = state.tracker.open_tasks().await;
    Json(tasks.iter().map(OutboundA2aTaskSummary::from).collect())
}

/// `POST /api/agents/{name}/a2a/outbound/{task_id}/stop` — ask the remote agent to cancel
/// the task, the same cancel the agent's own `stop_agent a2a:<name>` makes.
///
/// # Errors
/// `404` (`not_open`) when no open task has that id; `502` (`unreachable`)
/// when its agent can't be reached to cancel it.
pub(crate) async fn api_a2a_outbound_stop(
    State(state): State<A2aAgentsStatusState>,
    Path(task_id): Path<String>,
) -> Result<Json<OutboundA2aTaskSummary>, (StatusCode, Json<OutboundTaskError>)> {
    match state.tracker.stop_task(&task_id).await {
        Ok(Some(task)) => Ok(Json(OutboundA2aTaskSummary::from(&task))),
        Ok(None) => Err(outbound_not_open(&task_id)),
        Err(e) => {
            tracing::warn!(task_id, error = %e, "user stop of a2a remote task failed");
            Err((
                StatusCode::BAD_GATEWAY,
                Json(OutboundTaskError {
                    error: format!(
                        "Couldn't reach {} to cancel the task. You can stop watching it instead; it \
                         may keep running on their side.",
                        e.agent_name()
                    ),
                    code: "unreachable",
                }),
            ))
        }
    }
}

/// `POST /api/agents/{name}/a2a/outbound/{task_id}/stop-watching` — close the task locally
/// without reaching its agent, ending the retries and their notices.
///
/// # Errors
/// `404` (`not_open`) when no open task has that id.
pub(crate) async fn api_a2a_outbound_stop_watching(
    State(state): State<A2aAgentsStatusState>,
    Path(task_id): Path<String>,
) -> Result<Json<OutboundA2aTaskSummary>, (StatusCode, Json<OutboundTaskError>)> {
    state
        .tracker
        .stop_watching(&task_id)
        .await
        .map(|task| Json(OutboundA2aTaskSummary::from(&task)))
        .ok_or_else(|| outbound_not_open(&task_id))
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

/// One entry in `GET /api/agents/{name}/a2a/agents`. `error` and `card` are always present
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

/// `GET /api/agents/{name}/a2a/agents` — every registered remote agent's live status.
pub(crate) async fn api_a2a_agents_list(
    State(state): State<A2aAgentsStatusState>,
) -> Json<Vec<A2aAgentView>> {
    let agents = state.hub.snapshot().await;
    Json(agents.into_iter().map(A2aAgentView::from).collect())
}

/// Default `config/a2a.json` content when the file doesn't exist yet.
const DEFAULT_A2A_AGENTS_JSON: &str = r#"{"agents":{}}"#;

/// `GET /api/agents/{name}/a2a/agents/raw` — return raw `config/a2a.json` contents.
pub(super) async fn api_a2a_agents_raw_get(
    State(state): State<AgentFilesState>,
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

/// `PUT /api/agents/{name}/a2a/agents/raw` — write `config/a2a.json` atomically, save
/// unconditionally, trigger a workspace reload, and report diagnostics.
///
/// The save always succeeds, even when `body` fails validation: the loader
/// skips an unusable agent entry with a warning and keeps every other agent
/// running (see `crate::a2a::client::config::load_a2a_agents_map`), so an
/// invalid save is safe to accept and report rather than reject outright —
/// consistent with `write_file`/`edit_file`, `config.toml`/`providers.toml`,
/// and the workspace file editor.
pub(super) async fn api_a2a_agents_raw_put(
    State(state): State<AgentFilesState>,
    body: String,
) -> Result<Json<ValidateResponse>, (StatusCode, Json<ValidateResponse>)> {
    let diagnostics = crate::a2a::client::config::diagnose_a2a_json(&body);

    let path =
        crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    state
        .checkpoint_workspace_before_write("raw write a2a.json")
        .await;

    crate::util::fs::atomic_write(&path, body.as_bytes())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ValidateResponse {
                    valid: false,
                    error: Some(format!("failed to write a2a.json: {e}")),
                    diagnostics: Vec::new(),
                }),
            )
        })?;

    if let Some(reload_tx) = &state.reload_tx {
        reload_tx.send(super::super::ReloadSignal::Workspace).ok();
    }

    Ok(Json(ValidateResponse::from_diagnostics(
        diagnostics,
        "a2a.json",
    )))
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

/// Read the A2A settings the status endpoints report: the listener's
/// `enabled`/`port`/`public_url` and the gateway bind from the hub config,
/// and this agent's `visibility` from its own config. Reads the raw TOML
/// rather than a resolved config so a file that fails full validation still
/// reports what it does say.
fn read_a2a_status_config(hub_dir: &FsPath, agent_config_dir: &FsPath) -> A2aStatusConfig {
    let default = A2aStatusConfig::default();
    let read_table = |path: std::path::PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
    };
    let hub_value = read_table(hub_dir.join("config.toml"));
    let agent_value = read_table(agent_config_dir.join("config.toml"));
    let hub_a2a = hub_value
        .as_ref()
        .and_then(|v| v.get("a2a"))
        .and_then(toml::Value::as_table);
    let gateway_table = hub_value
        .as_ref()
        .and_then(|v| v.get("gateway"))
        .and_then(toml::Value::as_table);
    let agent_a2a = agent_value
        .as_ref()
        .and_then(|v| v.get("a2a"))
        .and_then(toml::Value::as_table);
    A2aStatusConfig {
        enabled: hub_a2a
            .and_then(|t| t.get("enabled"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(default.enabled),
        port: hub_a2a
            .and_then(|t| t.get("port"))
            .and_then(toml::Value::as_integer)
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(default.port),
        public_url: hub_a2a
            .and_then(|t| t.get("public_url"))
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        visibility: match agent_a2a
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
/// Agent-to-agent settings — never the raw `CardError` display, which names internal
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

/// What [`api_a2a_status`] reports about how far the agent can be reached.
fn relay_access_note(has_own_address: bool, relay_connected: bool) -> &'static str {
    match (relay_connected, has_own_address) {
        (true, true) => {
            "Reachable through the Residuum relay and at your own address. Your other Residuum installs find it automatically; anyone else needs a caller key."
        }
        (true, false) => {
            "Reachable through the Residuum relay. Your other Residuum installs find it automatically; anyone else needs a caller key."
        }
        (false, true) => {
            "Reachable locally and at your own address. Connect to the Residuum relay in Settings → All agents → Residuum Cloud to make it reachable through the relay too."
        }
        (false, false) => {
            "Reachable locally. Connect to the Residuum relay in Settings → All agents → Residuum Cloud to make it reachable from other places, or enter your own address in Settings → All agents → Agent-to-agent if you run your own tunnel."
        }
    }
}

/// Response body for `GET /api/agents/{name}/a2a/status`.
#[derive(Serialize)]
pub(super) struct A2aStatusResponse {
    enabled: bool,
    port: u16,
    visibility: &'static str,
    /// The address other agents reach this one at from outside this machine:
    /// under the user's own `[a2a] public_url` when set, otherwise through
    /// the relay while the tunnel is connected, otherwise `None`.
    public_url: Option<String>,
    /// This agent's address on the local A2A listener.
    local_url: String,
    /// Whether other Residuum installs can reach this agent through the relay
    /// right now: A2A is enabled and the tunnel is connected.
    relay_access: bool,
    /// Plain-language statement of how far the agent can be reached.
    relay_access_note: &'static str,
    listener_running: bool,
    card_error: Option<String>,
}

/// State for the Agent-to-agent settings' status and card endpoints: the agent's
/// config and the relay tunnel's status.
#[derive(Clone)]
pub(crate) struct A2aStatusState {
    pub config: ConfigApiState,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

/// Build the router for the Agent-to-agent settings' status and card endpoints.
pub(crate) fn a2a_status_router(state: A2aStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/a2a/status", axum::routing::get(api_a2a_status))
        .route("/api/a2a/card", axum::routing::get(api_a2a_card))
        .with_state(state)
}

/// `GET /api/agents/{name}/a2a/status` — whether A2A is on, how other agents can reach it,
/// and whether the listener or the workspace agent card currently have a
/// problem.
///
/// `public_url` is the agent's address under the configured `[a2a]
/// public_url` when set, else its relay address (`{origin}/a2a/{instance}/{name}`)
/// while the tunnel is connected, and `null` otherwise; `local_url` is the
/// address on the local listener.
pub(super) async fn api_a2a_status(State(state): State<A2aStatusState>) -> Json<A2aStatusResponse> {
    let config = &state.config;
    let cfg = read_a2a_status_config(&config.hub_dir, &config.config_dir);
    let listener_running = if cfg.enabled {
        probe_listener_running(cfg.port).await
    } else {
        false
    };
    let card_path = WorkspaceLayout::new(&config.workspace_dir).agent_card_json();
    let card_error = AgentCardFile::load(&card_path)
        .err()
        .map(|e| plain_card_error(&e));

    let own_url =
        crate::a2a::public_url::known_a2a_public_url(&cfg.as_a2a_config(), &config.agent_name);
    let relay_url = crate::a2a::public_url::relay_a2a_base(&state.tunnel_status_rx.borrow())
        .map(|base| crate::a2a::public_url::relay_agent_url(&base, &config.agent_name));
    let relay_access = cfg.enabled && relay_url.is_some();

    Json(A2aStatusResponse {
        enabled: cfg.enabled,
        port: cfg.port,
        visibility: cfg.visibility.as_str(),
        relay_access_note: relay_access_note(own_url.is_some(), relay_access),
        public_url: own_url.or(relay_url),
        local_url: format!(
            "http://{}:{}{}/{}",
            cfg.gateway_bind,
            cfg.port,
            crate::a2a::public_url::AGENTS_PATH_PREFIX,
            config.agent_name
        ),
        relay_access,
        listener_running,
        card_error,
    })
}

/// `GET /api/agents/{name}/a2a/card` — the Agent Card as the listener would currently serve
/// it, or a `503` with a plain-language error if the workspace
/// `agent-card.json` file is invalid.
pub(super) async fn api_a2a_card(
    State(state): State<A2aStatusState>,
) -> Result<Json<a2a::AgentCard>, (StatusCode, String)> {
    let config = &state.config;
    let cfg = read_a2a_status_config(&config.hub_dir, &config.config_dir);
    let card_path = WorkspaceLayout::new(&config.workspace_dir).agent_card_json();
    let file = AgentCardFile::load(&card_path)
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, plain_card_error(&e)))?;
    let relay_base = crate::a2a::public_url::relay_a2a_base(&state.tunnel_status_rx.borrow());
    let runtime = CardRuntime::from_config(
        &cfg.as_a2a_config(),
        &cfg.gateway_bind,
        &config.agent_name,
        relay_base.as_deref(),
    );
    Ok(Json(build_agent_card(&file, &runtime)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_ports::free_port;

    fn hub_state(dir: &std::path::Path) -> HubApiState {
        HubApiState::for_test(dir)
    }

    fn status_state(config: ConfigApiState) -> A2aStatusState {
        status_state_with(config, TunnelStatus::Disconnected)
    }

    fn status_state_with(config: ConfigApiState, tunnel: TunnelStatus) -> A2aStatusState {
        A2aStatusState {
            config,
            tunnel_status_rx: tokio::sync::watch::channel(tunnel).1,
        }
    }

    fn relay_connected() -> TunnelStatus {
        TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: Some("https://bear.agent-residuum.com".to_string()),
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: None,
            instance_origin: None,
        }
    }

    fn files_state(dir: &std::path::Path) -> AgentFilesState {
        AgentFilesState::for_test(dir.join("workspace"), None)
    }

    fn test_state(dir: &std::path::Path) -> ConfigApiState {
        ConfigApiState {
            team: None,
            hub_dir: dir.to_path_buf(),
            config_dir: dir.to_path_buf(),
            agent_name: "test-agent".to_string(),
            workspace_dir: dir.join("workspace"),
            memory_dir: None,
            reload_tx: None,
            scope: crate::gateway::web::WorkspaceScope::Agent,
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    #[tokio::test]
    async fn create_list_revoke_roundtrip_returns_token_only_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let created = api_a2a_keys_create(
            State(hub_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: Some("my other instance".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(created.name, "laptop");
        assert!(created.token.starts_with("rsdm_a2a_"));

        let list = api_a2a_keys_list(State(hub_state(dir.path())))
            .await
            .unwrap();
        let body = serde_json::to_string(&list.0).unwrap();
        assert!(body.contains("my other instance"), "{body}");
        assert!(
            !body.contains(&created.token),
            "listing must never carry the token"
        );

        let revoked = api_a2a_keys_revoke(State(hub_state(dir.path())), Path("laptop".to_string()))
            .await
            .unwrap();
        assert!(revoked.revoked);
        let after = api_a2a_keys_list(State(hub_state(dir.path())))
            .await
            .unwrap();
        assert!(after.keys.is_empty());
    }

    #[tokio::test]
    async fn revoke_returns_the_checkpoint_taken_before_the_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_hub_state(dir.path());
        let _created = api_a2a_keys_create(
            State(state.clone()),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let revoked = api_a2a_keys_revoke(State(state.clone()), Path("laptop".to_string()))
            .await
            .unwrap();
        let id = revoked
            .checkpoint_id
            .clone()
            .expect("revoke should name the checkpoint taken before it");
        let stored = state
            .checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Hub,
                id.clone(),
                "a2a-keys.toml".to_string(),
            )
            .await
            .unwrap();
        assert!(
            stored.is_some(),
            "the returned checkpoint must still contain the caller-key file"
        );

        std::fs::write(state.hub_dir.join("config.toml"), "later = true").unwrap();
        let later = state
            .checkpoint_config_id_before_write("later config write")
            .await
            .expect("a later checkpoint should be recorded");
        assert_ne!(
            later, id,
            "the id returned for Undo stays the pre-revoke checkpoint after a newer one is taken"
        );
    }

    #[tokio::test]
    async fn invalid_name_is_bad_request_and_unknown_revoke_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let Err((status, _)) = api_a2a_keys_create(
            State(hub_state(dir.path())),
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
            api_a2a_keys_revoke(State(hub_state(dir.path())), Path("nope".to_string())).await
        else {
            panic!("unknown key revoke should fail");
        };
        assert_eq!(revoke_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_name_is_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let _first = api_a2a_keys_create(
            State(hub_state(dir.path())),
            Json(CreateA2aKeyRequest {
                name: "laptop".to_string(),
                description: None,
            }),
        )
        .await
        .unwrap();

        let Err((status, _)) = api_a2a_keys_create(
            State(hub_state(dir.path())),
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

    async fn agents_state(hub: &Arc<A2aClientHub>, dir: &FsPath) -> A2aAgentsStatusState {
        let bus = crate::bus::spawn_broker();
        let messenger = Arc::new(crate::background::messaging::AgentMessenger::new(
            Arc::new(crate::background::registry::SessionRegistry::new()),
            bus.publisher(),
            Arc::new(crate::background::store::SessionStore::new(
                dir.join("sessions"),
            )),
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let tracker = RemoteTaskTracker::load(
            dir.join("outbound.json"),
            Arc::clone(hub),
            messenger,
            dir.join("inbox"),
        )
        .await;
        A2aAgentsStatusState {
            hub: Arc::clone(hub),
            tracker,
        }
    }

    async fn unreachable_laptop_with_task(dir: &FsPath) -> A2aAgentsStatusState {
        let hub = crate::a2a::A2aClientHub::new_shared();
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            std::collections::HashMap::new(),
            crate::a2a::AgentSource::Config,
        )
        .await;
        let state = agents_state(&hub, dir).await;
        state
            .tracker
            .track(
                &crate::bus::SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;
        state
    }

    #[tokio::test]
    async fn outbound_list_shows_open_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let state = unreachable_laptop_with_task(dir.path()).await;
        let Json(tasks) = api_a2a_outbound_list(State(state)).await;
        let [only] = tasks.as_slice() else {
            panic!("expected exactly one task");
        };
        assert_eq!(only.task_id, "t1");
        assert_eq!(only.agent, "laptop");
        assert_eq!(only.state, "working");
        assert!(only.open);
    }

    #[tokio::test]
    async fn outbound_stop_reports_unreachable_then_stop_watching_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = unreachable_laptop_with_task(dir.path()).await;

        let Err((status, Json(err))) =
            api_a2a_outbound_stop(State(state.clone()), Path("t1".to_string())).await
        else {
            panic!("an unreachable agent can't be asked to cancel");
        };
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(err.code, "unreachable");
        assert!(err.error.contains("laptop"), "got: {}", err.error);

        let Json(closed) =
            api_a2a_outbound_stop_watching(State(state.clone()), Path("t1".to_string()))
                .await
                .unwrap();
        assert!(!closed.open);
        let Json(tasks) = api_a2a_outbound_list(State(state.clone())).await;
        assert!(tasks.is_empty());

        let Err((gone_status, Json(gone_err))) =
            api_a2a_outbound_stop_watching(State(state), Path("t1".to_string())).await
        else {
            panic!("a closed task has nothing to stop");
        };
        assert_eq!(gone_status, StatusCode::NOT_FOUND);
        assert_eq!(gone_err.code, "not_open");
    }

    #[tokio::test]
    async fn outbound_stop_unknown_task_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let state = unreachable_laptop_with_task(dir.path()).await;
        let Err((status, _)) = api_a2a_outbound_stop(State(state), Path("nope".to_string())).await
        else {
            panic!("an unknown task can't be stopped");
        };
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

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
        let dir = tempfile::tempdir().unwrap();
        let Json(agents) = api_a2a_agents_list(State(agents_state(&hub, dir.path()).await)).await;
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
        let resp = api_a2a_agents_raw_get(State(files_state(dir.path())))
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
        let result = api_a2a_agents_raw_put(State(files_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(result.valid);

        let resp = api_a2a_agents_raw_get(State(files_state(dir.path())))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), content.as_bytes());
    }

    #[tokio::test]
    async fn agents_raw_put_saves_invalid_json_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let state = files_state(dir.path());
        let result = api_a2a_agents_raw_put(State(state.clone()), "not json".to_string())
            .await
            .unwrap();
        assert!(!result.valid, "invalid JSON should be flagged invalid");
        assert!(!result.diagnostics.is_empty());

        let path = WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
        let saved = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            saved, "not json",
            "the save should have happened despite the invalid content"
        );
    }

    /// A state whose checkpoint repositories can't be opened, counting the
    /// attempts.
    fn unopenable_checkpoints(
        dir: &std::path::Path,
    ) -> (AgentFilesState, Arc<std::sync::atomic::AtomicUsize>) {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&attempts);
        let mut state = files_state(dir);
        state.checkpoints = crate::gateway::web::CheckpointAccess::lazy(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(crate::checkpoints::CheckpointError::Git(
                "the repository is corrupt".to_string(),
            ))
        });
        (state, attempts)
    }

    #[tokio::test]
    async fn agents_raw_put_opens_the_checkpoint_repositories_once_and_saves_without_them() {
        let dir = tempfile::tempdir().unwrap();
        let (state, attempts) = unopenable_checkpoints(dir.path());
        let content = r#"{"agents":{"laptop":{"url":"https://laptop.example.com"}}}"#;

        let result = api_a2a_agents_raw_put(State(state.clone()), content.to_string())
            .await
            .unwrap();

        assert!(result.valid);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        let path = WorkspaceLayout::new(&state.workspace_dir).a2a_agents_json();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), content);
    }

    #[tokio::test]
    async fn agents_raw_get_never_opens_the_checkpoint_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let (state, attempts) = unopenable_checkpoints(dir.path());

        let resp = api_a2a_agents_raw_get(State(state)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn agents_raw_put_reports_bad_agent_name_without_blocking_the_save() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"{"agents":{"Bad Name":{"url":"https://x.example.com"}}}"#;
        let result = api_a2a_agents_raw_put(State(files_state(dir.path())), content.to_string())
            .await
            .unwrap();
        assert!(!result.valid, "a bad agent name should be flagged invalid");
        assert_eq!(result.diagnostics.len(), 1);
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
        let port = free_port();
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
        let port = free_port();
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
            Some("https://example.com/a2a/laptop/agents/test-agent")
        );
        assert!(status.card_error.is_none());
    }

    #[tokio::test]
    async fn status_reports_listener_running_when_something_answers_auth_check() {
        let dir = tempfile::tempdir().unwrap();
        let router = axum::Router::new().route(
            AUTH_CHECK_PATH,
            axum::routing::get(|| async { StatusCode::NO_CONTENT }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        crate::util::spawn_in_span(async move { axum::serve(listener, router).await });

        write_config(
            dir.path(),
            &format!("[a2a]\nenabled = true\nport = {port}\n"),
        );
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let status = api_a2a_status(State(status_state(state))).await.0;
        assert!(status.listener_running);
    }

    #[tokio::test]
    async fn status_never_probes_a_disabled_listener() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
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
        let port = free_port();
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
    async fn status_puts_the_agent_under_its_own_path_without_a_relay() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(
            dir.path(),
            &format!("[a2a]\nport = {port}\npublic_url = \"https://example.com/a2a/\"\n"),
        );
        let state = test_state(dir.path());
        let status = api_a2a_status(State(status_state(state))).await.0;
        assert_eq!(
            status.public_url.as_deref(),
            Some("https://example.com/a2a/agents/test-agent")
        );
        assert_eq!(
            status.local_url,
            format!("http://127.0.0.1:{port}/agents/test-agent")
        );
        assert!(!status.relay_access);
        assert!(
            status.relay_access_note.contains("own address")
                && status
                    .relay_access_note
                    .contains("Connect to the Residuum relay"),
            "{}",
            status.relay_access_note
        );
    }

    #[tokio::test]
    async fn status_with_nothing_configured_or_connected_only_offers_the_local_address() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(dir.path(), &format!("[a2a]\nport = {port}\n"));
        let status = api_a2a_status(State(status_state(test_state(dir.path()))))
            .await
            .0;
        assert_eq!(status.public_url, None);
        assert!(!status.relay_access);
        assert!(
            status.relay_access_note.starts_with("Reachable locally."),
            "{}",
            status.relay_access_note
        );
    }

    #[tokio::test]
    async fn status_uses_the_relay_address_while_the_tunnel_is_connected() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(dir.path(), &format!("[a2a]\nport = {port}\n"));
        let state = status_state_with(test_state(dir.path()), relay_connected());
        let status = api_a2a_status(State(state)).await.0;
        assert_eq!(
            status.public_url.as_deref(),
            Some("https://bear.agent-residuum.com/a2a/laptop/test-agent")
        );
        assert!(status.relay_access);
        assert!(
            status
                .relay_access_note
                .starts_with("Reachable through the Residuum relay"),
            "{}",
            status.relay_access_note
        );
    }

    #[tokio::test]
    async fn status_prefers_the_configured_public_url_over_the_relay_address() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(
            dir.path(),
            &format!("[a2a]\nport = {port}\npublic_url = \"https://example.com\"\n"),
        );
        let state = status_state_with(test_state(dir.path()), relay_connected());
        let status = api_a2a_status(State(state)).await.0;
        assert_eq!(
            status.public_url.as_deref(),
            Some("https://example.com/agents/test-agent")
        );
        assert!(status.relay_access, "the relay still reaches the agent");
    }

    #[tokio::test]
    async fn status_reports_no_relay_access_when_a2a_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(
            dir.path(),
            &format!("[a2a]\nenabled = false\nport = {port}\n"),
        );
        let state = status_state_with(test_state(dir.path()), relay_connected());
        let status = api_a2a_status(State(state)).await.0;
        assert!(!status.relay_access);
    }

    #[tokio::test]
    async fn card_endpoint_advertises_the_relay_address_while_connected() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        write_config(dir.path(), &format!("[a2a]\nport = {port}\n"));
        let state = test_state(dir.path());
        write_card(&state, VALID_CARD);
        let card = api_a2a_card(State(status_state_with(state, relay_connected())))
            .await
            .unwrap()
            .0;
        let first = card.supported_interfaces.first().unwrap();
        assert_eq!(
            first.url,
            "https://bear.agent-residuum.com/a2a/laptop/test-agent"
        );
    }
}

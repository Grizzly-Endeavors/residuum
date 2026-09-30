//! Tests for the hub router, against a fake [`AgentDirectory`].

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::routing::{any, get, post};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tower::ServiceExt;

use super::{HubHttpState, agent_repair_router, hub_router};
use crate::bus::{WorkspaceEvent, topics};
use crate::config::paths::TeamPaths;
use crate::gateway::ReloadSignal;
use crate::gateway::web::{ConfigApiState, WorkspaceScope};
use crate::hub::{
    A2aVisibility, Actor, AgentActivity, AgentDirectory, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, HubEvent, LifecycleError,
};
use crate::tunnel::{TUNNEL_NONCE_HEADER, TunnelStatus, tunnel_nonce};
use crate::workspace::team_files::TeamWriteCoordinator;
use crate::workspace::watch::{WatchHealth, WorkspaceChange, WorkspaceChangeKind};

fn summary(name: &str, state: AgentState) -> AgentSummary {
    AgentSummary {
        name: name.to_string(),
        state,
        last_error: None,
        autostart: true,
        role: None,
        a2a_visibility: A2aVisibility::Private,
    }
}

/// An in-memory directory. Running agents serve a small router that echoes
/// what it was asked; every agent's repair router is the real one over a
/// temp directory.
struct FakeDirectory {
    root: PathBuf,
    agents: Mutex<Vec<AgentSummary>>,
    events: broadcast::Sender<HubEvent>,
    /// Handed out by the next `subscribe`, so a test can start a connection
    /// with events already lost.
    primed: Mutex<Option<broadcast::Receiver<HubEvent>>>,
    calls: Mutex<Vec<String>>,
}

impl FakeDirectory {
    fn new(root: &Path, agents: Vec<AgentSummary>) -> Arc<Self> {
        let (events, _rx) = broadcast::channel(4);
        Arc::new(Self {
            root: root.to_path_buf(),
            agents: Mutex::new(agents),
            events,
            primed: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn find(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.name == name)
            .cloned()
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))
    }

    fn set_state(&self, name: &str, state: AgentState) -> Result<AgentSummary, LifecycleError> {
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .iter_mut()
            .find(|a| a.name == name)
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))?;
        agent.state = state;
        Ok(agent.clone())
    }

    /// Make the next `subscribe` return a receiver that has already fallen
    /// behind.
    fn prime_a_lagged_receiver(&self) {
        let rx = self.events.subscribe();
        for i in 0..10 {
            self.events
                .send(HubEvent::AgentActivity {
                    name: "scout".to_string(),
                    activity: AgentActivity {
                        busy: i % 2 == 0,
                        unread: i,
                    },
                })
                .unwrap();
        }
        *self.primed.lock().unwrap() = Some(rx);
    }

    fn config_state(&self, name: &str, live: bool) -> ConfigApiState {
        let agent_dir = self.root.join(name);
        let team = TeamWriteCoordinator::new(&TeamPaths::new(self.root.join("team")));
        ConfigApiState {
            hub_dir: self.root.join("hub"),
            config_dir: agent_dir.join("config"),
            agent_name: name.to_string(),
            workspace_dir: agent_dir.clone(),
            memory_dir: None,
            reload_tx: live.then(|| tokio::sync::mpsc::unbounded_channel().0),
            checkpoints: crate::checkpoints::test_engine(),
            team: Some(team.view_for_user(agent_dir)),
            scope: WorkspaceScope::Agent,
        }
    }
}

async fn echo_socket(mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        if let AxumMessage::Text(text) = message
            && socket
                .send(AxumMessage::text(format!("echo:{text}")))
                .await
                .is_err()
        {
            return;
        }
    }
}

fn running_agent_router(name: &str) -> Router {
    let status_agent = name.to_string();
    let hook_agent = name.to_string();
    let session_agent = name.to_string();
    Router::new()
        .route(
            "/api/status",
            get(move || {
                let agent = status_agent.clone();
                async move { axum::Json(json!({ "agent": agent, "route": "running" })) }
            }),
        )
        .route(
            "/api/files/{id}",
            get(|uri: axum::http::Uri| async move { uri.to_string() }),
        )
        .route(
            "/api/things/{id}/read",
            get(|axum::extract::Path(id): axum::extract::Path<String>| async move { id }),
        )
        .route(
            "/api/sessions",
            post(move |headers: HeaderMap| {
                let agent = session_agent.clone();
                async move {
                    let artifact = headers
                        .get("x-residuum-artifact")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    axum::Json(json!({ "agent": agent, "artifact": artifact }))
                }
            }),
        )
        .route(
            "/webhook/{hook}",
            post(
                move |axum::extract::Path(hook): axum::extract::Path<String>| {
                    let agent = hook_agent.clone();
                    async move { axum::Json(json!({ "agent": agent, "hook": hook })) }
                },
            ),
        )
        .route(
            "/ws",
            any(|ws: WebSocketUpgrade| async move { ws.on_upgrade(echo_socket) }),
        )
}

#[async_trait]
impl AgentDirectory for FakeDirectory {
    fn list(&self) -> Vec<AgentSummary> {
        self.agents.lock().unwrap().clone()
    }

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.find(name)
    }

    fn agent_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let agent = self.find(name)?;
        if agent.state != AgentState::Running {
            return Err(LifecycleError::NotRunning {
                name: name.to_string(),
                state: agent.state,
            });
        }
        Ok(running_agent_router(name))
    }

    fn agent_repair_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let agent = self.find(name)?;
        Ok(agent_repair_router(
            self.config_state(name, agent.state == AgentState::Running),
        ))
    }

    fn agent_a2a_router(&self, name: &str) -> Result<Router, LifecycleError> {
        Err(LifecycleError::Failed(format!(
            "{name} has no a2a router in this fake"
        )))
    }

    fn activity(&self) -> Vec<(String, AgentActivity)> {
        Vec::new()
    }

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.record(format!("create {} by {}", request.name, by.wire()));
        crate::config::validate_agent_name(&request.name).map_err(LifecycleError::InvalidName)?;
        if self.find(&request.name).is_ok() {
            return Err(LifecycleError::AlreadyExists(request.name));
        }
        let mut created = summary(&request.name, AgentState::Running);
        if let Some(visibility) = request.a2a_visibility {
            created.a2a_visibility = visibility;
        }
        self.agents.lock().unwrap().push(created.clone());
        Ok(created)
    }

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        self.record(format!("delete {name} by {}", by.wire()));
        self.find(name)?;
        self.agents.lock().unwrap().retain(|a| a.name != name);
        Ok(DeleteOutcome {
            deleted: true,
            checkpoint_id: Some("cp-1".to_string()),
        })
    }

    async fn start(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.record(format!("start {name}"));
        self.set_state(name, AgentState::Running)
    }

    async fn stop(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.record(format!("stop {name}"));
        self.set_state(name, AgentState::Stopped)
    }

    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.record(format!("restart {name}"));
        self.set_state(name, AgentState::Running)
    }

    async fn patch(&self, name: &str, patch: AgentPatch) -> Result<AgentSummary, LifecycleError> {
        self.record(format!("patch {name}"));
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .iter_mut()
            .find(|a| a.name == name)
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))?;
        if let Some(autostart) = patch.autostart {
            agent.autostart = autostart;
        }
        if let Some(visibility) = patch.a2a_visibility {
            agent.a2a_visibility = visibility;
        }
        Ok(agent.clone())
    }

    fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.primed
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| self.events.subscribe())
    }
}

/// A hub router over a fake directory and temp directories, with the
/// receiving ends of the channels the hub signals on.
struct Harness {
    app: Router,
    directory: Arc<FakeDirectory>,
    root: tempfile::TempDir,
    reload_rx: mpsc::UnboundedReceiver<ReloadSignal>,
    shutdown_rx: mpsc::Receiver<()>,
    team_bus: crate::bus::BusHandle,
    _tunnel_tx: watch::Sender<TunnelStatus>,
    health_tx: watch::Sender<WatchHealth>,
    _restart_rx: mpsc::Receiver<()>,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        std::fs::create_dir_all(root.path().join("team")).unwrap();
        let directory = FakeDirectory::new(
            root.path(),
            vec![
                summary("scout", AgentState::Running),
                summary("quiet", AgentState::Stopped),
            ],
        );

        let (reload_tx, reload_rx) = mpsc::unbounded_channel();
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let (tunnel_tx, tunnel_rx) = watch::channel(TunnelStatus::Disconnected);
        let (health_tx, health_rx) = watch::channel(WatchHealth::Native);
        let team_bus = crate::bus::spawn_broker();
        let (_layer, span_buffer) = crate::util::telemetry::SpanBufferLayer::new(
            &crate::util::telemetry::SpanBufferConfig::default(),
        );
        let hub = HubHttpState {
            hub_dir: hub_dir.clone(),
            reload_tx,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
            tunnel_status_rx: tunnel_rx,
            update_status: crate::update::SharedUpdateStatus::default(),
            restart_tx,
            shutdown_tx,
            tracing_service: Arc::new(crate::tracing_service::TracingService::new(
                crate::config::TracingConfig::default(),
                span_buffer,
            )),
            client_context: Arc::new(crate::tracing_service::ClientContext {
                version: "test".to_string(),
                commit: None,
                os: "test-os".to_string(),
                arch: "test-arch".to_string(),
                model_provider: None,
                model_name: None,
                active_subagents: Vec::new(),
                config_flags: std::collections::BTreeMap::new(),
                agent: None,
            }),
            active_subagents: Arc::new(Vec::new),
            workbench_serving: crate::workbench::server::WorkbenchServing::Unavailable {
                reason: "not started in tests".to_string(),
            },
            team: TeamWriteCoordinator::new(&TeamPaths::new(root.path().join("team"))),
            team_bus: team_bus.clone(),
            team_watch_health: health_rx,
            started_at: std::time::Instant::now(),
        };
        let shared: Arc<dyn AgentDirectory> = Arc::<FakeDirectory>::clone(&directory);
        let app = hub_router(shared, hub);
        Self {
            app,
            directory,
            root,
            reload_rx,
            shutdown_rx,
            team_bus,
            _tunnel_tx: tunnel_tx,
            health_tx,
            _restart_rx: restart_rx,
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, body.to_vec())
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let (status, _headers, bytes) = self.send(builder.body(body).unwrap()).await;
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    /// The status of the response to `request`.
    async fn status(&self, request: Request<Body>) -> StatusCode {
        self.send(request).await.0
    }

    /// The status and body of the response to `request`.
    async fn send_body(&self, request: Request<Body>) -> (StatusCode, Vec<u8>) {
        let (status, _headers, body) = self.send(request).await;
        (status, body)
    }

    /// Make a request, assert its status, and return the JSON body (or the
    /// text of a body that isn't JSON).
    async fn expect(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
        want: StatusCode,
    ) -> Value {
        let (status, reply) = self.call(method.clone(), uri, body).await;
        assert_eq!(status, want, "{method} {uri}: {reply}");
        reply
    }

    async fn get_expect(&self, uri: &str, want: StatusCode) -> Value {
        self.expect(Method::GET, uri, None, want).await
    }

    async fn post_expect(&self, uri: &str, want: StatusCode) -> Value {
        self.expect(Method::POST, uri, None, want).await
    }

    /// Serve the app on a local port, for the WebSocket tests.
    async fn serve(&self) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = self.app.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        addr
    }
}

type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: SocketAddr, path: &str) -> ClientSocket {
    let (socket, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
        .await
        .unwrap();
    socket
}

async fn next_frame(socket: &mut ClientSocket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("timed out waiting for a frame")
        .expect("socket closed")
        .expect("socket error");
    match message {
        ClientMessage::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other @ (ClientMessage::Binary(_)
        | ClientMessage::Ping(_)
        | ClientMessage::Pong(_)
        | ClientMessage::Close(_)
        | ClientMessage::Frame(_)) => panic!("expected a text frame, got {other:?}"),
    }
}

async fn send_client(socket: &mut ClientSocket, frame: &Value) {
    socket
        .send(ClientMessage::text(frame.to_string()))
        .await
        .unwrap();
}

// ---- lifecycle API -------------------------------------------------------

fn agent_names(list: &Value) -> Vec<&str> {
    list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn agent_list_is_sorted_by_name() {
    let h = Harness::new();
    let list = h.get_expect("/api/hub/agents", StatusCode::OK).await;
    assert_eq!(agent_names(&list), ["quiet", "scout"]);
    assert_eq!(list["agents"][0]["state"], "stopped");
    assert_eq!(list["agents"][0]["last_error"], Value::Null);
}

#[tokio::test]
async fn an_empty_hub_lists_no_agents() {
    let h = Harness::new();
    h.directory.agents.lock().unwrap().clear();
    let list = h.get_expect("/api/hub/agents", StatusCode::OK).await;
    assert_eq!(list, json!({ "agents": [] }));
}

#[tokio::test]
async fn create_answers_201_with_the_summary_and_attributes_the_user() {
    let h = Harness::new();
    let created = h
        .expect(
            Method::POST,
            "/api/hub/agents",
            Some(json!({ "name": "nova", "models_from": "scout", "a2a_visibility": "public" })),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(created["name"], "nova");
    assert_eq!(created["a2a_visibility"], "public");
    assert_eq!(h.directory.calls(), ["create nova by user"]);
}

#[tokio::test]
async fn create_answers_400_for_an_invalid_name_and_409_for_an_existing_one() {
    let h = Harness::new();
    let invalid = h
        .expect(
            Method::POST,
            "/api/hub/agents",
            Some(json!({ "name": "Bad Name" })),
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert!(invalid["error"].as_str().unwrap().contains("Bad Name"));

    let duplicate = h
        .expect(
            Method::POST,
            "/api/hub/agents",
            Some(json!({ "name": "scout" })),
            StatusCode::CONFLICT,
        )
        .await;
    assert_eq!(duplicate["error"], "an agent named 'scout' already exists");
}

#[tokio::test]
async fn create_answers_400_for_a_body_it_cannot_read() {
    let h = Harness::new();
    let (status, bytes) = h
        .send_body(
            Request::builder()
                .method(Method::POST)
                .uri("/api/hub/agents")
                .header("content-type", "application/json")
                .body(Body::from("{ not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let refusal: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(refusal["error"].as_str().unwrap().contains("request body"));
    assert!(h.directory.calls().is_empty());
}

#[tokio::test]
async fn delete_reports_the_checkpoint_and_404s_for_an_unknown_agent() {
    let h = Harness::new();
    let deleted = h
        .expect(
            Method::DELETE,
            "/api/hub/agents/quiet",
            None,
            StatusCode::OK,
        )
        .await;
    assert_eq!(deleted, json!({ "deleted": true, "checkpoint_id": "cp-1" }));

    let missing = h
        .expect(
            Method::DELETE,
            "/api/hub/agents/ghost",
            None,
            StatusCode::NOT_FOUND,
        )
        .await;
    assert_eq!(missing, json!({ "error": "no agent named 'ghost'" }));
}

#[tokio::test]
async fn start_stop_and_restart_answer_the_new_summary() {
    let h = Harness::new();
    let started = h
        .post_expect("/api/hub/agents/quiet/start", StatusCode::OK)
        .await;
    assert_eq!(started["state"], "running");
    let stopped = h
        .post_expect("/api/hub/agents/quiet/stop", StatusCode::OK)
        .await;
    assert_eq!(stopped["state"], "stopped");
    let restarted = h
        .post_expect("/api/hub/agents/quiet/restart", StatusCode::OK)
        .await;
    assert_eq!(restarted["state"], "running");
    assert_eq!(
        h.directory.calls(),
        ["start quiet", "stop quiet", "restart quiet"]
    );

    for action in ["start", "stop", "restart"] {
        let missing = h
            .post_expect(
                &format!("/api/hub/agents/ghost/{action}"),
                StatusCode::NOT_FOUND,
            )
            .await;
        assert_eq!(missing["error"], "no agent named 'ghost'", "{action}");
    }
}

#[tokio::test]
async fn patch_changes_autostart_and_visibility() {
    let h = Harness::new();
    let patched = h
        .expect(
            Method::PATCH,
            "/api/hub/agents/scout",
            Some(json!({ "autostart": false, "a2a_visibility": "public" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(patched["autostart"], false);
    assert_eq!(patched["a2a_visibility"], "public");

    h.expect(
        Method::PATCH,
        "/api/hub/agents/ghost",
        Some(json!({ "autostart": true })),
        StatusCode::NOT_FOUND,
    )
    .await;
}

#[tokio::test]
async fn patch_needs_a_field_and_a_known_value() {
    let h = Harness::new();
    let empty = h
        .expect(
            Method::PATCH,
            "/api/hub/agents/scout",
            Some(json!({})),
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert!(empty["error"].as_str().unwrap().contains("at least one"));

    h.expect(
        Method::PATCH,
        "/api/hub/agents/scout",
        Some(json!({ "a2a_visibility": "secret" })),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(h.directory.calls().is_empty());
}

#[tokio::test]
async fn status_reports_version_uptime_tunnel_and_counts() {
    let h = Harness::new();
    h.directory
        .agents
        .lock()
        .unwrap()
        .push(summary("broken", AgentState::Failed));
    let hub_status = h.get_expect("/api/hub/status", StatusCode::OK).await;
    assert_eq!(hub_status["version"], crate::update::CURRENT_VERSION);
    assert!(hub_status["uptime_secs"].is_u64());
    assert_eq!(hub_status["tunnel"]["status"], "disconnected");
    assert_eq!(hub_status["tunnel"]["enabled"], false);
    assert_eq!(hub_status["agents"]["running"], 1);
    assert_eq!(hub_status["agents"]["stopped"], 1);
    assert_eq!(hub_status["agents"]["failed"], 1);
    assert_eq!(hub_status["agents"]["starting"], 0);
}

// ---- agent dispatch ------------------------------------------------------

#[tokio::test]
async fn agent_requests_reach_the_agents_router_without_the_hub_prefix() {
    let h = Harness::new();
    let agent_status = h
        .get_expect("/api/agents/scout/status", StatusCode::OK)
        .await;
    assert_eq!(
        agent_status,
        json!({ "agent": "scout", "route": "running" })
    );

    // The path moves under `/api` in the agent's table and the query survives.
    let file_uri = h
        .get_expect("/api/agents/scout/files/abc?x=1&y=two", StatusCode::OK)
        .await;
    assert_eq!(file_uri, json!("/api/files/abc?x=1&y=two"));
}

#[tokio::test]
async fn an_agents_own_path_parameters_still_extract_behind_dispatch() {
    let h = Harness::new();
    let id = h
        .get_expect("/api/agents/scout/things/xyz/read", StatusCode::OK)
        .await;
    assert_eq!(id, json!("xyz"));
}

#[tokio::test]
async fn unknown_and_stopped_agents_get_the_contract_errors() {
    let h = Harness::new();
    let unknown = h
        .get_expect("/api/agents/ghost/status", StatusCode::NOT_FOUND)
        .await;
    assert_eq!(unknown, json!({ "error": "no agent named 'ghost'" }));

    let stopped = h
        .get_expect("/api/agents/quiet/status", StatusCode::CONFLICT)
        .await;
    assert_eq!(
        stopped,
        json!({ "error": "quiet is stopped", "state": "stopped" })
    );

    h.directory
        .agents
        .lock()
        .unwrap()
        .push(summary("broken", AgentState::Failed));
    let failed = h
        .get_expect("/api/agents/broken/sessions", StatusCode::CONFLICT)
        .await;
    assert_eq!(failed["state"], "failed");
}

#[tokio::test]
async fn repair_routes_use_the_repair_router_so_a_stopped_agent_can_be_fixed() {
    let h = Harness::new();
    let config_dir = h.root.path().join("quiet/config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "# quiet's config\n").unwrap();

    let (read_status, read_bytes) = h
        .send_body(
            Request::get("/api/agents/quiet/config/raw")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(String::from_utf8(read_bytes).unwrap(), "# quiet's config\n");

    // A repair write lands on disk for the stopped agent too.
    let write_status = h
        .status(
            Request::put("/api/agents/quiet/config/raw")
                .body(Body::from("# fixed\n"))
                .unwrap(),
        )
        .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(config_dir.join("config.toml")).unwrap(),
        "# fixed\n"
    );

    // Workspace files and checkpoints are repair routes as well.
    h.get_expect("/api/agents/quiet/workspace/files", StatusCode::OK)
        .await;
    h.get_expect(
        "/api/agents/quiet/checkpoints?repo=workspace",
        StatusCode::OK,
    )
    .await;

    // Anything else still needs the agent to be running.
    let needs_running = h
        .get_expect("/api/agents/quiet/status", StatusCode::CONFLICT)
        .await;
    assert_eq!(needs_running["state"], "stopped");
}

#[tokio::test]
async fn an_unknown_agent_is_a_404_on_repair_routes_too() {
    let h = Harness::new();
    let missing = h
        .get_expect("/api/agents/ghost/config/raw", StatusCode::NOT_FOUND)
        .await;
    assert_eq!(missing["error"], "no agent named 'ghost'");
}

#[tokio::test]
async fn an_agents_checkpoint_routes_refuse_the_hub_repositories() {
    let h = Harness::new();
    for repo in ["hub", "team"] {
        h.get_expect(
            &format!("/api/agents/scout/checkpoints?repo={repo}"),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

#[tokio::test]
async fn a_websocket_upgrade_passes_through_dispatch() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/agents/scout/ws").await;
    socket.send(ClientMessage::text("hello")).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply, ClientMessage::text("echo:hello"));
}

#[tokio::test]
async fn a_websocket_to_a_stopped_agent_is_refused_with_the_state() {
    let h = Harness::new();
    let addr = h.serve().await;
    let refusal = tokio_tungstenite::connect_async(format!("ws://{addr}/api/agents/quiet/ws"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &refusal,
            tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::CONFLICT
        ),
        "expected a 409 refusal, got {refusal:?}"
    );
}

#[tokio::test]
async fn webhooks_reach_the_named_agents_webhook_route() {
    let h = Harness::new();
    let delivered = h.post_expect("/webhook/scout/deploy", StatusCode::OK).await;
    assert_eq!(delivered, json!({ "agent": "scout", "hook": "deploy" }));

    let stopped = h
        .post_expect("/webhook/quiet/deploy", StatusCode::CONFLICT)
        .await;
    assert_eq!(stopped["state"], "stopped");

    let unknown = h
        .post_expect("/webhook/ghost/deploy", StatusCode::NOT_FOUND)
        .await;
    assert_eq!(unknown["error"], "no agent named 'ghost'");
}

// ---- artifact sessions ---------------------------------------------------

#[tokio::test]
async fn a_session_start_that_names_no_agent_is_refused_with_an_explanation() {
    let h = Harness::new();
    let refusal = h
        .post_expect("/api/sessions", StatusCode::BAD_REQUEST)
        .await;
    let message = refusal["error"].as_str().unwrap();
    assert!(message.contains("/api/agents/<name>/sessions"), "{message}");
}

#[tokio::test]
async fn a_session_start_needs_a_running_agent() {
    let h = Harness::new();
    let start = |name: &str| {
        Request::post(format!("/api/agents/{name}/sessions"))
            .header("x-residuum-artifact", "chart")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "prompt": "go" }).to_string()))
            .unwrap()
    };

    let (unknown_status, unknown_bytes) = h.send_body(start("ghost")).await;
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_slice::<Value>(&unknown_bytes).unwrap(),
        json!({ "error": "no agent named 'ghost'" })
    );

    let (stopped_status, stopped_bytes) = h.send_body(start("quiet")).await;
    assert_eq!(stopped_status, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped_bytes).unwrap(),
        json!({ "error": "quiet is stopped", "state": "stopped" })
    );

    // The running agent sees the artifact identity header intact.
    let (running_status, running_bytes) = h.send_body(start("scout")).await;
    assert_eq!(running_status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&running_bytes).unwrap(),
        json!({ "agent": "scout", "artifact": "chart" })
    );
}

// ---- guards --------------------------------------------------------------

fn cross_site(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap()
}

fn through_the_tunnel(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(TUNNEL_NONCE_HEADER, tunnel_nonce())
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn the_cross_site_guard_covers_the_whole_app() {
    let h = Harness::new();
    for uri in [
        "/api/hub/agents/scout/stop",
        "/api/agents/scout/sessions",
        "/webhook/scout/deploy",
        "/api/hub/secrets",
        "/api/team/workspace/file",
    ] {
        let write_status = h.status(cross_site(Method::POST, uri)).await;
        assert_eq!(write_status, StatusCode::FORBIDDEN, "{uri}");
    }
    assert!(h.directory.calls().is_empty());

    // Reads are side-effect free and stay open; a cross-site WebSocket
    // upgrade is not.
    let read_status = h.status(cross_site(Method::GET, "/api/hub/agents")).await;
    assert_eq!(read_status, StatusCode::OK);
    let upgrade = Request::get("/api/hub/ws")
        .header("sec-fetch-site", "cross-site")
        .header("upgrade", "websocket")
        .body(Body::empty())
        .unwrap();
    let upgrade_status = h.status(upgrade).await;
    assert_eq!(upgrade_status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_remote_control_guard_refuses_shutdown_and_disconnect_over_the_tunnel() {
    let mut h = Harness::new();
    for uri in ["/api/hub/shutdown", "/api/hub/cloud/disconnect"] {
        let guarded_status = h.status(through_the_tunnel(Method::POST, uri)).await;
        assert_eq!(guarded_status, StatusCode::FORBIDDEN, "{uri}");
    }
    assert!(h.directory.calls().is_empty(), "no agent was stopped");
    assert!(h.shutdown_rx.try_recv().is_err(), "no shutdown was sent");

    // Other lifecycle routes stay reachable remotely.
    let open_status = h
        .status(through_the_tunnel(
            Method::POST,
            "/api/hub/agents/scout/stop",
        ))
        .await;
    assert_eq!(open_status, StatusCode::OK);
}

#[tokio::test]
async fn stop_all_is_reachable_over_the_tunnel() {
    let h = Harness::new();
    let status = h
        .status(through_the_tunnel(Method::POST, "/api/hub/stop-all"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h.directory.calls(), ["stop scout"]);
}

#[tokio::test]
async fn shutdown_and_stop_all_work_locally() {
    let mut h = Harness::new();
    let shutdown = h.post_expect("/api/hub/shutdown", StatusCode::OK).await;
    assert_eq!(shutdown, json!({ "shutting_down": true }));
    assert!(h.shutdown_rx.try_recv().is_ok());

    let stop_all = h.post_expect("/api/hub/stop-all", StatusCode::OK).await;
    assert_eq!(stop_all["stopped"].as_array().unwrap().len(), 1);
    assert_eq!(stop_all["stopped"][0]["name"], "scout");
    assert_eq!(stop_all["failed"], json!([]));
    assert_eq!(
        h.directory.calls(),
        ["stop scout"],
        "stopped agents are left alone"
    );
}

#[tokio::test]
async fn cloud_disconnect_works_locally_and_asks_the_hub_to_reload() {
    let mut h = Harness::new();
    std::fs::write(
        h.root.path().join("hub/config.toml"),
        "[cloud]\nenabled = true\ntoken = \"secret:cloud_token\"\n",
    )
    .unwrap();
    let disconnected = h
        .post_expect("/api/hub/cloud/disconnect", StatusCode::OK)
        .await;
    assert_eq!(disconnected, json!({ "ok": true }));
    assert!(matches!(h.reload_rx.try_recv(), Ok(ReloadSignal::Hub)));
}

// ---- hub-level routes ----------------------------------------------------

#[tokio::test]
async fn hub_config_secrets_and_keys_live_under_api_hub() {
    let mut h = Harness::new();

    let config = h.get_expect("/api/hub/config/raw", StatusCode::OK).await;
    assert_eq!(config, json!("timezone = \"UTC\"\n"));
    let put_status = h
        .status(
            Request::put("/api/hub/config/raw")
                .body(Body::from("timezone = \"UTC\"\n# edited\n"))
                .unwrap(),
        )
        .await;
    assert_eq!(put_status, StatusCode::OK);
    assert!(matches!(h.reload_rx.try_recv(), Ok(ReloadSignal::Hub)));

    let stored = h
        .expect(
            Method::POST,
            "/api/hub/secrets",
            Some(json!({ "name": "fw", "value": "sk-literal" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(stored["reference"], "secret:fw");
    let secrets = h.get_expect("/api/hub/secrets", StatusCode::OK).await;
    assert_eq!(secrets["names"], json!(["fw"]));

    h.get_expect("/api/hub/agent-keys", StatusCode::OK).await;
    h.expect(
        Method::POST,
        "/api/hub/a2a/keys",
        Some(json!({ "name": "laptop" })),
        StatusCode::OK,
    )
    .await;
    h.get_expect("/api/hub/a2a/keys", StatusCode::OK).await;
    h.expect(
        Method::DELETE,
        "/api/hub/a2a/keys/laptop",
        None,
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
async fn the_relocated_routes_are_gone_from_their_old_paths() {
    let h = Harness::new();
    for (method, uri) in [
        (Method::GET, "/api/secrets"),
        (Method::GET, "/api/agent-keys"),
        (Method::GET, "/api/a2a/keys"),
        (Method::GET, "/api/cloud/status"),
        (Method::POST, "/api/shutdown"),
        (Method::GET, "/api/update/status"),
        (Method::GET, "/api/tracing/status"),
        (Method::GET, "/api/system/timezone"),
        (Method::GET, "/api/mcp-catalog"),
        (Method::GET, "/api/workbench/info"),
        (Method::GET, "/api/workspace/files"),
        (Method::GET, "/api/checkpoints?repo=hub"),
        (Method::POST, "/api/config/complete-setup"),
        (Method::GET, "/api/status"),
    ] {
        h.expect(method.clone(), uri, None, StatusCode::NOT_FOUND)
            .await;
    }
}

#[tokio::test]
async fn hub_info_routes_answer_under_api_hub() {
    let h = Harness::new();
    for uri in [
        "/api/hub/update/status",
        "/api/hub/tracing/status",
        "/api/hub/system/timezone",
        "/api/hub/mcp-catalog",
    ] {
        h.get_expect(uri, StatusCode::OK).await;
    }
    let cloud = h.get_expect("/api/hub/cloud/status", StatusCode::OK).await;
    assert_eq!(cloud["status"], "disconnected");
}

#[tokio::test]
async fn provider_models_are_listed_without_an_agent() {
    let h = Harness::new();
    let hub_listing = h
        .expect(
            Method::POST,
            "/api/hub/providers/models",
            Some(json!({ "provider": "not-a-provider" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(hub_listing["error"], "unknown provider: not-a-provider");

    let agent_listing = h
        .expect(
            Method::POST,
            "/api/agents/quiet/providers/models",
            Some(json!({ "provider": "not-a-provider" })),
            StatusCode::OK,
        )
        .await;
    assert_eq!(agent_listing["error"], "unknown provider: not-a-provider");
}

#[tokio::test]
async fn hub_checkpoints_serve_only_the_hub_and_team_repositories() {
    let h = Harness::new();
    for repo in ["hub", "team"] {
        h.get_expect(&format!("/api/hub/checkpoints?repo={repo}"), StatusCode::OK)
            .await;
        h.get_expect(
            &format!("/api/hub/checkpoints/stats?repo={repo}"),
            StatusCode::OK,
        )
        .await;
    }
    for repo in ["workspace", "agent_config"] {
        let refusal = h
            .get_expect(
                &format!("/api/hub/checkpoints?repo={repo}"),
                StatusCode::BAD_REQUEST,
            )
            .await;
        assert!(
            refusal.as_str().unwrap().contains("hub or team"),
            "{refusal}"
        );
    }
}

#[tokio::test]
async fn onboarding_is_a_hub_route() {
    let h = Harness::new();
    let refusal = h
        .expect(
            Method::POST,
            "/api/hub/config/complete-setup",
            Some(json!({
                "hub_config": "timezone = \"UTC\"\n",
                "agent_name": "Team",
                "config": "",
                "providers": "",
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert_eq!(refusal["valid"], false);
}

// ---- team routes ---------------------------------------------------------

#[tokio::test]
async fn team_files_are_addressed_relative_to_the_team_directory() {
    let h = Harness::new();
    h.expect(
        Method::PUT,
        "/api/team/workspace/file",
        Some(json!({ "path": "wiki/plan.md", "content": "# plan" })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(h.root.path().join("team/wiki/plan.md")).unwrap(),
        "# plan"
    );

    let root_listing = h
        .get_expect("/api/team/workspace/files", StatusCode::OK)
        .await;
    let root_names: Vec<&str> = root_listing
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(root_names, ["wiki"], "the team root lists its own contents");

    let wiki_listing = h
        .get_expect("/api/team/workspace/files?path=wiki", StatusCode::OK)
        .await;
    assert_eq!(wiki_listing[0]["name"], "plan.md");

    let (read_status, read_bytes) = h
        .send_body(
            Request::get("/api/team/workspace/file?path=wiki/plan.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(read_status, StatusCode::OK);
    assert!(String::from_utf8(read_bytes).unwrap().contains("# plan"));
}

#[tokio::test]
async fn team_files_cannot_escape_the_team_directory() {
    let h = Harness::new();
    std::fs::write(h.root.path().join("hub/secret.txt"), "hidden").unwrap();
    let (status, bytes) = h
        .send_body(
            Request::get("/api/team/workspace/file?path=../hub/secret.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_ne!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&bytes).contains("hidden"));
}

#[tokio::test]
async fn the_workbench_lives_under_the_team() {
    let h = Harness::new();
    let info = h
        .get_expect("/api/team/workbench/info", StatusCode::OK)
        .await;
    assert_eq!(info["unavailable_reason"], "not started in tests");
    let artifacts = h
        .get_expect("/api/team/workbench/artifacts", StatusCode::OK)
        .await;
    assert_eq!(artifacts, json!([]));
}

// ---- static app and root routes ------------------------------------------

#[tokio::test]
async fn client_routes_get_the_app_shell_and_unknown_api_paths_404() {
    let h = Harness::new();
    let (shell_status, shell_headers, _bytes) = h
        .send(
            Request::get("/agent/scout/chat")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(shell_status, StatusCode::OK);
    assert!(
        shell_headers["content-type"]
            .to_str()
            .unwrap()
            .contains("html")
    );

    h.get_expect("/api/nothing-here", StatusCode::NOT_FOUND)
        .await;
}

#[tokio::test]
async fn the_cloud_callback_stays_at_the_root() {
    let h = Harness::new();
    let (status, bytes) = h
        .send_body(Request::get("/cloud/callback").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8(bytes).unwrap().contains("Missing token"));
}

// ---- hub websocket -------------------------------------------------------

#[tokio::test]
async fn the_hub_socket_opens_with_a_snapshot_then_forwards_events() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;

    let snapshot = next_frame(&mut socket).await;
    assert_eq!(snapshot["type"], "agents_snapshot");
    assert_eq!(agent_names(&snapshot), ["quiet", "scout"]);

    let events = [
        HubEvent::AgentState {
            agent: summary("scout", AgentState::Stopped),
        },
        HubEvent::AgentCreated {
            agent: summary("nova", AgentState::Running),
            by: Actor::Agent("scout".to_string()),
        },
        HubEvent::AgentActivity {
            name: "scout".to_string(),
            activity: AgentActivity {
                busy: true,
                unread: 3,
            },
        },
        HubEvent::AgentDeleted {
            name: "nova".to_string(),
            by: Actor::User,
        },
    ];
    for event in &events {
        h.directory.events.send(event.clone()).unwrap();
    }
    for event in &events {
        assert_eq!(
            next_frame(&mut socket).await,
            serde_json::to_value(event).unwrap()
        );
    }

    h.directory
        .events
        .send(HubEvent::Notice {
            level: crate::hub::types::NoticeLevel::Warn,
            message: "watch out".to_string(),
            agent: Some("scout".to_string()),
        })
        .unwrap();
    assert_eq!(
        next_frame(&mut socket).await,
        json!({ "type": "notice", "level": "warn", "message": "watch out", "agent": "scout" })
    );
}

#[tokio::test]
async fn a_lagging_hub_socket_gets_a_fresh_snapshot_instead_of_silence() {
    let h = Harness::new();
    h.directory.prime_a_lagged_receiver();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;

    // The connect snapshot, then a second one because events were lost, then
    // the events that were still buffered.
    let connect_snapshot = next_frame(&mut socket).await;
    assert_eq!(connect_snapshot["type"], "agents_snapshot");
    let resnapshot = next_frame(&mut socket).await;
    assert_eq!(resnapshot["type"], "agents_snapshot");
    assert_eq!(resnapshot["agents"].as_array().unwrap().len(), 2);
    let buffered = next_frame(&mut socket).await;
    assert_eq!(buffered["type"], "agent_activity");
}

fn team_change(path: &str) -> WorkspaceChange {
    WorkspaceChange {
        path: path.to_string(),
        kind: WorkspaceChangeKind::Modified,
    }
}

/// Publish `changes` on the team bus until the socket shows a frame, since
/// the watch request has no acknowledgement.
async fn publish_until_a_frame_arrives(
    h: &Harness,
    socket: &mut ClientSocket,
    changes: &[&str],
) -> Value {
    let publisher = h.team_bus.publisher();
    for _attempt in 0..100 {
        let batch: Vec<WorkspaceChange> = changes.iter().map(|p| team_change(p)).collect();
        publisher
            .publish(topics::Workspace, WorkspaceEvent::Changed(batch.into()))
            .await
            .unwrap();
        if let Ok(Some(Ok(ClientMessage::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(100), socket.next()).await
        {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
    panic!("no frame arrived");
}

#[tokio::test]
async fn watched_team_paths_are_forwarded_and_others_are_not() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": ["team/wiki"] }),
    )
    .await;
    let frame = publish_until_a_frame_arrives(
        &h,
        &mut socket,
        &["team/wiki/a.md", "team/skills/s.md", "memory/m.md"],
    )
    .await;
    assert_eq!(
        frame,
        json!({
            "type": "workspace_changed",
            "changes": [{ "path": "team/wiki/a.md", "kind": "modified" }],
        })
    );

    // Watching nothing stops the frames.
    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": [] }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let publisher = h.team_bus.publisher();
    for _attempt in 0..5 {
        publisher
            .publish(
                topics::Workspace,
                WorkspaceEvent::Changed(vec![team_change("team/wiki/a.md")].into()),
            )
            .await
            .unwrap();
    }
    let quiet = tokio::time::timeout(Duration::from_millis(400), socket.next()).await;
    assert!(quiet.is_err(), "no frames once nothing is watched");
}

#[tokio::test]
async fn a_watch_outside_team_is_refused_visibly() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": ["wiki"] }),
    )
    .await;
    let refusal = next_frame(&mut socket).await;
    assert_eq!(refusal["type"], "notice");
    assert_eq!(refusal["level"], "warn");
    assert!(refusal["message"].as_str().unwrap().contains("team/"));

    send_client(&mut socket, &json!({ "type": "nonsense" })).await;
    let unreadable = next_frame(&mut socket).await;
    assert_eq!(unreadable["type"], "notice");
}

#[tokio::test]
async fn watching_while_live_updates_are_off_says_so() {
    let h = Harness::new();
    h.health_tx.send_replace(WatchHealth::Off);
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");

    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": ["team"] }),
    )
    .await;
    let unavailable = next_frame(&mut socket).await;
    assert_eq!(unavailable["type"], "workspace_watch_unavailable");
}

#[tokio::test]
async fn a_resync_reaches_a_watching_client() {
    let h = Harness::new();
    let addr = h.serve().await;
    let mut socket = connect(addr, "/api/hub/ws").await;
    assert_eq!(next_frame(&mut socket).await["type"], "agents_snapshot");
    send_client(
        &mut socket,
        &json!({ "type": "watch_team", "prefixes": ["team"] }),
    )
    .await;

    let publisher = h.team_bus.publisher();
    for _attempt in 0..100 {
        publisher
            .publish(
                topics::Workspace,
                WorkspaceEvent::Resync(crate::workspace::watch::WorkspaceResyncReason::Overflow),
            )
            .await
            .unwrap();
        if let Ok(Some(Ok(ClientMessage::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(100), socket.next()).await
        {
            let frame: Value = serde_json::from_str(text.as_str()).unwrap();
            assert_eq!(
                frame,
                json!({ "type": "workspace_resync", "reason": "overflow" })
            );
            return;
        }
    }
    panic!("no resync frame arrived");
}

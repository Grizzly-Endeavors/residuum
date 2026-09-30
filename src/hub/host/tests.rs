//! Integration tests for the agent host: real agents built from temp
//! directories against mock model servers, served through the hub's HTTP
//! wiring.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing_subscriber::layer::SubscriberExt as _;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::hub::runtime::build_app;
use crate::hub::test_support::{free_port, mount_reply, write_agent};

const POLL_TIMEOUT: Duration = Duration::from_secs(20);

/// A hub over a temp residuum root with running-capable agents, each talking
/// to its own mock model server that answers "<name> here".
struct Fixture {
    root: tempfile::TempDir,
    host: Arc<AgentHost>,
    services: HubServices,
    addr: String,
    mocks: BTreeMap<String, MockServer>,
    http: reqwest::Client,
}

/// The string at `key` of a JSON object.
fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap()
}

/// The array at `key` of a JSON object.
fn array_at<'a>(value: &'a Value, key: &str) -> &'a Vec<Value> {
    value.get(key).and_then(Value::as_array).unwrap()
}

impl Fixture {
    /// Build a hub with the agents `names` on disk (not started yet).
    /// `hub_toml` is appended to the hub config after the timezone.
    async fn new(names: &[&str], hub_toml: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(
            hub_dir.join("config.toml"),
            format!("timezone = \"UTC\"\n{hub_toml}"),
        )
        .unwrap();
        let mut mocks = BTreeMap::new();
        for name in names {
            let server = MockServer::start().await;
            mount_reply(&server, &format!("{name} here"), Duration::ZERO).await;
            write_agent(root.path(), name, &server.uri());
            mocks.insert((*name).to_string(), server);
        }
        let hub = HubConfig::load_at(&hub_dir).unwrap();
        let services = HubServices::for_tests(root.path(), &hub).await;
        let host = AgentHost::new(services.clone(), hub);
        host.discover().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (reload_tx, _reload_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = build_app(&host, &services, reload_tx, None).unwrap();
        crate::util::spawn_in_span(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            root,
            host,
            services,
            addr,
            mocks,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn get(&self, path: &str) -> (u16, String) {
        let response = self.http.get(self.url(path)).send().await.unwrap();
        (
            response.status().as_u16(),
            response.text().await.unwrap_or_default(),
        )
    }

    /// Send `text` to the agent over its WebSocket and return its reply.
    async fn chat(&self, name: &str, text: &str) -> String {
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{}/api/agents/{name}/ws", self.addr))
                .await
                .unwrap();
        ws.send(WsMessage::text(
            json!({ "type": "send_message", "id": "m1", "content": text }).to_string(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(POLL_TIMEOUT, async {
            while let Some(frame) = ws.next().await {
                let WsMessage::Text(raw) = frame.unwrap() else {
                    continue;
                };
                let value: Value = serde_json::from_str(&raw).unwrap();
                if value.get("type") == Some(&json!("response")) {
                    return str_at(&value, "content").to_string();
                }
            }
            panic!("the WebSocket closed before the agent replied");
        })
        .await
        .expect("the agent replies within the timeout")
    }

    /// Start an artifact session in the agent and return its address.
    async fn start_session(&self, name: &str, prompt: &str) -> String {
        let response = self
            .http
            .post(self.url(&format!("/api/agents/{name}/sessions")))
            .header("x-residuum-artifact", "test-artifact")
            .json(&json!({ "prompt": prompt }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 202);
        let body: Value = response.json().await.unwrap();
        str_at(&body, "address").to_string()
    }

    /// The `(live states, completed count)` of the agent's sessions.
    async fn sessions(&self, name: &str) -> (Vec<String>, usize) {
        let (status, body) = self.get(&format!("/api/agents/{name}/sessions")).await;
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(&body).unwrap();
        let live = array_at(&value, "live")
            .iter()
            .map(|session| str_at(session, "state").to_string())
            .collect();
        (live, array_at(&value, "completed").len())
    }

    /// Send `name` a request on a repair route that logs, an unauthenticated
    /// webhook delivery, and an A2A message. Their handlers, and the tasks
    /// they spawn, are outside the agent's running router.
    async fn exercise_repair_webhook_and_a2a_routes(&self, name: &str) {
        let rejected = self
            .http
            .patch(self.url(&format!("/api/agents/{name}/config/patch")))
            .json(&json!({ "webhooks": { "$inline": {}, "extra": 1 } }))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status().as_u16(), 400);

        let webhook = self
            .http
            .post(self.url(&format!("/webhook/{name}/deploy")))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(webhook.status().as_u16(), 401);

        let message = a2a::SendMessageRequest {
            message: a2a::Message::new(a2a::Role::User, vec![a2a::Part::text("hello over a2a")]),
            configuration: None,
            metadata: None,
            tenant: None,
        };
        let router = self.host.agent_a2a_router(name).unwrap();
        let request = axum::http::Request::post("/")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "SendMessage",
                    "params": message,
                })
                .to_string(),
            ))
            .unwrap();
        let response = tower::ServiceExt::oneshot(router, request).await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
    }

    fn state_of(&self, name: &str) -> AgentState {
        self.host.summary(name).unwrap().state
    }

    fn mock(&self, name: &str) -> &MockServer {
        self.mocks
            .get(name)
            .expect("the fixture has a mock model for every agent")
    }

    fn activity_of(&self, name: &str) -> AgentActivity {
        self.host
            .activity()
            .into_iter()
            .find(|(agent, _)| agent == name)
            .map(|(_, activity)| activity)
            .unwrap()
    }
}

/// Poll `check` until it returns `Some`, or fail after the timeout.
async fn eventually<T, F, Fut>(what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        if let Some(found) = check().await {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn drain_events(rx: &mut broadcast::Receiver<HubEvent>) -> Vec<HubEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// The `(agent, state)` of every `agent_state` event in `events`.
fn state_changes(events: &[HubEvent]) -> Vec<(String, AgentState)> {
    events
        .iter()
        .filter_map(|event| match event {
            HubEvent::AgentState { agent } => Some((agent.name.clone(), agent.state)),
            HubEvent::AgentCreated { .. }
            | HubEvent::AgentDeleted { .. }
            | HubEvent::AgentActivity { .. }
            | HubEvent::Notice { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn each_agent_serves_its_own_routes_with_separate_memory_and_sessions() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    assert_eq!(hub.state_of("scout"), AgentState::Running);

    for name in ["atlas", "scout"] {
        let (status, body) = hub.get(&format!("/api/agents/{name}/status")).await;
        assert_eq!(status, 200, "{name}: {body}");
        assert!(body.contains("running"), "{name}: {body}");
    }

    assert_eq!(hub.chat("scout", "hello scout").await, "scout here");
    assert_eq!(hub.chat("atlas", "hello atlas").await, "atlas here");

    // Memory is per agent: each history holds only its own conversation.
    let (_, scout_history) = hub.get("/api/agents/scout/chat/history").await;
    let (_, atlas_history) = hub.get("/api/agents/atlas/chat/history").await;
    assert!(scout_history.contains("hello scout"), "{scout_history}");
    assert!(!scout_history.contains("hello atlas"), "{scout_history}");
    assert!(atlas_history.contains("hello atlas"), "{atlas_history}");
    assert!(!atlas_history.contains("hello scout"), "{atlas_history}");
    for name in ["atlas", "scout"] {
        let recent = hub
            .root
            .path()
            .join(name)
            .join("memory")
            .join("recent_messages.json");
        assert!(recent.is_file(), "{name} keeps its own recent messages");
    }

    // Sessions are per agent too.
    hub.start_session("scout", "look something up").await;
    eventually("scout's session to finish its turn", || async {
        (hub.sessions("scout").await.0 == ["idle"]).then_some(())
    })
    .await;
    assert_eq!(hub.sessions("atlas").await, (Vec::new(), 0));
}

#[tokio::test]
async fn an_unknown_agent_is_404_and_a_stopped_one_is_409_except_for_repair_routes() {
    let hub = Fixture::new(&["scout"], "").await;

    let (unknown_status, unknown_body) = hub.get("/api/agents/nobody/status").await;
    assert_eq!(unknown_status, 404);
    assert!(
        unknown_body.contains("no agent named 'nobody'"),
        "{unknown_body}"
    );

    let (stopped_status, stopped_body) = hub.get("/api/agents/scout/sessions").await;
    assert_eq!(stopped_status, 409);
    let value: Value = serde_json::from_str(&stopped_body).unwrap();
    assert_eq!(str_at(&value, "state"), "stopped");
    assert_eq!(str_at(&value, "error"), "scout is stopped");

    // The config route still answers, so a stopped agent can be repaired.
    let (repair_status, repair_body) = hub.get("/api/agents/scout/config/raw").await;
    assert_eq!(repair_status, 200, "{repair_body}");
}

#[tokio::test]
async fn stopping_one_agent_leaves_the_other_serving() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    let mut events = hub.host.subscribe();

    let summary = hub.host.stop("scout").await.unwrap();

    assert_eq!(summary.state, AgentState::Stopped);
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    let (stopped_status, _) = hub.get("/api/agents/scout/sessions").await;
    assert_eq!(stopped_status, 409);
    let (running_status, _) = hub.get("/api/agents/atlas/sessions").await;
    assert_eq!(running_status, 200);
    assert_eq!(hub.chat("atlas", "still there?").await, "atlas here");
    assert_eq!(
        state_changes(&drain_events(&mut events)),
        [("scout".to_string(), AgentState::Stopped)],
        "only the stopped agent's state changed"
    );

    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.chat("scout", "back?").await, "scout here");
}

#[tokio::test]
async fn starting_publishes_starting_then_running_in_order() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut events = hub.host.subscribe();

    hub.host.start("scout").await.unwrap();
    hub.host.restart("scout").await.unwrap();

    assert_eq!(
        state_changes(&drain_events(&mut events)),
        [
            ("scout".to_string(), AgentState::Starting),
            ("scout".to_string(), AgentState::Running),
            ("scout".to_string(), AgentState::Stopped),
            ("scout".to_string(), AgentState::Starting),
            ("scout".to_string(), AgentState::Running),
        ]
    );
}

#[tokio::test]
async fn a_panic_in_one_agents_loop_fails_only_that_agent() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    // Point the hub's bug reports at a mock, so the automatic report for the
    // crash can be seen arriving.
    let reports = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/bug-report"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "public_id": "RR-TEST",
            "submitted_at": "2026-01-01T00:00:00Z"
        })))
        .mount(&reports)
        .await;
    hub.services
        .tracing_service
        .update_config(crate::config::TracingConfig {
            auto_error_reporting: true,
            feedback_endpoint: reports.uri(),
            ..crate::config::TracingConfig::default()
        })
        .await;
    hub.host.start_autostart().await;
    let mut events = hub.host.subscribe();

    let slot = hub.host.slot("scout").unwrap();
    let command_tx = {
        // The panic hook is a server command; send it over the WebSocket.
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
                .await
                .unwrap();
        ws.send(WsMessage::text(
            json!({ "type": "server_command", "name": "panic_for_test" }).to_string(),
        ))
        .await
        .unwrap();
        ws
    };

    eventually("scout to be marked failed", || async {
        (hub.state_of("scout") == AgentState::Failed).then_some(())
    })
    .await;
    drop(command_tx);

    let summary = hub.host.summary("scout").unwrap();
    let last_error = summary
        .last_error
        .expect("a failed agent carries its error");
    assert!(
        last_error.message.contains("crashed"),
        "the message is plain language: {}",
        last_error.message
    );
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    assert_eq!(hub.chat("atlas", "you ok?").await, "atlas here");
    let (status, body) = hub.get("/api/agents/scout/sessions").await;
    assert_eq!(status, 409);
    assert!(body.contains("failed"), "{body}");
    assert!(
        state_changes(&drain_events(&mut events))
            .contains(&("scout".to_string(), AgentState::Failed)),
        "the failure is published on the hub bus"
    );
    // The failure was also auto-reported, naming the agent, and left in the
    // agent's inbox.
    eventually("the automatic bug report", || async {
        let received = reports.received_requests().await.unwrap_or_default();
        received
            .iter()
            .any(|request| String::from_utf8_lossy(&request.body).contains("agent 'scout'"))
            .then_some(())
    })
    .await;
    let inbox = WorkspaceLayout::new(&slot.dir).user_inbox_dir();
    assert!(
        std::fs::read_dir(&inbox).is_ok_and(|entries| entries.count() > 0),
        "the failure is in scout's inbox"
    );

    // A failed agent restarts only when someone restarts it.
    hub.host.restart("scout").await.unwrap();
    assert_eq!(hub.chat("scout", "recovered?").await, "scout here");
}

#[tokio::test]
async fn sessions_from_both_agents_queue_on_the_one_shared_budget() {
    let hub = Fixture::new(&["atlas", "scout"], "[background]\nmax_concurrent = 1\n").await;
    assert_eq!(hub.services.session_budget.available_permits(), 1);
    hub.host.start_autostart().await;

    // The single permit is taken, so neither agent's session can run.
    let held = Arc::clone(&hub.services.session_budget)
        .acquire_owned()
        .await
        .unwrap();
    hub.start_session("scout", "first task").await;
    hub.start_session("atlas", "second task").await;
    for name in ["atlas", "scout"] {
        eventually(&format!("{name}'s session to queue"), || async {
            (hub.sessions(name).await.0 == ["queued"]).then_some(())
        })
        .await;
    }

    drop(held);
    for name in ["atlas", "scout"] {
        eventually(&format!("{name}'s session to run its turn"), || async {
            (hub.sessions(name).await.0 == ["idle"]).then_some(())
        })
        .await;
    }
}

#[tokio::test]
async fn patch_persists_to_the_config_and_reloads_the_agent() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let mut events = hub.host.subscribe();
    let reloads_before = {
        let slot = hub.host.slot("scout").unwrap();
        let guard = slot.lock();
        *guard.running.as_ref().unwrap().control.reload_done.borrow()
    };

    let summary = hub
        .host
        .patch(
            "scout",
            AgentPatch {
                autostart: Some(false),
                a2a_visibility: Some(A2aVisibility::Private),
            },
        )
        .await
        .unwrap();

    assert!(!summary.autostart);
    assert_eq!(summary.a2a_visibility, A2aVisibility::Private);
    let written = std::fs::read_to_string(
        hub.root
            .path()
            .join("scout")
            .join("config")
            .join("config.toml"),
    )
    .unwrap();
    assert!(written.contains("autostart = false"), "{written}");
    assert!(written.contains("visibility = \"private\""), "{written}");
    let reloads_after = {
        let slot = hub.host.slot("scout").unwrap();
        let guard = slot.lock();
        *guard.running.as_ref().unwrap().control.reload_done.borrow()
    };
    assert!(reloads_after > reloads_before, "the agent reloaded");
    assert_eq!(hub.state_of("scout"), AgentState::Running);
    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentState { agent }
                if !agent.autostart && agent.a2a_visibility == A2aVisibility::Private
        )),
        "the change is published"
    );
    // The write was checkpointed in the agent's own config repository.
    let (_, status) = hub.get("/api/agents/scout/status").await;
    let status: Value = serde_json::from_str(&status).unwrap();
    let count = status
        .get("checkpoints")
        .and_then(|checkpoints| checkpoints.get("agent_config"))
        .and_then(|repo| repo.get("checkpoint_count"))
        .and_then(Value::as_u64);
    assert_eq!(count, Some(1));

    let empty = hub.host.patch("scout", AgentPatch::default()).await;
    assert!(matches!(empty, Err(LifecycleError::InvalidRequest(_))));
    let unknown = hub
        .host
        .patch(
            "nobody",
            AgentPatch {
                autostart: Some(true),
                a2a_visibility: None,
            },
        )
        .await;
    assert!(matches!(unknown, Err(LifecycleError::NotFound(_))));
}

#[tokio::test]
async fn patching_a_stopped_agent_writes_its_config_without_starting_it() {
    let hub = Fixture::new(&["scout"], "").await;

    let summary = hub
        .host
        .patch(
            "scout",
            AgentPatch {
                autostart: Some(false),
                a2a_visibility: None,
            },
        )
        .await
        .unwrap();

    assert!(!summary.autostart);
    assert_eq!(summary.state, AgentState::Stopped);
    // Not autostarted any more.
    hub.host.start_autostart().await;
    assert_eq!(hub.state_of("scout"), AgentState::Stopped);
}

#[tokio::test]
async fn unread_counts_messages_while_no_client_is_connected_and_resets_on_connect() {
    let hub = Fixture::new(&["scout"], "").await;
    // A slow model, so the client can leave before the reply is published.
    hub.mock("scout").reset().await;
    mount_reply(hub.mock("scout"), "scout here", Duration::from_millis(600)).await;
    hub.host.start("scout").await.unwrap();
    let mut events = hub.host.subscribe();

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    ws.send(WsMessage::text(
        json!({ "type": "send_message", "id": "m1", "content": "ping" }).to_string(),
    ))
    .await
    .unwrap();
    eventually("scout to be busy", || async {
        hub.activity_of("scout").busy.then_some(())
    })
    .await;
    ws.close(None).await.unwrap();
    drop(ws);

    eventually("the unread reply", || async {
        let activity = hub.activity_of("scout");
        (!activity.busy && activity.unread == 1).then_some(())
    })
    .await;

    // A client connecting resets it.
    let (_ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{}/api/agents/scout/ws", hub.addr))
            .await
            .unwrap();
    eventually("the unread count to reset", || async {
        (hub.activity_of("scout").unread == 0).then_some(())
    })
    .await;

    let activities: Vec<AgentActivity> = drain_events(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            HubEvent::AgentActivity { name, activity } if name == "scout" => Some(activity),
            HubEvent::AgentActivity { .. }
            | HubEvent::AgentState { .. }
            | HubEvent::AgentCreated { .. }
            | HubEvent::AgentDeleted { .. }
            | HubEvent::Notice { .. } => None,
        })
        .collect();
    let busy_at = activities.iter().position(|a| a.busy).unwrap();
    let unread_at = activities.iter().position(|a| a.unread == 1).unwrap();
    let reset_at = activities.iter().rposition(|a| a.unread == 0).unwrap();
    assert!(
        busy_at < unread_at && unread_at < reset_at,
        "{activities:?}"
    );
}

#[tokio::test]
async fn stopping_an_agent_records_its_live_sessions_as_interrupted() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.mock("scout").reset().await;
    mount_reply(hub.mock("scout"), "slow", Duration::from_secs(30)).await;
    hub.host.start("scout").await.unwrap();
    hub.start_session("scout", "a long task").await;
    eventually("the session to run", || async {
        (hub.sessions("scout").await.0 == ["running"]).then_some(())
    })
    .await;

    tokio::time::timeout(Duration::from_secs(60), hub.host.stop("scout"))
        .await
        .expect("stop returns once sessions are recorded")
        .unwrap();
    hub.host.start("scout").await.unwrap();

    let (live, completed) = hub.sessions("scout").await;
    assert!(live.is_empty(), "no session survived the stop: {live:?}");
    assert_eq!(completed, 1, "the interrupted session was recorded");
    let (_, body) = hub.get("/api/agents/scout/sessions").await;
    let listing: Value = serde_json::from_str(&body).unwrap();
    let outcome = array_at(&listing, "completed")
        .first()
        .map(|run| str_at(run, "outcome"));
    assert_eq!(outcome, Some("cancelled"), "the run was stopped, not lost");
}

/// Collects, for each event, its target and whether it or any span around it
/// carries an `agent` field.
#[derive(Clone, Default)]
struct AgentFieldCapture {
    events: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
}

/// One captured log event.
#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    agent: Option<String>,
    message: String,
    /// The `task` field of the monitored task the event ran in, if any.
    task: Option<String>,
    /// The names of the spans around the event, outermost first.
    spans: Vec<String>,
}

struct AgentSpanField(String);

struct TaskSpanField(String);

#[derive(Default)]
struct AgentFieldVisitor {
    agent: Option<String>,
    task: Option<String>,
    message: String,
}

impl tracing::field::Visit for AgentFieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "agent" => self.agent = Some(value.to_string()),
            "task" => self.task = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "agent" => self.agent = Some(format!("{value:?}")),
            "message" => self.message = format!("{value:?}"),
            _ => {}
        }
    }
}

impl<S> tracing_subscriber::Layer<S> for AgentFieldCapture
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = AgentFieldVisitor::default();
        attrs.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            if let Some(agent) = visitor.agent {
                span.extensions_mut().insert(AgentSpanField(agent));
            }
            if let Some(task) = visitor.task {
                span.extensions_mut().insert(TaskSpanField(task));
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = AgentFieldVisitor::default();
        event.record(&mut visitor);
        let agent = visitor.agent.or_else(|| {
            ctx.event_scope(event).and_then(|scope| {
                scope.from_root().find_map(|span| {
                    span.extensions()
                        .get::<AgentSpanField>()
                        .map(|f| f.0.clone())
                })
            })
        });
        let task = ctx.event_scope(event).and_then(|scope| {
            scope.from_root().find_map(|span| {
                span.extensions()
                    .get::<TaskSpanField>()
                    .map(|f| f.0.clone())
            })
        });
        self.events.lock().unwrap().push(CapturedEvent {
            target: event.metadata().target().to_string(),
            agent,
            message: visitor.message,
            task,
            spans: ctx
                .event_scope(event)
                .map(|scope| {
                    scope
                        .from_root()
                        .map(|span| span.name().to_string())
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
}

/// Hub-level code logs about the process as a whole, not on an agent's
/// behalf; everything else that logs during an agent's life must say whose.
fn is_hub_level(event: &CapturedEvent) -> bool {
    [
        "residuum::hub",
        "residuum::tunnel",
        "residuum::update",
        "residuum::daemon",
    ]
    .iter()
    .any(|prefix| event.target.starts_with(prefix))
        || event.task.as_deref() == Some("a2a-sibling-discovery")
        || event.spans.iter().any(|span| span == "team_feed")
}

/// The test binary run as a child process, so the scenario below has the whole
/// process's tracing to itself: a subscriber installed for one test thread
/// shares callsite bookkeeping with the tests running beside it, which makes
/// span capture unreliable there.
#[test]
fn every_log_line_an_agent_produces_carries_its_agent_field() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "hub::host::tests::agent_log_lines_scenario",
            "--nocapture",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the scenario failed or didn't run:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Start two agents, chat with both, and stop them, then check every log
/// line the agents produced names its agent. Run by the test above.
#[test]
#[ignore = "run by every_log_line_an_agent_produces_carries_its_agent_field in its own process"]
fn agent_log_lines_scenario() {
    let capture = AgentFieldCapture::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(capture.clone()))
        .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let hub = Fixture::new(&["atlas", "scout"], "").await;
        std::fs::write(
            hub.root
                .path()
                .join("scout")
                .join("config")
                .join("config.toml"),
            "[webhooks.deploy]\nsecret = \"s3cret\"\n",
        )
        .unwrap();
        // Building the hub is the hub's own doing; what follows is the agents'.
        capture.events.lock().unwrap().clear();

        hub.host.start_autostart().await;
        hub.chat("scout", "hello").await;
        hub.chat("atlas", "hello").await;
        hub.exercise_repair_webhook_and_a2a_routes("scout").await;
        hub.host.stop_all().await;
    });

    let events = capture.events.lock().unwrap().clone();
    let agent_events: Vec<_> = events
        .iter()
        .filter(|event| event.target.starts_with("residuum") && !is_hub_level(event))
        .collect();
    assert!(
        agent_events.len() > 10,
        "the agents logged while starting, chatting, and stopping: {}",
        agent_events.len()
    );
    let untagged: Vec<String> = agent_events
        .iter()
        .filter(|event| event.agent.is_none())
        .map(|event| format!("{}: {} in {:?}", event.target, event.message, event.spans))
        .collect();
    assert!(
        untagged.is_empty(),
        "log lines without an agent: {untagged:#?}"
    );
    let agents: std::collections::BTreeSet<_> = agent_events
        .iter()
        .filter_map(|event| event.agent.clone())
        .collect();
    assert_eq!(
        agents,
        ["atlas".to_string(), "scout".to_string()]
            .into_iter()
            .collect()
    );
}

#[tokio::test]
async fn a_user_edit_through_the_repair_routes_is_attributed_to_the_user() {
    let hub = Fixture::new(&["scout"], "").await;
    let wiki = hub.host.team_paths().wiki_dir();
    std::fs::create_dir_all(&wiki).unwrap();
    let note = wiki.join("note.md");
    std::fs::write(&note, "as the agent read it").unwrap();
    let agent_view = hub
        .services
        .team
        .view_for_agent("scout", hub.root.path().join("scout"));
    let seen = hub.services.team.stamp(&note).await.unwrap();

    let saved = hub
        .http
        .put(hub.url("/api/agents/scout/workspace/file"))
        .json(&json!({ "path": "team/wiki/note.md", "content": "edited by the user, longer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status().as_u16(), 200);

    let err = agent_view
        .lock_unchanged(&note, Some(&seen))
        .await
        .err()
        .expect("the agent's stale write conflicts with the user's edit");
    assert!(err.contains("the user"), "{err}");
}

#[tokio::test]
async fn discovery_lists_every_agent_and_role_lines_come_from_the_wiki() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    let team = hub.host.team_paths();
    std::fs::create_dir_all(team.wiki_dir().join("agents")).unwrap();
    std::fs::write(
        team.agent_role_page("scout"),
        "---\ndescription: Finds things out\n---\n\nScout's page.\n",
    )
    .unwrap();

    let listed = hub.host.list();

    assert_eq!(
        listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["atlas", "scout"]
    );
    let roles: Vec<Option<&str>> = listed.iter().map(|a| a.role.as_deref()).collect();
    assert_eq!(roles, [None, Some("Finds things out")]);
    assert!(listed.iter().all(|a| a.autostart));
    assert!(
        listed
            .iter()
            .all(|a| a.a2a_visibility == A2aVisibility::Public)
    );
}

fn create_request(name: &str, description: Option<&str>) -> CreateAgentRequest {
    CreateAgentRequest {
        name: name.to_string(),
        description: description.map(str::to_string),
        models_from: Some("scout".to_string()),
        providers_toml: None,
        a2a_visibility: None,
    }
}

/// Whether any request the model server got so far mentions `needle`.
async fn model_was_told(server: &MockServer, needle: &str) -> bool {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|request| String::from_utf8_lossy(&request.body).contains(needle))
}

#[tokio::test]
async fn creating_an_agent_writes_it_starts_it_and_briefs_it() {
    let hub = Fixture::new(&["scout"], "").await;
    let mut events = hub.host.subscribe();

    let summary = hub
        .host
        .create(
            create_request("nova", Some("keeps the wiki tidy")),
            Actor::User,
        )
        .await
        .unwrap();

    assert_eq!(summary.name, "nova");
    assert_eq!(summary.state, AgentState::Running);
    assert_eq!(summary.a2a_visibility, A2aVisibility::Private);
    assert_eq!(summary.role.as_deref(), Some("keeps the wiki tidy"));
    let nova = hub.root.path().join("nova");
    assert!(nova.join("config").join("config.toml").is_file());
    assert!(hub.host.team_paths().agent_role_page("nova").is_file());
    assert!(
        !nova.join("BOOTSTRAP.md").exists(),
        "a created agent skips the first-run interview"
    );
    assert_eq!(hub.host.list().len(), 2);
    assert_eq!(hub.chat("nova", "hello nova").await, "scout here");

    // The description reached the new agent's main conversation as its first
    // message, so it can write its own notes.
    eventually("the description to reach the model", || async {
        model_was_told(hub.mock("scout"), "keeps the wiki tidy")
            .await
            .then_some(())
    })
    .await;
    assert!(model_was_told(hub.mock("scout"), "You were just created").await);

    let published = drain_events(&mut events);
    assert_eq!(
        state_changes(&published),
        [
            ("nova".to_string(), AgentState::Starting),
            ("nova".to_string(), AgentState::Running),
        ]
    );
    assert!(
        published.iter().any(|event| matches!(
            event,
            HubEvent::AgentCreated { agent, by: Actor::User } if agent.name == "nova"
        )),
        "the creation is published with who did it"
    );
}

#[tokio::test]
async fn an_agent_created_by_another_agent_is_briefed_in_that_agents_name() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();

    hub.host
        .create(
            create_request("nova", Some("triages the inbox")),
            Actor::Agent("scout".to_string()),
        )
        .await
        .unwrap();

    eventually("the creator's message to reach the model", || async {
        model_was_told(hub.mock("scout"), "agent:scout")
            .await
            .then_some(())
    })
    .await;
    assert!(model_was_told(hub.mock("scout"), "triages the inbox").await);
}

#[tokio::test]
async fn the_agent_that_creates_or_deletes_an_agent_gets_an_inbox_item_about_it() {
    let hub = Fixture::new(&["scout"], "").await;
    let inbox = WorkspaceLayout::new(hub.root.path().join("scout")).user_inbox_dir();
    let items = || std::fs::read_dir(&inbox).map_or(0, Iterator::count);
    hub.host.start("scout").await.unwrap();
    let before = items();

    hub.host
        .create(
            create_request("nova", None),
            Actor::Agent("scout".to_string()),
        )
        .await
        .unwrap();
    let after_create = items();
    hub.host
        .delete("nova", Actor::Agent("scout".to_string()))
        .await
        .unwrap();
    let after_delete = items();
    hub.host
        .create(create_request("kit", None), Actor::User)
        .await
        .unwrap();

    assert_eq!(after_create, before + 1, "creating left an item");
    assert_eq!(after_delete, after_create + 1, "deleting left an item");
    assert_eq!(items(), after_delete, "the user's own actions leave none");
}

#[tokio::test]
async fn creating_an_agent_refuses_bad_requests_before_writing_anything() {
    let hub = Fixture::new(&["scout"], "").await;

    let bad_name = hub
        .host
        .create(create_request("Not A Name", None), Actor::User)
        .await;
    let taken = hub
        .host
        .create(create_request("scout", None), Actor::User)
        .await;
    let no_models = hub
        .host
        .create(
            CreateAgentRequest {
                models_from: None,
                ..create_request("nova", None)
            },
            Actor::User,
        )
        .await;
    let unknown_source = hub
        .host
        .create(
            CreateAgentRequest {
                models_from: Some("nobody".to_string()),
                ..create_request("nova", None)
            },
            Actor::User,
        )
        .await;

    assert!(matches!(bad_name, Err(LifecycleError::InvalidName(_))));
    assert!(matches!(taken, Err(LifecycleError::AlreadyExists(_))));
    assert!(matches!(no_models, Err(LifecycleError::InvalidRequest(_))));
    assert!(matches!(unknown_source, Err(LifecycleError::NotFound(_))));
    assert_eq!(hub.host.list().len(), 1);
    assert!(!hub.root.path().join("nova").exists());
}

#[tokio::test]
async fn deleting_an_agent_stops_it_and_keeps_its_history_for_a_restore() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    hub.chat("scout", "remember this").await;
    let mut events = hub.host.subscribe();

    let outcome = hub.host.delete("scout", Actor::User).await.unwrap();

    assert!(outcome.deleted);
    let checkpoint_id = outcome
        .checkpoint_id
        .expect("the directory was checkpointed");
    assert!(matches!(
        hub.host.summary("scout"),
        Err(LifecycleError::NotFound(_))
    ));
    assert!(!hub.root.path().join("scout").exists());
    assert!(!hub.host.team_paths().agent_role_page("scout").exists());
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    assert_eq!(hub.chat("atlas", "still here?").await, "atlas here");
    let published = drain_events(&mut events);
    assert!(
        published.iter().any(|event| matches!(
            event,
            HubEvent::AgentDeleted { name, by: Actor::User } if name == "scout"
        )),
        "the deletion is published with who did it"
    );

    // The agent's checkpoint history outlived it: an engine rebuilt from its
    // name and paths restores the directory.
    let checkpoints_dir = crate::config::HubPaths::new(&hub.services.hub_dir).checkpoints_dir();
    let engine = crate::checkpoints::CheckpointEngine::with_shared_repos(
        Arc::clone(&hub.services.checkpoints),
        "scout",
        hub.root.path().join("scout"),
        hub.root.path().join("scout").join("config"),
        &checkpoints_dir,
        None,
    )
    .unwrap()
    .with_team_coordinator(hub.services.team.clone());
    crate::hub::provision::restore_agent(
        hub.root.path(),
        &hub.host.team_paths(),
        &hub.services.team,
        &TeamWriter::User,
        "scout",
        &engine,
        &checkpoint_id,
    )
    .await
    .unwrap();
    hub.host.adopt("scout");
    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.chat("scout", "am I back?").await, "scout here");
    let (_, history) = hub.get("/api/agents/scout/chat/history").await;
    assert!(history.contains("remember this"), "{history}");
}

#[tokio::test]
async fn two_agents_cannot_hold_the_same_teams_port() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    let port = free_port().await;
    for name in ["atlas", "scout"] {
        std::fs::write(
            hub.root
                .path()
                .join(name)
                .join("config")
                .join("config.toml"),
            format!(
                "[teams]\napp_id = \"app\"\napp_password = \"pw\"\ntenant_id = \"tenant\"\nport = {port}\n"
            ),
        )
        .unwrap();
    }

    hub.host.start("atlas").await.unwrap();
    let second = hub.host.start("scout").await;

    let message = match second {
        Err(LifecycleError::Failed(message)) => message,
        other => panic!("expected the second start to fail, got {other:?}"),
    };
    assert!(message.contains(&format!("port {port}")), "{message}");
    assert!(message.contains("'atlas'"), "{message}");
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    assert_eq!(hub.state_of("scout"), AgentState::Failed);
}

#[tokio::test]
async fn an_agent_with_a_broken_config_fails_alone_with_a_plain_message() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    std::fs::write(
        hub.root
            .path()
            .join("scout")
            .join("config")
            .join("providers.toml"),
        "not valid toml [[[",
    )
    .unwrap();

    hub.host.start_autostart().await;

    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    let scout = hub.host.summary("scout").unwrap();
    assert_eq!(scout.state, AgentState::Failed);
    let message = scout.last_error.unwrap().message;
    assert!(message.contains("scout couldn't start"), "{message}");
    assert!(message.contains("start it again"), "{message}");
}

/// Read frames from `ws` until one of type `frame_type` satisfies `matches`.
async fn frame_where(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    frame_type: &str,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(POLL_TIMEOUT, async {
        while let Some(frame) = ws.next().await {
            let WsMessage::Text(raw) = frame.unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&raw).unwrap();
            if value.get("type") == Some(&json!(frame_type)) && matches(&value) {
                return value;
            }
        }
        panic!("the WebSocket closed before a {frame_type} frame arrived");
    })
    .await
    .unwrap_or_else(|_| panic!("no {frame_type} frame arrived within the timeout"))
}

#[tokio::test]
async fn the_hub_websocket_carries_an_agents_activity_as_it_chats() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    let (mut hub_ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hub/ws", hub.addr))
        .await
        .unwrap();
    let snapshot = frame_where(&mut hub_ws, "agents_snapshot", |_| true).await;
    assert_eq!(array_at(&snapshot, "agents").len(), 2);

    hub.chat("scout", "hello").await;

    let busy = frame_where(&mut hub_ws, "agent_activity", |frame| {
        str_at(frame, "name") == "scout" && frame.get("busy") == Some(&json!(true))
    })
    .await;
    assert_eq!(str_at(&busy, "name"), "scout");
}

#[tokio::test]
async fn one_team_change_feed_serves_every_agent_and_the_artifact_reload_watcher() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    let team = hub.root.path().join("team");
    std::fs::create_dir_all(team.join("wiki")).unwrap();
    std::fs::create_dir_all(team.join("workbench")).unwrap();

    let mut sockets = Vec::new();
    for name in ["atlas", "scout"] {
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{}/api/agents/{name}/ws", hub.addr))
                .await
                .unwrap();
        ws.send(WsMessage::text(
            json!({ "type": "watch_workspace", "prefixes": ["team/wiki"] }).to_string(),
        ))
        .await
        .unwrap();
        sockets.push(ws);
    }
    // The feed is already running; give the watches a moment to settle.
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(team.join("wiki").join("note.md"), "a note").unwrap();
    std::fs::write(team.join("workbench").join("chart.html"), "<p>chart</p>").unwrap();

    for ws in &mut sockets {
        let changed = frame_where(ws, "workspace_changed", |frame| {
            array_at(frame, "changes")
                .iter()
                .any(|change| str_at(change, "path") == "team/wiki/note.md")
        })
        .await;
        assert_eq!(array_at(&changed, "changes").len(), 1, "{changed}");
        let artifact = frame_where(ws, "artifact_updated", |_| true).await;
        assert_eq!(str_at(&artifact, "name"), "chart");
    }
}

// ─── Lifecycle races and fail-closed settings ─────────────────────────

/// Answers an embeddings request with one fixed vector per input text.
struct EmbedReply;

impl wiremock::Respond for EmbedReply {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let count = body
            .get("input")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let data: Vec<Value> = (0..count)
            .map(|index| json!({ "embedding": [0.1, 0.2, 0.3], "index": index }))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }
}

fn teams_config(port: u16) -> String {
    format!(
        "[teams]\napp_id = \"app\"\napp_password = \"pw\"\ntenant_id = \"tenant\"\nport = {port}\n"
    )
}

fn config_path(hub: &Fixture, name: &str) -> std::path::PathBuf {
    hub.root
        .path()
        .join(name)
        .join("config")
        .join("config.toml")
}

fn providers_path(hub: &Fixture, name: &str) -> std::path::PathBuf {
    hub.root
        .path()
        .join(name)
        .join("config")
        .join("providers.toml")
}

fn spawn_restart(host: &Arc<AgentHost>) -> tokio::task::JoinHandle<Result<(), LifecycleError>> {
    let host = Arc::clone(host);
    tokio::spawn(async move { host.restart("bob").await.map(|_| ()) })
}

#[tokio::test]
async fn racing_starts_restarts_and_a_delete_never_resurrect_the_agent() {
    let hub = Fixture::new(&["bob"], "").await;
    let model_url = hub.mock("bob").uri();
    for round in 0..12 {
        if round > 0 {
            crate::hub::test_support::write_agent(hub.root.path(), "bob", &model_url);
            hub.host.adopt("bob");
        }
        // Odd rounds start with a broken model config, so the racing starts
        // fail and go through the failure-to-inbox path.
        if round % 2 == 1 {
            std::fs::write(providers_path(&hub, "bob"), "not valid toml [[[").unwrap();
        }
        let slot = hub.host.slot("bob").unwrap();

        // The delete queues on the agent's lock behind the first restart,
        // and every later operation queues behind the delete already holding
        // the agent's slot.
        let mut ops = Vec::new();
        ops.push(spawn_restart(&hub.host));
        let deleter = Arc::clone(&hub.host);
        ops.push(tokio::spawn(async move {
            deleter.delete("bob", Actor::User).await.map(|_| ())
        }));
        for _ in 0..4 {
            ops.push(spawn_restart(&hub.host));
        }
        for _ in 0..4 {
            let starter = Arc::clone(&hub.host);
            ops.push(tokio::spawn(async move {
                starter.start("bob").await.map(|_| ())
            }));
        }
        for op in ops {
            match op.await.unwrap() {
                Ok(()) | Err(LifecycleError::NotFound(_) | LifecycleError::Failed(_)) => {}
                Err(other) => panic!("unexpected error in round {round}: {other:?}"),
            }
        }

        assert!(
            !hub.root.path().join("bob").exists(),
            "round {round}: the deleted agent's directory was recreated"
        );
        assert!(
            matches!(hub.host.summary("bob"), Err(LifecycleError::NotFound(_))),
            "round {round}: the deleted agent still has a slot"
        );
        assert!(
            slot.lock().running.is_none(),
            "round {round}: a runtime outlived the deleted agent"
        );
    }
}

#[tokio::test]
async fn a_corrupt_or_truncated_config_keeps_the_last_loaded_visibility() {
    let hub = Fixture::new(&["atlas"], "").await;
    let path = config_path(&hub, "atlas");
    std::fs::write(&path, "[a2a]\nvisibility = \"private\"\n").unwrap();
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Private
    );

    std::fs::write(&path, "[a2a\nvisibility = \"priv").unwrap();
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Private,
        "a corrupt config file"
    );

    std::fs::write(&path, "[a2a]\nvisibility = \"priv").unwrap();
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Private,
        "a config truncated in the middle of a value"
    );

    std::fs::write(&path, "").unwrap();
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Private,
        "a config truncated to nothing"
    );

    std::fs::write(&path, "[a2a]\nvisibility = \"public\"\n").unwrap();
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Public,
        "a good config takes effect again"
    );
}

#[tokio::test]
async fn an_agent_whose_config_never_loaded_is_private() {
    let hub = Fixture::new(&["atlas"], "").await;
    crate::hub::test_support::write_agent(hub.root.path(), "ghost", "http://127.0.0.1:1");
    std::fs::write(config_path(&hub, "ghost"), "[a2a\nvisibility = ").unwrap();
    hub.host.adopt("ghost");

    let summary = hub.host.summary("ghost").unwrap();

    assert_eq!(summary.a2a_visibility, A2aVisibility::Private);
    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Public,
        "an empty config is the defaults"
    );
}

#[tokio::test]
async fn a_running_agent_keeps_the_visibility_it_loaded_when_its_file_breaks() {
    let hub = Fixture::new(&["atlas"], "").await;
    std::fs::write(
        config_path(&hub, "atlas"),
        "[a2a]\nvisibility = \"private\"\n",
    )
    .unwrap();
    hub.host.start("atlas").await.unwrap();

    std::fs::write(
        config_path(&hub, "atlas"),
        "[a2a]\nvisibility = \"public\"\n[[[",
    )
    .unwrap();

    assert_eq!(
        hub.host.summary("atlas").unwrap().a2a_visibility,
        A2aVisibility::Private
    );
}

#[tokio::test]
async fn once_the_hub_is_shutting_down_nothing_starts_and_running_agents_still_stop() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start("atlas").await.unwrap();

    hub.host.begin_shutdown();

    for result in [
        hub.host.start("scout").await,
        hub.host.restart("atlas").await,
        hub.host
            .create(create_request("nova", None), Actor::User)
            .await,
    ] {
        assert_eq!(
            result.unwrap_err(),
            LifecycleError::Failed("Residuum is shutting down".to_string())
        );
    }
    assert_eq!(hub.state_of("scout"), AgentState::Stopped);
    assert!(!hub.root.path().join("nova").exists());
    assert_eq!(
        hub.state_of("atlas"),
        AgentState::Running,
        "a refused restart leaves the running agent alone"
    );

    hub.host.stop_all().await;
    assert_eq!(hub.state_of("atlas"), AgentState::Stopped);
}

#[tokio::test]
async fn agents_starting_at_once_cannot_both_take_a_teams_port() {
    let names = ["a", "b", "c", "d"];
    let hub = Fixture::new(&names, "").await;
    let port = free_port().await;
    for name in names {
        std::fs::write(config_path(&hub, name), teams_config(port)).unwrap();
    }

    let starts = names.map(|name| {
        let host = Arc::clone(&hub.host);
        tokio::spawn(async move { host.start(name).await })
    });
    let mut started = 0;
    for start in starts {
        if start.await.unwrap().is_ok() {
            started += 1;
        }
    }

    assert_eq!(started, 1, "exactly one agent gets the port");
    let holder = names
        .into_iter()
        .find(|name| hub.state_of(name) == AgentState::Running)
        .unwrap();

    // Stopping releases the port for the next agent.
    hub.host.stop(holder).await.unwrap();
    let next = names
        .into_iter()
        .find(|name| hub.state_of(name) == AgentState::Failed)
        .unwrap();
    hub.host.start(next).await.unwrap();
    assert_eq!(hub.state_of(next), AgentState::Running);
}

#[tokio::test]
async fn deleting_an_agent_frees_its_teams_port() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    let port = free_port().await;
    for name in ["atlas", "scout"] {
        std::fs::write(config_path(&hub, name), teams_config(port)).unwrap();
    }
    hub.host.start("atlas").await.unwrap();

    hub.host.delete("atlas", Actor::User).await.unwrap();

    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.state_of("scout"), AgentState::Running);
}

#[tokio::test]
async fn a_reload_that_changes_the_teams_port_moves_the_reservation() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    let (first, second) = (free_port().await, free_port().await);
    std::fs::write(config_path(&hub, "atlas"), teams_config(first)).unwrap();
    std::fs::write(config_path(&hub, "scout"), teams_config(first)).unwrap();
    hub.host.start("atlas").await.unwrap();

    std::fs::write(config_path(&hub, "atlas"), teams_config(second)).unwrap();
    let slot = hub.host.slot("atlas").unwrap();
    hub.host.refresh_teams_port(&slot);

    {
        let ports = hub.host.teams_ports.lock().unwrap();
        assert_eq!(ports.get(&second).map(String::as_str), Some("atlas"));
        assert!(!ports.contains_key(&first));
    }
    hub.host.start("scout").await.unwrap();
    assert_eq!(hub.state_of("scout"), AgentState::Running);
}

#[tokio::test]
async fn a_teams_bind_failure_is_a_hub_notice_naming_the_agent_and_port() {
    let hub = Fixture::new(&["atlas"], "").await;
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    std::fs::write(config_path(&hub, "atlas"), teams_config(port)).unwrap();
    let mut events = hub.host.subscribe();

    hub.host.start("atlas").await.unwrap();

    let message = eventually("the Teams bind failure notice", || {
        let found = drain_events(&mut events)
            .into_iter()
            .find_map(|event| match event {
                HubEvent::Notice {
                    message,
                    agent: Some(agent),
                    ..
                } if agent == "atlas" && message.contains("Teams") => Some(message),
                HubEvent::Notice { .. }
                | HubEvent::AgentState { .. }
                | HubEvent::AgentCreated { .. }
                | HubEvent::AgentDeleted { .. }
                | HubEvent::AgentActivity { .. } => None,
            });
        std::future::ready(found)
    })
    .await;
    assert!(message.contains(&port.to_string()), "{message}");
    drop(taken);
}

#[tokio::test]
async fn the_team_wiki_follows_the_agents_embedding_model() {
    let hub = Fixture::new(&["atlas"], "").await;
    assert!(!hub.services.team_wiki.has_vector());
    let embeddings = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(EmbedReply)
        .mount(&embeddings)
        .await;
    let chat_url = hub.mock("atlas").uri();
    let providers = |embedding_line: &str| {
        format!(
            "[providers]\nmock = {{ type = \"openai\", api_key = \"test-key\", url = \"{chat_url}\" }}\nembed = {{ type = \"openai\", api_key = \"test-key\", url = \"{}\" }}\n\n[models]\nmain = \"mock/test-model\"\n{embedding_line}\n",
            embeddings.uri()
        )
    };
    std::fs::write(
        providers_path(&hub, "atlas"),
        providers("embedding = \"embed/test-embed\""),
    )
    .unwrap();

    // The hub opened text-only; the first agent that configures an embedding
    // model starts and the shared index gains vectors.
    hub.host.start("atlas").await.unwrap();
    eventually("the team wiki to gain vectors", || {
        std::future::ready(hub.services.team_wiki.has_vector().then_some(()))
    })
    .await;

    // Removing the model drops the index back to text only.
    std::fs::write(providers_path(&hub, "atlas"), providers("")).unwrap();
    hub.host.refresh_team_embedding().await;
    assert!(!hub.services.team_wiki.has_vector());
}

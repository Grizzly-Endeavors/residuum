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
use crate::hub::test_support::{free_port, mount_reply, write_agent};
use crate::hub::wiring::build_hub_app;

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
        let app = build_hub_app(Arc::clone(&host) as Arc<dyn AgentDirectory>);
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
            .post(self.url(&format!("/api/agents/{name}/api/sessions")))
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
        let (status, body) = self.get(&format!("/api/agents/{name}/api/sessions")).await;
        assert_eq!(status, 200, "{body}");
        let value: Value = serde_json::from_str(&body).unwrap();
        let live = array_at(&value, "live")
            .iter()
            .map(|session| str_at(session, "state").to_string())
            .collect();
        (live, array_at(&value, "completed").len())
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
        let (status, body) = hub.get(&format!("/api/agents/{name}/api/status")).await;
        assert_eq!(status, 200, "{name}: {body}");
        assert!(body.contains("running"), "{name}: {body}");
    }

    assert_eq!(hub.chat("scout", "hello scout").await, "scout here");
    assert_eq!(hub.chat("atlas", "hello atlas").await, "atlas here");

    // Memory is per agent: each history holds only its own conversation.
    let (_, scout_history) = hub.get("/api/agents/scout/api/chat/history").await;
    let (_, atlas_history) = hub.get("/api/agents/atlas/api/chat/history").await;
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

    let (unknown_status, unknown_body) = hub.get("/api/agents/nobody/api/status").await;
    assert_eq!(unknown_status, 404);
    assert!(
        unknown_body.contains("no agent named 'nobody'"),
        "{unknown_body}"
    );

    let (stopped_status, stopped_body) = hub.get("/api/agents/scout/api/sessions").await;
    assert_eq!(stopped_status, 409);
    let value: Value = serde_json::from_str(&stopped_body).unwrap();
    assert_eq!(str_at(&value, "state"), "stopped");
    assert_eq!(str_at(&value, "error"), "scout is stopped");

    // The config route still answers, so a stopped agent can be repaired.
    let (repair_status, repair_body) = hub.get("/api/agents/scout/api/config/raw").await;
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
    let (stopped_status, _) = hub.get("/api/agents/scout/api/sessions").await;
    assert_eq!(stopped_status, 409);
    let (running_status, _) = hub.get("/api/agents/atlas/api/sessions").await;
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
    let (status, body) = hub.get("/api/agents/scout/api/sessions").await;
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
    let (_, status) = hub.get("/api/agents/scout/api/status").await;
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
    let (_, body) = hub.get("/api/agents/scout/api/sessions").await;
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
        // Building the hub is the hub's own doing; what follows is the agents'.
        capture.events.lock().unwrap().clear();

        hub.host.start_autostart().await;
        hub.chat("scout", "hello").await;
        hub.chat("atlas", "hello").await;
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
    let (_, history) = hub.get("/api/agents/scout/api/chat/history").await;
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

// ─── Teamwork messaging ───────────────────────────────────────────────

/// The `(role, content)` of the last chat message in a model request.
fn last_chat_message(request: &wiremock::Request) -> (String, String) {
    let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
    let last = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let text = |key: &str| {
        last.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (text("role"), text("content"))
}

/// How long a scripted model takes to decide on a tool call. A message that
/// reaches a main agent during the last model call of its turn is not acted
/// on until its next turn, so scripted agents pause before replying to let
/// the sender's turn finish first.
const SCRIPTED_TOOL_CALL_DELAY: Duration = Duration::from_millis(300);

/// A model reply that calls `tool` with `arguments`.
fn tool_call_reply(tool: &str, arguments: &Value) -> ResponseTemplate {
    static NEXT_CALL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT_CALL.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    ResponseTemplate::new(200)
        .set_delay(SCRIPTED_TOOL_CALL_DELAY)
        .set_body_json(json!({
            "choices": [{ "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": format!("call_{id}"),
                    "type": "function",
                    "function": { "name": tool, "arguments": arguments.to_string() }
                }]
            } }]
        }))
}

/// Script the model behind `server`: `script` sees the role and content of
/// the last chat message and returns a tool call to make, or `None` to
/// answer with `fallback` text.
async fn mount_script<F>(server: &MockServer, fallback: &str, script: F)
where
    F: Fn(&str, &str) -> Option<(&'static str, Value)> + Send + Sync + 'static,
{
    server.reset().await;
    let fallback = fallback.to_string();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let (role, content) = last_chat_message(request);
            match script(&role, &content) {
                Some((tool, arguments)) => tool_call_reply(tool, &arguments),
                None => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{ "message": { "role": "assistant", "content": fallback } }]
                })),
            }
        })
        .mount(server)
        .await;
}

/// The address a teammate message tells its receiver to reply to.
fn reply_address(message: &str) -> String {
    let (_, rest) = message
        .split_once("to=\"")
        .expect("a teammate message names its reply address");
    rest.split('"').next().unwrap().to_string()
}

fn is_user_message_with(role: &str, content: &str, needle: &str) -> bool {
    role == "user" && content.contains(needle)
}

fn is_teammate_message_with(role: &str, content: &str, needle: &str) -> bool {
    role == "user" && content.contains("[Message from teammate ") && content.contains(needle)
}

/// Start `names`, in order.
async fn start_all(hub: &Fixture, names: &[&str]) {
    for name in names {
        hub.host.start(name).await.unwrap();
    }
}

/// Write beta's role page, so teammates see a role line for it.
fn write_beta_role(hub: &Fixture) {
    let team = hub.host.team_paths();
    std::fs::create_dir_all(team.wiki_dir().join("agents")).unwrap();
    std::fs::write(
        team.agent_role_page("beta"),
        "---\ndescription: Reviews drafts\n---\n\nBeta's page.\n",
    )
    .unwrap();
}

#[tokio::test]
async fn a_teammate_message_is_attributed_and_the_reply_finds_its_way_back() {
    let hub = Fixture::new(&["alpha", "beta"], "").await;
    mount_script(hub.mock("alpha"), "alpha idle", |role, content| {
        is_user_message_with(role, content, "kickoff").then(|| {
            (
                "message_agent",
                json!({ "to": "agent:beta", "message": "ping from alpha" }),
            )
        })
    })
    .await;
    mount_script(hub.mock("beta"), "beta idle", |role, content| {
        is_teammate_message_with(role, content, "ping from alpha").then(|| {
            (
                "message_agent",
                json!({ "to": reply_address(content), "message": "pong from beta" }),
            )
        })
    })
    .await;
    start_all(&hub, &["alpha", "beta"]).await;

    hub.chat("alpha", "kickoff").await;

    eventually("alpha to receive beta's reply", || async {
        model_was_told(hub.mock("alpha"), "pong from beta")
            .await
            .then_some(())
    })
    .await;
    assert!(
        model_was_told(
            hub.mock("beta"),
            "[Message from teammate agent:alpha, not the user."
        )
        .await,
        "beta sees alpha's message labeled as a teammate's, with alpha's address"
    );
    assert!(
        model_was_told(hub.mock("alpha"), "[Message from teammate agent:beta,").await,
        "alpha sees beta's reply attributed to beta"
    );
}

#[tokio::test]
async fn a_sessions_teammate_message_is_answered_at_the_sessions_own_address() {
    let hub = Fixture::new(&["alpha", "beta"], "").await;
    mount_script(hub.mock("alpha"), "alpha idle", |role, content| {
        if is_user_message_with(role, content, "kickoff-session") {
            Some((
                "subagent_spawn",
                json!({ "task": "session-task: ask beta" }),
            ))
        } else if is_user_message_with(role, content, "session-task") {
            Some((
                "message_agent",
                json!({ "to": "agent:beta", "message": "session ping" }),
            ))
        } else {
            None
        }
    })
    .await;
    mount_script(hub.mock("beta"), "beta idle", |role, content| {
        is_teammate_message_with(role, content, "session ping").then(|| {
            (
                "message_agent",
                json!({ "to": reply_address(content), "message": "session pong" }),
            )
        })
    })
    .await;
    start_all(&hub, &["alpha", "beta"]).await;

    hub.chat("alpha", "kickoff-session").await;

    eventually("alpha's session to receive beta's reply", || async {
        model_was_told(hub.mock("alpha"), "session pong")
            .await
            .then_some(())
    })
    .await;
    assert!(
        model_was_told(
            hub.mock("beta"),
            "[Message from teammate agent:alpha/spawned-"
        )
        .await,
        "beta sees the sender as alpha's session, fully qualified"
    );
    assert!(
        model_was_told(hub.mock("alpha"), "[Message from teammate agent:beta,").await,
        "the session sees beta's reply attributed to beta"
    );
    assert!(
        !model_was_told(hub.mock("beta"), "no session at").await,
        "beta's reply found the session"
    );
}

#[tokio::test]
async fn a_two_agent_loop_hits_the_hard_hop_limit_and_the_refusal_is_seen() {
    let hub = Fixture::new(
        &["alpha", "beta"],
        "[background]\nhop_soft_limit = 2\nhop_hard_limit = 5\n",
    )
    .await;
    let bounce = |peer: &'static str| {
        move |role: &str, content: &str| {
            if is_user_message_with(role, content, "kickoff") {
                Some((
                    "message_agent",
                    json!({ "to": format!("agent:{peer}"), "message": "loop" }),
                ))
            } else if is_teammate_message_with(role, content, "loop") {
                Some((
                    "message_agent",
                    json!({ "to": reply_address(content), "message": "loop" }),
                ))
            } else {
                None
            }
        }
    };
    mount_script(hub.mock("alpha"), "alpha done", bounce("beta")).await;
    mount_script(hub.mock("beta"), "beta done", bounce("alpha")).await;
    start_all(&hub, &["alpha", "beta"]).await;

    hub.chat("alpha", "kickoff").await;

    let refusal = "message loop limit reached";
    eventually("one side to be refused at the hard limit", || async {
        let alpha = model_was_told(hub.mock("alpha"), refusal).await;
        let beta = model_was_told(hub.mock("beta"), refusal).await;
        (alpha || beta).then_some(())
    })
    .await;
    let soft_note = "This exchange has reached";
    assert!(
        model_was_told(hub.mock("alpha"), soft_note).await
            || model_was_told(hub.mock("beta"), soft_note).await,
        "the soft limit asked a receiver to reply only if needed"
    );
    // The loop ended: nothing more is sent once both agents settle.
    let total_requests = || async {
        hub.mock("alpha").received_requests().await.unwrap().len()
            + hub.mock("beta").received_requests().await.unwrap().len()
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let settled = total_requests().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(settled, total_requests().await, "the refused loop stops");
}

#[tokio::test]
async fn messaging_a_stopped_or_unknown_teammate_is_a_tool_error_and_queues_nothing() {
    let hub = Fixture::new(&["alpha", "beta"], "").await;
    mount_script(hub.mock("alpha"), "alpha idle", |role, content| {
        if is_user_message_with(role, content, "to-stopped") {
            Some((
                "message_agent",
                json!({ "to": "agent:beta", "message": "anyone home?" }),
            ))
        } else if is_user_message_with(role, content, "to-ghost") {
            Some((
                "message_agent",
                json!({ "to": "agent:ghost", "message": "hello" }),
            ))
        } else if is_user_message_with(role, content, "to-no-session") {
            Some((
                "message_agent",
                json!({ "to": "agent:beta/spawned-nothing-0000", "message": "hello" }),
            ))
        } else {
            None
        }
    })
    .await;
    mount_script(hub.mock("beta"), "beta idle", |_, _| None).await;
    start_all(&hub, &["alpha"]).await;

    hub.chat("alpha", "to-stopped").await;
    hub.chat("alpha", "to-ghost").await;
    let alpha = hub.mock("alpha");
    assert!(
        model_was_told(alpha, "teammate 'beta' is stopped; nothing was queued").await
            && model_was_told(alpha, "they can start it from the team view").await,
        "a stopped teammate is a tool error that says so"
    );
    assert!(
        model_was_told(alpha, "no teammate named 'ghost'").await
            && model_was_told(alpha, "Your teammates are: beta").await,
        "an unknown name lists the teammates"
    );

    // Starting beta later delivers nothing from before: it was never queued.
    hub.host.start("beta").await.unwrap();
    hub.chat("alpha", "to-no-session").await;
    assert!(
        model_was_told(alpha, "has no session 'spawned-nothing-0000'").await,
        "a missing session is a tool error"
    );
    assert!(
        !model_was_told(hub.mock("beta"), "anyone home?").await,
        "nothing queued for the agent while it was stopped"
    );
}

#[tokio::test]
async fn list_agents_shows_teammates_with_state_and_role_and_marks_the_caller() {
    let hub = Fixture::new(&["alpha", "beta"], "").await;
    write_beta_role(&hub);
    mount_script(hub.mock("alpha"), "alpha idle", |role, content| {
        is_user_message_with(role, content, "who is here").then(|| ("list_agents", json!({})))
    })
    .await;
    start_all(&hub, &["alpha"]).await;

    hub.chat("alpha", "who is here").await;

    let alpha = hub.mock("alpha");
    assert!(model_was_told(alpha, "1 teammate(s):").await);
    assert!(
        model_was_told(alpha, "[agent:beta] stopped — Reviews drafts").await,
        "teammates carry their state and role line"
    );
    assert!(
        model_was_told(alpha, "[agent:alpha] (you)").await,
        "the caller is marked"
    );
}

#[tokio::test]
async fn the_team_block_lists_teammates_and_follows_their_state() {
    let hub = Fixture::new(&["alpha", "beta"], "").await;
    write_beta_role(&hub);
    start_all(&hub, &["alpha", "beta"]).await;

    hub.chat("alpha", "first turn").await;
    let alpha = hub.mock("alpha");
    assert!(
        model_was_told(alpha, "<TEAM>").await
            && model_was_told(alpha, "beta (running): Reviews drafts").await,
        "the prompt lists beta as running with its role line"
    );
    assert!(
        !model_was_told(alpha, "alpha (running)").await,
        "the roster lists teammates, not the agent itself"
    );

    hub.host.stop("beta").await.unwrap();
    hub.chat("alpha", "second turn").await;
    let requests = alpha.received_requests().await.unwrap();
    let second_turn_prompt = requests
        .iter()
        .rev()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .find(|body| body.contains("second turn"))
        .expect("alpha's second turn reached the model");
    assert!(
        second_turn_prompt.contains("beta (stopped): Reviews drafts"),
        "the next turn shows the state change"
    );
}

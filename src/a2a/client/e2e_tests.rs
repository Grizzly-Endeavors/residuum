//! End-to-end tests driving the A2A client tools (`message_agent`,
//! `stop_agent`) against a real `a2a-server-lf` server. Lives as a
//! `#[cfg(test)]` unit-test module
//! rather than `tests/*.rs` because it needs `AgentMessenger::new`, which is
//! `pub(crate)`; delivery is asserted through a registered session's
//! interrupt channel, the same fixture pattern `background::messaging`'s own
//! tests use.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use a2a::*;
use a2a_server::*;
use futures_util::stream::BoxStream;
use tokio_util::sync::CancellationToken;

use crate::a2a::client::hub::AgentSource;
use crate::a2a::{A2aClientHub, RemoteTaskTracker};
use crate::agent::HopCounter;
use crate::agent::interrupt::Interrupt;
use crate::background::HopLimits;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::{
    MAIN_ADDRESS, SessionCategory, SessionInfo, SessionRegistry, SessionState,
};
use crate::background::store::SessionStore;
use crate::bus::{EventTrigger, SessionAddress};
use crate::config::BackgroundModelTier;
use crate::tools::Tool;
use crate::tools::background::StopAgentTool;
use crate::tools::message_agent::MessageAgentTool;

/// Longest a test waits for an expected delivery before failing.
const RECV_TIMEOUT: Duration = Duration::from_secs(20);

/// Test executor: a fresh task goes `WORKING` then
/// `INPUT_REQUIRED`; a follow-up (the task already exists) completes with an
/// artifact. A message containing "hold" stays `WORKING` indefinitely, for
/// the cancel test.
struct TestExecutor;

fn status_event(task_id: &str, ctx: &str, state: TaskState, text: Option<&str>) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: ctx.into(),
        status: TaskStatus {
            state,
            message: text.map(|t| {
                let mut m = Message::new(Role::Agent, vec![Part::text(t)]);
                m.task_id = Some(task_id.into());
                m.context_id = Some(ctx.into());
                m
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

impl AgentExecutor for TestExecutor {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let followup = ctx
            .stored_task
            .as_ref()
            .is_some_and(|t| t.status.state == TaskState::InputRequired);
        let text = ctx
            .message
            .as_ref()
            .and_then(Message::text)
            .unwrap_or_default()
            .to_string();
        let (task_id, ctx_id) = (ctx.task_id.clone(), ctx.context_id.clone());
        tokio::spawn(async move {
            if text.contains("direct") {
                // Answer with a bare message: no task is ever opened.
                let reply = Message::new(Role::Agent, vec![Part::text("direct answer")]);
                tx.send(Ok(StreamResponse::Message(reply))).await.ok();
                return;
            }
            tx.send(Ok(status_event(
                &task_id,
                &ctx_id,
                TaskState::Working,
                None,
            )))
            .await
            .ok();
            if text.contains("hold") {
                // Never completes on its own — only a cancel moves it on.
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            if followup {
                tx.send(Ok(StreamResponse::ArtifactUpdate(
                    TaskArtifactUpdateEvent {
                        task_id: task_id.clone(),
                        context_id: ctx_id.clone(),
                        artifact: Artifact {
                            artifact_id: new_artifact_id(),
                            name: Some("report".into()),
                            description: None,
                            parts: vec![Part::text("the report")],
                            metadata: None,
                            extensions: None,
                        },
                        append: None,
                        last_chunk: Some(true),
                        metadata: None,
                    },
                )))
                .await
                .ok();
                tx.send(Ok(status_event(
                    &task_id,
                    &ctx_id,
                    TaskState::Completed,
                    Some("done"),
                )))
                .await
                .ok();
            } else {
                tx.send(Ok(status_event(
                    &task_id,
                    &ctx_id,
                    TaskState::InputRequired,
                    Some("which format?"),
                )))
                .await
                .ok();
            }
        });
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }))
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        Box::pin(futures_util::stream::once(async move {
            Ok(status_event(
                &ctx.task_id,
                &ctx.context_id,
                TaskState::Canceled,
                None,
            ))
        }))
    }
}

/// Spawn a local `a2a-server-lf` server backed by [`TestExecutor`] on a free
/// loopback port, returning its base URL. `streaming` sets the agent card's
/// declared capability, which is what [`RemoteTaskTracker`] decides
/// stream-vs-poll from.
///
/// # Errors
/// Returns an error if the loopback listener can't be bound — an
/// environment problem, not something callers assert against, so they
/// unwrap it themselves (exempt from `unwrap_used` inside a `#[tokio::test]`
/// function, unlike this plain helper).
async fn spawn_agent_server(streaming: bool) -> std::io::Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let handler = Arc::new(
        DefaultRequestHandler::new(TestExecutor, InMemoryTaskStore::new()).with_capabilities(
            AgentCapabilities {
                streaming: Some(true),
                push_notifications: Some(false),
                extensions: None,
                extended_agent_card: None,
            },
        ),
    );
    let card = AgentCard {
        name: "test-agent".into(),
        description: "a test agent".into(),
        version: "1".into(),
        supported_interfaces: vec![AgentInterface::new(
            base.clone(),
            TRANSPORT_PROTOCOL_JSONRPC,
        )],
        capabilities: AgentCapabilities {
            streaming: Some(streaming),
            push_notifications: Some(false),
            extensions: None,
            extended_agent_card: None,
        },
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        skills: vec![],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    };
    let app = axum::Router::new()
        .merge(a2a_server::jsonrpc::jsonrpc_router(handler))
        .merge(a2a_server::agent_card::agent_card_router(Arc::new(
            StaticAgentCard::new(card),
        )));
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    Ok(base)
}

/// Shared fixture: a hub/tracker pair with a fresh workspace, plus the
/// session registry and messenger a tool under test identifies itself
/// through.
struct Fixture {
    hub: Arc<A2aClientHub>,
    tracker: Arc<RemoteTaskTracker>,
    registry: Arc<SessionRegistry>,
    messenger: Arc<AgentMessenger>,
    _dir: tempfile::TempDir,
}

/// # Errors
/// Returns an error if the temp workspace directory can't be created — an
/// environment problem, not something callers assert against, so they
/// unwrap it themselves.
async fn fixture() -> std::io::Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let registry = Arc::new(SessionRegistry::new());
    let bus_handle = crate::bus::spawn_broker();
    let store = Arc::new(SessionStore::new(dir.path().join("sessions")));
    let messenger = Arc::new(AgentMessenger::new(
        Arc::clone(&registry),
        bus_handle.publisher(),
        store,
        HopLimits { soft: 8, hard: 32 },
    ));
    let hub = A2aClientHub::new_shared();
    let tracker = RemoteTaskTracker::load(
        dir.path().join("outbound.json"),
        Arc::clone(&hub),
        Arc::clone(&messenger),
        dir.path().join("inbox"),
    )
    .await;
    Ok(Fixture {
        hub,
        tracker,
        registry,
        messenger,
        _dir: dir,
    })
}

fn sample_info(address: &str) -> SessionInfo {
    SessionInfo {
        address: SessionAddress::from(address),
        run_id: "run-1".to_string(),
        category: SessionCategory::Spawned,
        trigger: EventTrigger::Agent,
        source_label: "agent:test".to_string(),
        state: SessionState::Idle,
        spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
        depth: 1,
        purpose: "test".to_string(),
        agent_skill: None,
        model_tier: BackgroundModelTier::Medium,
        conversation_target: None,
        started_at: chrono::Utc::now(),
        usage: crate::agent::usage::SessionUsageTotals::default(),
        overlap: None,
    }
}

/// The delivered text if `interrupt` is an `AgentMessage`, or a placeholder
/// naming what it actually was — so a caller's `assert!(content.contains(..))`
/// fails with a useful message instead of this helper panicking directly.
fn agent_message_content(interrupt: &Interrupt) -> String {
    match interrupt {
        Interrupt::AgentMessage(event) => event.content.clone(),
        Interrupt::UserMessage(_) => {
            "<expected an AgentMessage interrupt, got UserMessage>".to_string()
        }
        Interrupt::Subconscious(_) => {
            "<expected an AgentMessage interrupt, got Subconscious>".to_string()
        }
        Interrupt::Stopped => "<expected an AgentMessage interrupt, got Stopped>".to_string(),
    }
}

#[tokio::test]
async fn send_and_stream_watch_delivers_result_to_sender() {
    let f = fixture().await.unwrap();
    let url = spawn_agent_server(true).await.unwrap();
    f.hub
        .register_external(
            "agent1".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

    let sender = "spawned-fixture-0001";
    let mut interrupt_rx = f
        .registry
        .register(sample_info(sender), CancellationToken::new())
        .unwrap();

    let tool = MessageAgentTool::new(
        SessionAddress::from(sender),
        "spawned".to_string(),
        Arc::clone(&f.messenger),
        HopCounter::new(0),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );

    let sent_result = tool
        .execute(serde_json::json!({ "to": "a2a:agent1", "message": "start" }))
        .await
        .unwrap();
    assert!(!sent_result.is_error, "got: {}", sent_result.output);
    assert!(
        sent_result
            .output
            .contains("Sent to remote agent a2a:agent1"),
        "got: {}",
        sent_result.output
    );

    let first_interrupt = tokio::time::timeout(RECV_TIMEOUT, interrupt_rx.recv())
        .await
        .expect("timed out waiting for the input_required delivery")
        .unwrap();
    let first_content = agent_message_content(&first_interrupt);
    assert!(
        first_content.contains("input_required"),
        "got: {first_content}"
    );
    assert!(
        first_content.contains("which format?"),
        "got: {first_content}"
    );

    let followup_result = tool
        .execute(serde_json::json!({ "to": "a2a:agent1", "message": "markdown" }))
        .await
        .unwrap();
    assert!(!followup_result.is_error, "got: {}", followup_result.output);

    let second_interrupt = tokio::time::timeout(RECV_TIMEOUT, interrupt_rx.recv())
        .await
        .expect("timed out waiting for the completed delivery")
        .unwrap();
    let second_content = agent_message_content(&second_interrupt);
    assert!(
        second_content.contains("completed"),
        "got: {second_content}"
    );
    assert!(
        second_content.contains("the report"),
        "got: {second_content}"
    );

    assert!(
        f.tracker
            .any_open_task_for(sender, "agent1")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn poll_fallback_when_card_declares_no_streaming() {
    let f = fixture().await.unwrap();
    let url = spawn_agent_server(false).await.unwrap();
    f.hub
        .register_external(
            "agent1".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

    let sender = "spawned-fixture-0002";
    let mut interrupt_rx = f
        .registry
        .register(sample_info(sender), CancellationToken::new())
        .unwrap();

    let tool = MessageAgentTool::new(
        SessionAddress::from(sender),
        "spawned".to_string(),
        Arc::clone(&f.messenger),
        HopCounter::new(0),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );

    let sent_result = tool
        .execute(serde_json::json!({ "to": "a2a:agent1", "message": "start" }))
        .await
        .unwrap();
    assert!(!sent_result.is_error, "got: {}", sent_result.output);

    let first_interrupt = tokio::time::timeout(RECV_TIMEOUT, interrupt_rx.recv())
        .await
        .expect("timed out waiting for the input_required delivery via polling")
        .unwrap();
    assert!(agent_message_content(&first_interrupt).contains("input_required"));

    let followup_result = tool
        .execute(serde_json::json!({ "to": "a2a:agent1", "message": "markdown" }))
        .await
        .unwrap();
    assert!(!followup_result.is_error, "got: {}", followup_result.output);

    let second_interrupt = tokio::time::timeout(RECV_TIMEOUT, interrupt_rx.recv())
        .await
        .expect("timed out waiting for the completed delivery via polling")
        .unwrap();
    assert!(agent_message_content(&second_interrupt).contains("completed"));
}

#[tokio::test]
async fn stop_agent_cancels_the_open_task_exactly_once() {
    let f = fixture().await.unwrap();
    let url = spawn_agent_server(true).await.unwrap();
    f.hub
        .register_external(
            "agent1".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

    let sender = "spawned-fixture-0003";
    let mut interrupt_rx = f
        .registry
        .register(sample_info(sender), CancellationToken::new())
        .unwrap();

    let message_tool = MessageAgentTool::new(
        SessionAddress::from(sender),
        "spawned".to_string(),
        Arc::clone(&f.messenger),
        HopCounter::new(0),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );
    let sent_result = message_tool
        .execute(serde_json::json!({ "to": "a2a:agent1", "message": "hold this open" }))
        .await
        .unwrap();
    assert!(!sent_result.is_error, "got: {}", sent_result.output);

    // Give the executor a moment to reach WORKING before cancelling.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let stop_tool = StopAgentTool::new(
        Arc::clone(&f.registry),
        SessionAddress::from(sender),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );
    let stop_result = stop_tool
        .execute(serde_json::json!({ "address": "a2a:agent1" }))
        .await
        .unwrap();
    assert!(!stop_result.is_error, "got: {}", stop_result.output);
    assert!(
        stop_result.output.contains("Canceling"),
        "got: {}",
        stop_result.output
    );

    let interrupt = tokio::time::timeout(RECV_TIMEOUT, interrupt_rx.recv())
        .await
        .expect("timed out waiting for the canceled delivery")
        .unwrap();
    assert!(agent_message_content(&interrupt).contains("canceled"));

    // The background watcher may have observed the same cancellation
    // independently; `notified_this_turn` must stop it from delivering a
    // second time for this turn.
    let second = tokio::time::timeout(Duration::from_secs(2), interrupt_rx.recv()).await;
    assert!(
        second.is_err(),
        "expected exactly one delivery (a timeout), but a second one arrived"
    );
}

#[tokio::test]
async fn stop_agent_reports_no_open_task_when_none_exists() {
    let f = fixture().await.unwrap();
    let url = spawn_agent_server(true).await.unwrap();
    f.hub
        .register_external(
            "agent1".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

    let stop_tool = StopAgentTool::new(
        Arc::clone(&f.registry),
        SessionAddress::from("main"),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );
    let result = stop_tool
        .execute(serde_json::json!({ "address": "a2a:agent1" }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(
        result.output.contains("no open task"),
        "got: {}",
        result.output
    );
}

#[tokio::test]
async fn a_direct_message_reply_is_returned_as_the_tool_result() {
    let f = fixture().await.unwrap();
    let url = spawn_agent_server(true).await.unwrap();
    f.hub
        .register_external(
            "agent1".to_string(),
            url,
            HashMap::new(),
            AgentSource::Config,
        )
        .await;

    let sender = "spawned-fixture-0001";
    let tool = MessageAgentTool::new(
        SessionAddress::from(sender),
        "spawned".to_string(),
        Arc::clone(&f.messenger),
        HopCounter::new(0),
        Arc::clone(&f.hub),
        Arc::clone(&f.tracker),
    );

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tool.execute(serde_json::json!({ "to": "a2a:agent1", "message": "direct please" })),
    )
    .await
    .expect("a direct reply must not hang the tool")
    .unwrap();
    assert!(!result.is_error, "got: {}", result.output);
    assert!(
        result.output.contains("replied directly") && result.output.contains("direct answer"),
        "got: {}",
        result.output
    );
}

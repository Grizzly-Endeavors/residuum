//! A conversation session's `send_message` to its own chat, run through a
//! real session turn: the tools' `SendMessageTool` posts to the chat, and the
//! turn's final text stays in the session.
//!
//! covers: residuum-background, residuum-tools, residuum-bus

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use residuum_bus::session::MAIN_DEPTH;
use residuum_bus::session::SessionState;
use residuum_bus::testing::{bus_barrier, next_event};
use residuum_bus::{SessionAddress, SessionEventKind, topics};
use residuum_core::testing::log::EventLog;

use crate::agent::ToolRegistry;
use crate::agent::hop::HopLimits;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::background::store::SessionStore;
use crate::background::subagent::SubAgentResources;
use crate::background::testing::test_memory_extras;
use crate::background::types::SubAgentConfig;
use crate::background::{IdleTimeouts, SessionRuntime, SessionRuntimeHandles, SessionSpawnRequest};
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message, ToolCall,
    ToolDefinition,
};
use crate::mcp::McpRegistry;
use crate::skills::{SkillIndex, SkillState};
use crate::workspace::identity::IdentityFiles;

/// Calls `send_message` to discord's `chan-1` on its first completion,
/// then answers with text.
struct SendsToChatProvider {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl InferenceProvider for SendsToChatProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(InferenceResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "tc-1".to_string(),
                    name: "send_message".to_string(),
                    arguments: serde_json::json!({
                        "endpoint": "discord",
                        "conversation": "chan-1",
                        "message": "Ivan says the build is green",
                    }),
                    server: None,
                }],
            ))
        } else {
            Ok(InferenceResponse::new("told the group".to_string(), vec![]))
        }
    }

    fn model_name(&self) -> &'static str {
        "sends-to-chat"
    }
}

struct BuildsChannel;

#[async_trait]
impl crate::bus::conversations::ConversationSource for BuildsChannel {
    async fn conversations(
        &self,
    ) -> anyhow::Result<Vec<crate::bus::conversations::KnownConversation>> {
        Ok(vec![crate::bus::conversations::KnownConversation {
            id: "chan-1".to_string(),
            kind: crate::bus::origin::ConversationKind::Channel,
            label: "#builds".to_string(),
        }])
    }
}

/// A throwaway checkpoint engine for a runtime that never checkpoints.
fn test_checkpoints(dir: &std::path::Path) -> Arc<crate::checkpoints::CheckpointEngine> {
    Arc::new(
        crate::checkpoints::CheckpointEngine::new(
            "test-agent",
            dir.join("workspace"),
            &crate::config::paths::TeamPaths::new(dir.join("team")),
            dir.join("agent-config"),
            dir.join("hub"),
            &dir.join("checkpoints"),
            None,
        )
        .unwrap(),
    )
}

/// A `TracingService` with auto error reporting off.
fn test_tracing_service() -> Arc<crate::tracing_service::TracingService> {
    let (_, handle) = crate::util::telemetry::SpanBufferLayer::new(
        &crate::util::telemetry::SpanBufferConfig::default(),
    );
    Arc::new(crate::tracing_service::TracingService::new(
        crate::config::TracingConfig::default(),
        handle,
    ))
}

fn test_tracing_client_context() -> Arc<crate::tracing_service::ClientContext> {
    Arc::new(crate::tracing_service::ClientContext {
        version: "test".to_string(),
        commit: None,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        model_provider: None,
        model_name: None,
        active_subagents: Vec::new(),
        config_flags: std::collections::BTreeMap::new(),
        agent: None,
    })
}

/// A runtime whose idle window outlasts any test, so a session stays idle
/// until the test delivers to it or stops it.
async fn held_idle_runtime() -> (
    SessionRuntime,
    Arc<SessionRegistry>,
    residuum_bus::Subscriber<residuum_bus::AgentResultEvent>,
    residuum_bus::BusHandle,
) {
    let idle_window = Duration::from_hours(1);
    let bus_handle = residuum_bus::spawn_broker();
    let results = bus_handle.subscribe(topics::Background).await.unwrap();
    let registry = Arc::new(SessionRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    // The store owns the guard, so the directory lives as long as any
    // session task still writing through it.
    let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
    let messenger = Arc::new(AgentMessenger::new(
        Arc::clone(&registry),
        bus_handle.publisher(),
        Arc::clone(&store),
        HopLimits { soft: 8, hard: 32 },
    ));
    let runtime = SessionRuntime::new(
        Arc::clone(&registry),
        store,
        Arc::new(tokio::sync::Semaphore::new(3)),
        IdleTimeouts {
            scheduled: idle_window,
            spawned: idle_window,
            external: idle_window,
            artifact: idle_window,
        },
        SessionRuntimeHandles {
            publisher: bus_handle.publisher(),
            tz: chrono_tz::UTC,
            messenger,
            checkpoints: test_checkpoints(&dir_path),
            tracing_service: test_tracing_service(),
            tracing_client_context: test_tracing_client_context(),
        },
    );
    (runtime, registry, results, bus_handle)
}

/// Skip `events` to the next time the session at `address` reports going
/// `idle`, so each call matches one turn's idle.
async fn next_idle(
    events: &mut residuum_bus::Subscriber<residuum_bus::SessionEvent>,
    address: &SessionAddress,
    what: &str,
) {
    loop {
        let event = next_event(what, events).await;
        if &event.address == address
            && matches!(
                event.kind,
                SessionEventKind::StateChanged(SessionState::Idle)
            )
        {
            return;
        }
    }
}

fn conversation_request(address: &str) -> SessionSpawnRequest {
    SessionSpawnRequest {
        address: SessionAddress::from(address),
        source_label: "discord:#builds".to_string(),
        trigger: residuum_bus::EventTrigger::Conversation,
        agent_skill: None,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        subagent_config: SubAgentConfig {
            prompt: "can you check the build?".to_string(),
            context: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            hop_count: 0,
            sender: None,
            inbound: None,
            images: Vec::new(),
            carried_history: Vec::new(),
        },
        conversation_target: Some(residuum_bus::ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "chan-1".to_string(),
        }),
        overlap: None,
    }
}

fn resources_with(provider: Box<dyn InferenceProvider>) -> SubAgentResources {
    let (layout, observer, merge_writer) = test_memory_extras();
    SubAgentResources {
        max_tool_iterations: None,
        repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
        provider,
        tools: ToolRegistry::new(),
        mcp_registry: McpRegistry::new_shared(),
        skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
        identity: IdentityFiles::default(),
        options: CompletionOptions::default(),
        skills_index: None,
        observations: None,
        recent_context: None,
        layout,
        observer,
        merge_writer,
        episode_skip_token_floor: 2000,
        hop_counter: crate::agent::HopCounter::new(0),
    }
}

#[tokio::test]
async fn a_conversation_session_s_send_message_to_its_chat_posts() {
    let log = EventLog::default();
    let _capture = log.capture();
    let (runtime, registry, _results, bus_handle) = held_idle_runtime().await;
    let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
    let mut chat: residuum_bus::Subscriber<residuum_bus::ResponseEvent> = bus_handle
        .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
            "discord",
        )))
        .await
        .unwrap();
    let mut final_text: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> = bus_handle
        .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
            "discord",
        )))
        .await
        .unwrap();
    let endpoints = residuum_bus::EndpointRegistry::from_entries([residuum_bus::EndpointEntry {
        id: residuum_bus::EndpointId::from("discord"),
        topic: residuum_bus::TopicId::Endpoint(residuum_bus::EndpointName::from("discord")),
        capabilities: residuum_bus::EndpointCapabilities::INTERACTIVE,
        display_name: "Discord".to_string(),
    }]);
    let _source = endpoints
        .conversations()
        .register("discord", Arc::new(BuildsChannel));
    let mut resources = resources_with(Box::new(SendsToChatProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
    }));
    resources
        .tools
        .register(Box::new(crate::tools::send_message::SendMessageTool::new(
            endpoints,
            bus_handle.publisher(),
            true,
        )));
    let address = SessionAddress::from("external-discord-sends");

    runtime.spawn(conversation_request(address.as_ref()), Some(resources));

    let posted = next_event("the session's send_message to its chat", &mut chat).await;
    assert_eq!(posted.conversation.as_deref(), Some("chan-1"));
    assert_eq!(posted.content, "Ivan says the build is green");
    next_idle(&mut session_events, &address, "the turn to end").await;
    bus_barrier(&bus_handle).await;
    assert!(
        final_text.drain().is_empty(),
        "the turn's final text stays in the session"
    );
    assert!(
        !log.events()
            .iter()
            .any(|e| e.text.contains("without a send_message to its chat")),
        "a turn that sent its chat a message is not reported as silent"
    );
    assert!(registry.stop(&address));
}

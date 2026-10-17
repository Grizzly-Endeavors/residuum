//! Test fixtures for code that builds session resources. Behind
//! `test-support`, and compiled for this crate's own tests.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use residuum_bus::session::{MAIN_DEPTH, SessionState};
use residuum_bus::testing::next_event;
use residuum_bus::{
    AgentResultEvent, ConversationTarget, EventTrigger, SessionAddress, SessionEventKind, topics,
};
use residuum_config::WorkspaceLayout;
use residuum_inference::{
    CompletionOptions, InferenceError, InferenceResponse, Message, ToolDefinition,
};
use residuum_mcp::McpRegistry;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_memory::reflector::Reflector;
use residuum_memory::search::MemoryIndex;
use residuum_skills::{SkillIndex, SkillState};
use residuum_tracing_service::{ClientContext, TracingService};
use residuum_workspace::identity::IdentityFiles;

use crate::messaging::AgentMessenger;
use crate::registry::SessionRegistry;
use crate::runtime::{IdleTimeouts, SessionRuntime, SessionRuntimeHandles, SessionSpawnRequest};
use crate::store::SessionStore;
use crate::subagent::SubAgentResources;
use crate::types::SubAgentConfig;

/// A throwaway checkpoint engine for tests that need a `SessionRuntime`
/// but don't exercise checkpointing behavior themselves.
///
/// # Panics
///
/// Panics when the checkpoint engine cannot be created in `dir`.
#[must_use]
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
)]
pub fn test_checkpoints(dir: &std::path::Path) -> Arc<residuum_checkpoints::CheckpointEngine> {
    Arc::new(
        residuum_checkpoints::CheckpointEngine::new(
            "test-agent",
            dir.join("workspace"),
            &residuum_config::paths::TeamPaths::new(dir.join("team")),
            dir.join("agent-config"),
            dir.join("hub"),
            &dir.join("checkpoints"),
            None,
        )
        .unwrap(),
    )
}

/// A `TracingService` with auto error reporting off, for tests that need
/// a `SessionRuntime` but don't exercise auto-reporting behavior
/// themselves — `on_error` calls through it are a guaranteed no-op.
#[must_use]
pub fn test_tracing_service() -> Arc<TracingService> {
    let (_, handle) = residuum_core::util::telemetry::SpanBufferLayer::new(
        &residuum_core::util::telemetry::SpanBufferConfig::default(),
    );
    Arc::new(TracingService::new(
        residuum_config::TracingConfig::default(),
        handle,
    ))
}

#[must_use]
pub fn test_tracing_client_context() -> Arc<ClientContext> {
    Arc::new(ClientContext {
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

/// Provider that returns canned responses in sequence, one per call —
/// unlike `MockProvider`, which always returns the same response — so a
/// test can distinguish a run's first turn from its second.
pub struct SequencedProvider {
    responses: Vec<String>,
    call_count: std::sync::atomic::AtomicUsize,
}

impl SequencedProvider {
    pub fn new(responses: Vec<&str>) -> Self {
        Self {
            responses: responses.into_iter().map(str::to_string).collect(),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl residuum_inference::InferenceProvider for SequencedProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        let idx = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let response = self
            .responses
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "no more responses configured".to_string());
        Ok(InferenceResponse::new(response, vec![]))
    }

    fn model_name(&self) -> &'static str {
        "sequenced"
    }
}

#[must_use]
pub fn make_sequenced_resources(responses: Vec<&str>) -> SubAgentResources {
    let (layout, observer, merge_writer) = test_memory_extras();
    SubAgentResources {
        max_tool_iterations: None,
        repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
        provider: Box::new(SequencedProvider::new(responses)),
        tools: residuum_agent::ToolRegistry::new(),
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
        hop_counter: residuum_agent::HopCounter::new(0),
    }
}

/// Build a runtime whose idle window outlasts any test, rather than
/// `test_runtime`'s 20ms: a session stays idle until the test delivers to
/// it or stops it, so the test never races the idle timeout completing
/// the run out from under it.
///
/// # Panics
///
/// Panics when the broker, the temporary directory or the session subscription
/// cannot be set up.
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
)]
pub async fn test_runtime_held_idle() -> (
    SessionRuntime,
    residuum_bus::Subscriber<AgentResultEvent>,
    residuum_bus::BusHandle,
) {
    let idle_window = Duration::from_hours(1);
    let bus_handle = residuum_bus::spawn_broker();
    let sub = bus_handle.subscribe(topics::Background).await.unwrap();
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
        residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
    ));
    let runtime = SessionRuntime::new(
        registry,
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
    (runtime, sub, bus_handle)
}

/// Skip `events` to the next time the session at `address` reports
/// going `idle`, so each call matches one turn's idle and a test can tell
/// a second turn's idle from the first one's.
pub async fn next_idle(
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

/// A conversation-triggered spawn request at a deterministic-style
/// address, wired with a conversation target so its turn output routes
/// back to it.
#[must_use]
pub fn sample_conversation_request(address: &str) -> SessionSpawnRequest {
    SessionSpawnRequest {
        address: SessionAddress::from(address),
        source_label: "discord:#builds".to_string(),
        trigger: EventTrigger::Conversation,
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
        conversation_target: Some(ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "chan-1".to_string(),
        }),
        overlap: None,
    }
}

/// Calls `send_message` to discord's `chan-1` on its first completion,
/// then answers with text.
pub struct SendsToChatProvider {
    pub calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl residuum_inference::InferenceProvider for SendsToChatProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Ok(InferenceResponse::new(
                String::new(),
                vec![residuum_inference::ToolCall {
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

pub struct BuildsChannel;

#[async_trait]
impl residuum_bus::conversations::ConversationSource for BuildsChannel {
    async fn conversations(
        &self,
    ) -> anyhow::Result<Vec<residuum_bus::conversations::KnownConversation>> {
        Ok(vec![residuum_bus::conversations::KnownConversation {
            id: "chan-1".to_string(),
            kind: residuum_bus::origin::ConversationKind::Channel,
            label: "#builds".to_string(),
        }])
    }
}

/// A layout, observer, and merge writer for a session's `SubAgentResources`,
/// backed by a temporary directory the layout keeps alive. `Observer::disabled`
/// and a disabled reflector mean neither ever fires unless a test configures
/// otherwise.
///
/// # Panics
///
/// Panics when the temporary directory or its search index cannot be created.
#[must_use]
#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
)]
pub fn test_memory_extras() -> (WorkspaceLayout, Arc<Observer>, Arc<MemoryMergeWriter>) {
    let dir = tempfile::tempdir().unwrap();
    let layout = WorkspaceLayout::new(dir.path()).with_tempdir_guard(dir);
    let search_index = Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
    let reflector = Reflector::disabled(chrono_tz::UTC);
    let merge_writer = Arc::new(MemoryMergeWriter::new(
        reflector,
        layout.clone(),
        search_index,
        None,
        None,
    ));
    let observer = Arc::new(Observer::disabled(chrono_tz::UTC));
    (layout, observer, merge_writer)
}

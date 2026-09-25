//! In-crate end-to-end tests for the A2A server: a real [`SessionRuntime`],
//! a scripted `InferenceProvider`, the real [`super::listener::A2aListener`]
//! bound to a loopback port, and the official `a2a-client-lf` client driving
//! it over HTTP. See `docs/systems-usage/a2a.md`.
//!
//! Declared as `#[cfg(test)] mod server_e2e_tests;` from `mod.rs`, so this
//! whole file (harness and tests alike) only exists in test builds.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a2a::{A2AError, CancelTaskRequest, GetTaskRequest, ListTasksRequest, Part, Role, TaskState};
use a2a_client::{A2AClientFactory, agent_card::AgentCardResolver, auth::AuthInterceptor};
use a2a_server::DefaultRequestHandler;
use a2a_server::TaskStore as _;
use async_trait::async_trait;
use futures_util::StreamExt as _;
use tokio::sync::Mutex as AsyncMutex;

use crate::a2a::auth::NoTunnel;
use crate::a2a::card::{CardRuntime, CardState, SharedCardState};
use crate::a2a::executor::SessionExecutor;
use crate::a2a::handler::{ResiduumA2aHandler, resume_in_progress_tasks};
use crate::a2a::keys_runtime::{A2aKeys, SharedA2aKeys};
use crate::a2a::listener::A2aListener;
use crate::a2a::task_store::{CALLER_METADATA_KEY, FileTaskStore, SharedTaskStore};
use crate::actions::store::ActionStore;
use crate::agent::HopCounter;
use crate::background::HopLimits;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::background::runtime::{SessionRuntime, SessionSpawnRequest};
use crate::background::store::SessionStore;
use crate::background::subagent::{SubAgentResources, test_memory_extras};
use crate::background::types::SubAgentConfig;
use crate::bus::{
    BusHandle, EndpointRegistry, Publisher, SessionAddress, SpawnRequestEvent, topics,
};
use crate::config::{A2aConfig, A2aVisibility, BackgroundModelTier, Config};
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message, ToolCall,
    ToolDefinition,
};
use crate::mcp::McpRegistry;
use crate::skills::{SkillIndex, SkillState};
use crate::tools::{SubagentToolDeps, ToolRegistry};
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

/// Shared queue of scripted model responses, consumed in order by
/// [`ScriptedProvider`] across every session run a harness spawns.
type SharedResponseQueue = Arc<AsyncMutex<VecDeque<InferenceResponse>>>;
/// Every prompt [`ScriptedProvider`] was called with, one entry per
/// `complete()` call, for assertions on what a session actually saw.
type SharedSeenMessages = Arc<AsyncMutex<Vec<Vec<Message>>>>;

/// A provider driven entirely by a shared, pre-loaded response queue —
/// shared across every session run a test harness spawns (initial run and
/// any resume), so a test can script a whole conversation's worth of model
/// turns up front. Also records every prompt it was called with, so a test
/// can assert on what the session actually saw (skill notes, attachment
/// lines, resume pointers, inline images).
struct ScriptedProvider {
    queue: SharedResponseQueue,
    seen: SharedSeenMessages,
    /// Artificial delay before returning, so a test can reliably observe a
    /// task in `WORKING` (e.g. to cancel it, or to register a fake spawned
    /// child) before the scripted turn resolves.
    delay: Duration,
}

#[async_trait]
impl InferenceProvider for ScriptedProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.seen.lock().await.push(messages.to_vec());
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.queue
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| InferenceError::Api("scripted provider exhausted".to_string()))
    }

    fn model_name(&self) -> &'static str {
        "mock"
    }
}

/// A minimal but fully populated `Config`, only used where an API demands a
/// whole `Config` (`gather_for_bug_report`) — nothing here is exercised by
/// the a2a path itself.
fn test_config(dir: &std::path::Path) -> Config {
    use crate::config::{
        AgentAbilitiesConfig, BackgroundConfig, GatewayConfig, IdleConfig, LearningConfig,
        MemoryConfig, SkillsConfig, SubconsciousSettings, ToolsConfig, TracingConfig,
        WebSearchConfig,
    };
    use crate::inference::retry::RetryConfig;

    Config {
        name: None,
        main: vec![],
        observer: vec![],
        reflector: vec![],
        pulse: vec![],
        subconscious: vec![],
        embedding: None,
        workspace_dir: dir.to_path_buf(),
        timeout_secs: 30,
        max_tokens: 4096,
        memory: MemoryConfig::default(),
        pulse_enabled: false,
        subconscious_settings: SubconsciousSettings::default(),
        learning: LearningConfig::default(),
        gateway: GatewayConfig::default(),
        timezone: chrono_tz::UTC,
        cloud: None,
        discord: None,
        telegram: None,
        teams: None,
        a2a: A2aConfig::default(),
        webhooks: HashMap::new(),
        skills: SkillsConfig { dirs: vec![] },
        tools: ToolsConfig { dirs: vec![] },
        retry: RetryConfig::default(),
        background: BackgroundConfig::default(),
        agent: AgentAbilitiesConfig::default(),
        idle: IdleConfig::default(),
        temperature: None,
        thinking: None,
        web_search: WebSearchConfig {
            provider_native: None,
            standalone_backend: None,
        },
        tracing: TracingConfig::default(),
        role_overrides: HashMap::new(),
        config_dir: dir.to_path_buf(),
        load_notices: vec![],
    }
}

/// A live A2A server stack: a real `SessionRuntime` driven by a scripted
/// provider, the real session executor/task store/handler, and the real
/// `A2aListener` bound to a loopback port — everything Stream E built,
/// wired together the same way `build_a2a_listener` wires it in production.
struct Harness {
    base_url: String,
    keys: SharedA2aKeys,
    task_store: SharedTaskStore,
    session_registry: Arc<SessionRegistry>,
    messenger: Arc<AgentMessenger>,
    bus_handle: BusHandle,
    skill_state: crate::skills::SharedSkillState,
    card_state: SharedCardState,
    queue: SharedResponseQueue,
    seen: SharedSeenMessages,
    workspace_dir: PathBuf,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    // Kept alive for the harness's lifetime.
    _tempdir: tempfile::TempDir,
}

/// Options for building a [`Harness`], so each test only sets what it needs.
struct HarnessOptions {
    card_skills: Vec<crate::a2a::card::AgentCardSkillFile>,
    workspace_skill: Option<(&'static str, &'static str)>,
    idle_timeout: Duration,
    /// Artificial delay every scripted model turn takes before resolving.
    response_delay: Duration,
}

impl Default for HarnessOptions {
    fn default() -> Self {
        Self {
            card_skills: vec![],
            workspace_skill: None,
            idle_timeout: Duration::from_millis(50),
            response_delay: Duration::ZERO,
        }
    }
}

async fn free_port() -> u16 {
    tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Set up the tempdir-backed workspace a harness runs against: required
/// directories, an optional workspace skill (for the skill-mapping test),
/// and the workspace agent-card file.
async fn setup_workspace(
    opts: &HarnessOptions,
) -> (
    tempfile::TempDir,
    PathBuf,
    WorkspaceLayout,
    crate::skills::SharedSkillState,
) {
    let tempdir = tempfile::tempdir().unwrap();
    let workspace_dir = tempdir.path().to_path_buf();
    let layout = WorkspaceLayout::new(&workspace_dir);
    for dir in layout.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.unwrap();
    }
    tokio::fs::create_dir_all(layout.a2a_tasks_dir())
        .await
        .unwrap();

    let mut skill_dirs = vec![layout.skills_dir()];
    if let Some((name, description)) = opts.workspace_skill {
        let skill_dir = layout.skills_dir().join(name);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
        )
        .await
        .unwrap();
    }
    let skill_index = SkillIndex::scan(&skill_dirs).await.unwrap();
    skill_dirs.clear();
    let skill_state = SkillState::new_shared(skill_index, vec![layout.skills_dir()]);

    let card_file = crate::a2a::card::AgentCardFile {
        name: "Test Agent".to_string(),
        description: "a test a2a agent".to_string(),
        skills: opts.card_skills.clone(),
        default_input_modes: None,
        default_output_modes: None,
    };
    tokio::fs::write(
        layout.agent_card_json(),
        serde_json::to_string(&card_file).unwrap(),
    )
    .await
    .unwrap();

    (tempdir, workspace_dir, layout, skill_state)
}

/// The shared, mostly-inert dependencies `SubagentToolDeps` requires but
/// none of these tests actually exercise (search, feedback tools).
fn build_support_deps(
    workspace_dir: &std::path::Path,
) -> (
    Arc<crate::memory::search::HybridSearcher>,
    Arc<crate::tracing_service::TracingService>,
    Arc<crate::tracing_service::ClientContext>,
) {
    let cfg = test_config(workspace_dir);
    let search_index = Arc::new(crate::memory::search::MemoryIndex::empty().unwrap());
    let hybrid_searcher = Arc::new(crate::memory::search::HybridSearcher::new(
        Arc::clone(&search_index),
        None,
        None,
        crate::config::SearchConfig::default(),
    ));
    let (_, span_buffer) = crate::util::telemetry::SpanBufferLayer::new(
        &crate::util::telemetry::SpanBufferConfig::default(),
    );
    let tracing_service = Arc::new(crate::tracing_service::TracingService::new(
        cfg.tracing.clone(),
        span_buffer,
    ));
    let tracing_client_context =
        Arc::new(crate::tracing_service::client_context::gather_for_bug_report(&cfg));
    (hybrid_searcher, tracing_service, tracing_client_context)
}

/// Everything needed to spawn the mini background listener, bundled to keep
/// [`spawn_harness`] under the line-count lint.
struct RuntimeDeps {
    session_registry: Arc<SessionRegistry>,
    messenger: Arc<AgentMessenger>,
    bus_handle: BusHandle,
    skill_state: crate::skills::SharedSkillState,
    workspace_dir: PathBuf,
    layout: WorkspaceLayout,
    a2a_hub: Arc<crate::a2a::A2aClientHub>,
    a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
    checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
}

/// Build the scripted-provider queue/call-log and spawn the mini background
/// listener that drives every session run in `runtime` from them.
fn start_scripted_sessions(
    runtime: Arc<SessionRuntime>,
    deps: RuntimeDeps,
    response_delay: Duration,
) -> (SharedResponseQueue, SharedSeenMessages) {
    let queue: SharedResponseQueue = Arc::new(AsyncMutex::new(VecDeque::new()));
    let seen: SharedSeenMessages = Arc::new(AsyncMutex::new(Vec::new()));
    let (hybrid_searcher, tracing_service, tracing_client_context) =
        build_support_deps(&deps.workspace_dir);

    let mini_deps = MiniListenerDeps {
        runtime,
        session_registry: deps.session_registry,
        messenger: deps.messenger,
        skill_state: deps.skill_state,
        publisher: deps.bus_handle.publisher(),
        queue: Arc::clone(&queue),
        seen: Arc::clone(&seen),
        workspace_dir: deps.workspace_dir.clone(),
        layout: deps.layout.clone(),
        action_store: Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
            deps.layout.scheduled_actions_json(),
        ))),
        action_notify: Arc::new(tokio::sync::Notify::new()),
        endpoint_registry: EndpointRegistry::from_entries(std::iter::empty()),
        tools_path: Arc::new(tokio::sync::RwLock::new(None)),
        path_policy: crate::tools::PathPolicy::new_shared(),
        agent_keys: crate::agent_keys::AgentKeys::new_shared(&deps.workspace_dir),
        hybrid_searcher,
        tracing_service,
        tracing_client_context,
        a2a_hub: deps.a2a_hub,
        a2a_tracker: deps.a2a_tracker,
        checkpoints: deps.checkpoints,
        response_delay,
    };
    spawn_mini_background_listener(deps.bus_handle, mini_deps);
    (queue, seen)
}

/// Build the real executor/handler and start the real `A2aListener` on
/// `port`, returning its caller-key store and shutdown sender.
async fn start_a2a_listener(
    workspace_dir: &std::path::Path,
    card_state: &SharedCardState,
    task_store: &SharedTaskStore,
    executor: SessionExecutor,
    port: u16,
) -> (SharedA2aKeys, tokio::sync::watch::Sender<bool>) {
    let inner = DefaultRequestHandler::new(
        executor,
        crate::a2a::task_store::DelegatingTaskStore(Arc::clone(task_store)),
    )
    .with_capabilities(a2a::AgentCapabilities {
        streaming: Some(true),
        push_notifications: Some(false),
        extensions: None,
        extended_agent_card: None,
    });
    let handler = Arc::new(ResiduumA2aHandler::new(inner, Arc::clone(task_store)));
    let keys = A2aKeys::new_shared(workspace_dir);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = A2aListener::new(
        A2aConfig {
            enabled: true,
            port,
            public_url: None,
            visibility: A2aVisibility::Public,
        },
        "127.0.0.1".to_string(),
        handler,
        Arc::clone(card_state),
        Arc::clone(&keys),
        Arc::new(NoTunnel),
        shutdown_rx,
    );
    tokio::spawn(listener.start());
    tokio::time::sleep(Duration::from_millis(50)).await;
    (keys, shutdown_tx)
}

async fn spawn_harness(opts: HarnessOptions) -> Harness {
    let (tempdir, workspace_dir, layout, skill_state) = setup_workspace(&opts).await;

    let port = free_port().await;
    let card_runtime = CardRuntime::from_config(
        &A2aConfig {
            enabled: true,
            port,
            public_url: None,
            visibility: A2aVisibility::Public,
        },
        "127.0.0.1",
    );
    let card_state = CardState::load(&layout.agent_card_json(), &card_runtime).unwrap();

    let bus_handle = crate::bus::spawn_broker();
    let session_registry = Arc::new(SessionRegistry::new());
    let session_store = Arc::new(SessionStore::new(layout.sessions_dir()));
    let messenger = Arc::new(AgentMessenger::new(
        Arc::clone(&session_registry),
        bus_handle.publisher(),
        Arc::clone(&session_store),
        HopLimits { soft: 8, hard: 32 },
    ));
    let background_config = crate::config::BackgroundConfig {
        idle_timeout_external: opts.idle_timeout,
        ..crate::config::BackgroundConfig::default()
    };
    let checkpoints = Arc::new(
        crate::checkpoints::CheckpointEngine::new(
            layout.root().to_path_buf(),
            workspace_dir.clone(),
            &workspace_dir.join("checkpoints"),
            None,
        )
        .unwrap(),
    );
    let runtime = Arc::new(SessionRuntime::new(
        Arc::clone(&session_registry),
        Arc::clone(&session_store),
        8,
        &background_config,
        crate::background::runtime::SessionRuntimeHandles {
            publisher: bus_handle.publisher(),
            tz: chrono_tz::UTC,
            messenger: Arc::clone(&messenger),
            checkpoints: Arc::clone(&checkpoints),
        },
    ));

    let a2a_hub = crate::a2a::A2aClientHub::new_shared();
    let a2a_tracker = crate::a2a::RemoteTaskTracker::load(
        layout.a2a_outbound_json(),
        Arc::clone(&a2a_hub),
        Arc::clone(&messenger),
        layout.agent_inbox_dir(),
    )
    .await;

    let (queue, seen) = start_scripted_sessions(
        Arc::clone(&runtime),
        RuntimeDeps {
            session_registry: Arc::clone(&session_registry),
            messenger: Arc::clone(&messenger),
            bus_handle: bus_handle.clone(),
            skill_state: Arc::clone(&skill_state),
            workspace_dir: workspace_dir.clone(),
            layout: layout.clone(),
            a2a_hub,
            a2a_tracker,
            checkpoints: Arc::clone(&checkpoints),
        },
        opts.response_delay,
    );

    let task_store = FileTaskStore::load(&layout.a2a_tasks_dir()).await.unwrap();
    let executor = SessionExecutor::new(
        Arc::clone(&messenger),
        Arc::clone(&session_registry),
        bus_handle.clone(),
        Arc::clone(&skill_state),
        Arc::clone(&card_state),
        layout.agent_inbox_dir(),
        chrono_tz::UTC,
    );
    let (keys, shutdown_tx) =
        start_a2a_listener(&workspace_dir, &card_state, &task_store, executor, port).await;

    Harness {
        base_url: format!("http://127.0.0.1:{port}"),
        keys,
        task_store,
        session_registry,
        messenger,
        bus_handle,
        skill_state,
        card_state,
        queue,
        seen,
        workspace_dir,
        shutdown_tx,
        _tempdir: tempdir,
    }
}

/// Everything [`spawn_mini_background_listener`] needs to turn a
/// [`SpawnRequestEvent`] into a real session run, standing in for the
/// production `background::listener::spawn_listener` +
/// `spawn_context::build_spawn_resources` pipeline: this test drives
/// `SessionRuntime::spawn` directly with a scripted provider instead of a
/// real LLM, exactly as `background/runtime.rs`'s own tests do.
struct MiniListenerDeps {
    runtime: Arc<SessionRuntime>,
    session_registry: Arc<SessionRegistry>,
    messenger: Arc<AgentMessenger>,
    skill_state: crate::skills::SharedSkillState,
    publisher: Publisher,
    queue: SharedResponseQueue,
    seen: SharedSeenMessages,
    workspace_dir: PathBuf,
    layout: WorkspaceLayout,
    action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: Arc<tokio::sync::Notify>,
    endpoint_registry: EndpointRegistry,
    tools_path: crate::tools::SharedToolsPath,
    path_policy: crate::tools::SharedPathPolicy,
    agent_keys: crate::agent_keys::SharedAgentKeys,
    hybrid_searcher: Arc<crate::memory::search::HybridSearcher>,
    tracing_service: Arc<crate::tracing_service::TracingService>,
    tracing_client_context: Arc<crate::tracing_service::ClientContext>,
    a2a_hub: Arc<crate::a2a::A2aClientHub>,
    a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
    checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    response_delay: Duration,
}

fn spawn_mini_background_listener(bus_handle: BusHandle, deps: MiniListenerDeps) {
    tokio::spawn(async move {
        let mut sub: crate::bus::Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        loop {
            let Ok(Some(event)) = sub.recv().await else {
                return;
            };
            let resources = build_test_resources(&deps, &event);
            let request = SessionSpawnRequest {
                address: event.address,
                source_label: event.source_label,
                trigger: event.source,
                agent_skill: event.skill,
                spawner: event.spawner,
                depth: event.depth,
                subagent_config: SubAgentConfig {
                    prompt: event.prompt,
                    context: event.context,
                    model_tier: event.model_tier,
                    hop_count: event.hop_count,
                    sender: event.sender,
                    inbound: event.inbound,
                    images: event.images,
                },
                conversation_target: event.conversation,
            };
            deps.runtime.spawn(request, Some(resources));
        }
    });
}

/// Build `SubAgentResources` for one session run: a real `ToolRegistry`
/// (gated exactly as production gates it, so `a2a_task_update` only appears
/// for an `a2a` conversation target) and a [`ScriptedProvider`] sharing this
/// harness's response queue and call log.
fn build_test_resources(deps: &MiniListenerDeps, event: &SpawnRequestEvent) -> SubAgentResources {
    let (layout, observer, merge_writer) = test_memory_extras();
    let hop_counter = HopCounter::new(event.hop_count);
    let tools = ToolRegistry::build_subagent_registry(SubagentToolDeps {
        tracker: crate::tools::FileTracker::new_shared(),
        path_policy: Arc::clone(&deps.path_policy),
        tools_path: Arc::clone(&deps.tools_path),
        agent_keys: Arc::clone(&deps.agent_keys),
        skill_state: Arc::clone(&deps.skill_state),
        tz: chrono_tz::UTC,
        hybrid_searcher: Arc::clone(&deps.hybrid_searcher),
        workspace_dir: deps.workspace_dir.clone(),
        episodes_dir: deps.layout.episodes_dir(),
        sessions_dir: deps.layout.sessions_dir(),
        agent_inbox_dir: deps.layout.agent_inbox_dir(),
        agent_inbox_archive_dir: deps.layout.agent_inbox_archive_dir(),
        user_inbox_dir: deps.layout.user_inbox_dir(),
        user_inbox_attachments_dir: deps.layout.user_inbox_attachments_dir(),
        session_registry: Arc::clone(&deps.session_registry),
        endpoint_registry: deps.endpoint_registry.clone(),
        publisher: deps.publisher.clone(),
        action_store: Arc::clone(&deps.action_store),
        action_notify: Arc::clone(&deps.action_notify),
        own_address: event.address.clone(),
        own_depth: event.depth,
        depth_cap: 2,
        session_category: crate::background::registry::SessionCategory::from_trigger(&event.source)
            .as_str()
            .to_string(),
        trigger: event.source.clone(),
        conversation_target: event.conversation.clone(),
        messenger: Arc::clone(&deps.messenger),
        hop_counter: hop_counter.clone(),
        tracing_service: Arc::clone(&deps.tracing_service),
        tracing_client_context: Arc::clone(&deps.tracing_client_context),
        web_search_backend: None,
        a2a_hub: Arc::clone(&deps.a2a_hub),
        a2a_tracker: Arc::clone(&deps.a2a_tracker),
        checkpoints: Arc::clone(&deps.checkpoints),
    });

    SubAgentResources {
        max_tool_iterations: None,
        repeat_call_guard: crate::config::RepeatCallGuardConfig::default(),
        provider: Box::new(ScriptedProvider {
            queue: Arc::clone(&deps.queue),
            seen: Arc::clone(&deps.seen),
            delay: deps.response_delay,
        }),
        tools,
        mcp_registry: McpRegistry::new_shared(),
        skill_state: Arc::clone(&deps.skill_state),
        identity: IdentityFiles::default(),
        options: CompletionOptions::default(),
        skills_index: None,
        observations: None,
        recent_context: None,
        layout,
        observer,
        merge_writer,
        episode_skip_token_floor: 2000,
        hop_counter,
    }
}

/// The longest this suite ever waits on one operation. Every await below is
/// bounded by this (directly or via [`drain_until_terminal`]) — a hang here
/// is a test failure, not a slow pass.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Resolve the harness's card and build an authenticated client for `token`.
async fn client_for(
    harness: &Harness,
    token: &str,
) -> a2a_client::A2AClient<Box<dyn a2a_client::Transport>> {
    let card = tokio::time::timeout(
        TEST_TIMEOUT,
        AgentCardResolver::new(None).resolve(&harness.base_url),
    )
    .await
    .expect("timed out resolving the agent card")
    .unwrap();
    A2AClientFactory::builder()
        .with_interceptor(Arc::new(AuthInterceptor::bearer(token)))
        .build()
        .create_from_card(&card)
        .await
        .unwrap()
}

fn send_request(
    text: &str,
    task_id: Option<&str>,
    context_id: Option<&str>,
) -> a2a::SendMessageRequest {
    let mut message = a2a::Message::new(Role::User, vec![Part::text(text)]);
    message.task_id = task_id.map(str::to_string);
    message.context_id = context_id.map(str::to_string);
    a2a::SendMessageRequest {
        message,
        configuration: None,
        metadata: None,
        tenant: None,
    }
}

/// An `a2a_task_update` tool call, as the model would produce it.
fn task_update_call(id: &str, state: &str, message: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: "a2a_task_update".to_string(),
        arguments: serde_json::json!({ "state": state, "message": message }),
    }
}

fn task_update_call_with_artifacts(
    id: &str,
    state: &str,
    message: &str,
    artifacts: &[&str],
) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: "a2a_task_update".to_string(),
        arguments: serde_json::json!({ "state": state, "message": message, "artifacts": artifacts }),
    }
}

/// Drain a streaming response until it ends, returning every event seen.
async fn drain_until_terminal(
    mut stream: futures_util::stream::BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
) -> Vec<a2a::StreamResponse> {
    let mut events = Vec::new();
    loop {
        let next = tokio::time::timeout(TEST_TIMEOUT, stream.next())
            .await
            .expect("timed out waiting for the next streamed event");
        match next {
            Some(item) => events.push(item.unwrap()),
            None => return events,
        }
    }
}

fn task_id_of(events: &[a2a::StreamResponse]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            a2a::StreamResponse::Task(t) => Some(t.id.clone()),
            a2a::StreamResponse::StatusUpdate(u) => Some(u.task_id.clone()),
            a2a::StreamResponse::ArtifactUpdate(u) => Some(u.task_id.clone()),
            a2a::StreamResponse::Message(_) => None,
        })
        .expect("a stream must carry a task id somewhere")
}

fn context_id_of(events: &[a2a::StreamResponse]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            a2a::StreamResponse::Task(t) => Some(t.context_id.clone()),
            a2a::StreamResponse::StatusUpdate(u) => Some(u.context_id.clone()),
            a2a::StreamResponse::ArtifactUpdate(u) => Some(u.context_id.clone()),
            a2a::StreamResponse::Message(_) => None,
        })
        .expect("a stream must carry a context id somewhere")
}

/// Poll `task_store` for `task_id` until it reaches `state`, or time out.
async fn wait_for_task_state(
    task_store: &SharedTaskStore,
    task_id: &str,
    state: TaskState,
) -> a2a::Task {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(task) = task_store.get(task_id).await.unwrap()
                && task.status.state == state
            {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for task {task_id} to reach {state:?}"))
}

#[tokio::test]
async fn send_streams_working_then_completes_with_an_artifact() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let token = harness.keys.create("alice", None).await.unwrap();
    tokio::fs::write(harness.workspace_dir.join("report.md"), "the report body")
        .await
        .unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call_with_artifacts(
                "call-1",
                "completed",
                "all done",
                &["report.md"],
            )],
        ));
        queue.push_back(InferenceResponse::new("turn wrap-up".to_string(), vec![]));
    }

    let client = client_for(&harness, &token).await;
    let stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("please write a report", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let events = drain_until_terminal(stream).await;

    let saw_working = events.iter().any(|e| {
        matches!(e, a2a::StreamResponse::StatusUpdate(u) if u.status.state == TaskState::Working)
    });
    assert!(
        saw_working,
        "must see a WORKING status before completion: {events:?}"
    );
    let artifact = events.iter().find_map(|e| match e {
        a2a::StreamResponse::ArtifactUpdate(u) => Some(&u.artifact),
        a2a::StreamResponse::Task(_)
        | a2a::StreamResponse::Message(_)
        | a2a::StreamResponse::StatusUpdate(_) => None,
    });
    let artifact = artifact.expect("must stream an artifact update");
    assert_eq!(artifact.name.as_deref(), Some("report.md"));
    assert_eq!(
        artifact.parts.first().unwrap().as_text(),
        Some("the report body")
    );

    let task_id = task_id_of(&events);
    let task = tokio::time::timeout(
        TEST_TIMEOUT,
        client.get_task(&GetTaskRequest {
            id: task_id,
            history_length: None,
            tenant: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(task.artifacts.map(|a| a.len()), Some(1));
    let message_text = task
        .status
        .message
        .and_then(|m| m.text().map(str::to_string));
    assert_eq!(message_text.as_deref(), Some("all done"));

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn input_required_then_followup_resumes_across_an_idle_session_and_completes() {
    let harness = spawn_harness(HarnessOptions {
        idle_timeout: Duration::from_millis(30),
        ..Default::default()
    })
    .await;
    let token = harness.keys.create("bob", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call(
                "call-1",
                "input_required",
                "which format?",
            )],
        ));
        queue.push_back(InferenceResponse::new(
            "waiting on the caller".to_string(),
            vec![],
        ));
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call("call-2", "completed", "here you go")],
        ));
        queue.push_back(InferenceResponse::new("turn wrap-up".to_string(), vec![]));
    }

    let client = client_for(&harness, &token).await;
    let stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("write something", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let events = drain_until_terminal(stream).await;
    let task_id = task_id_of(&events);
    let saw_input_required = events.iter().any(|e| {
        matches!(e, a2a::StreamResponse::StatusUpdate(u) if u.status.state == TaskState::InputRequired)
    });
    assert!(saw_input_required, "must reach INPUT_REQUIRED: {events:?}");

    // Let the session's own run idle out and complete, recording a resume
    // point — the same mechanism a follow-up after a long silence relies on.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let followup = send_request("markdown please", Some(&task_id), None);
    let response = tokio::time::timeout(TEST_TIMEOUT, client.send_message(&followup))
        .await
        .unwrap()
        .unwrap();
    let task = match response {
        a2a::SendMessageResponse::Task(t) => t,
        a2a::SendMessageResponse::Message(_) => panic!("expected a task response"),
    };
    assert_eq!(task.status.state, TaskState::Completed);

    let seen = harness.seen.lock().await;
    let saw_resume_pointer = seen
        .iter()
        .flatten()
        .any(|m| m.content.contains("Resumed session"));
    assert!(
        saw_resume_pointer,
        "the resumed run's prompt must carry the episode pointer note"
    );

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn cancel_marks_the_task_canceled() {
    let harness = spawn_harness(HarnessOptions {
        response_delay: Duration::from_millis(500),
        ..Default::default()
    })
    .await;
    let token = harness.keys.create("cara", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            "eventually done".to_string(),
            vec![],
        ));
    }

    let client = client_for(&harness, &token).await;
    let mut stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("take your time", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let first = tokio::time::timeout(TEST_TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let task_id = task_id_of(std::slice::from_ref(&first));

    let canceled = tokio::time::timeout(
        TEST_TIMEOUT,
        client.cancel_task(&CancelTaskRequest {
            id: task_id.clone(),
            metadata: None,
            tenant: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(canceled.status.state, TaskState::Canceled);

    let task = tokio::time::timeout(
        TEST_TIMEOUT,
        client.get_task(&GetTaskRequest {
            id: task_id,
            history_length: None,
            tenant: None,
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(task.status.state, TaskState::Canceled);

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn ownership_isolation_prevents_cross_caller_access() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let token_a = harness.keys.create("dana", None).await.unwrap();
    let token_b = harness.keys.create("eli", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new("plain answer".to_string(), vec![]));
    }

    let client_a = client_for(&harness, &token_a).await;
    let response = tokio::time::timeout(
        TEST_TIMEOUT,
        client_a.send_message(&send_request("hi", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let task_id = match response {
        a2a::SendMessageResponse::Task(t) => t.id,
        a2a::SendMessageResponse::Message(_) => panic!("expected a task response"),
    };

    let client_b = client_for(&harness, &token_b).await;
    let get_err = client_b
        .get_task(&GetTaskRequest {
            id: task_id.clone(),
            history_length: None,
            tenant: None,
        })
        .await
        .unwrap_err();
    assert_eq!(get_err.code, a2a::error_code::TASK_NOT_FOUND);

    let cancel_err = client_b
        .cancel_task(&CancelTaskRequest {
            id: task_id.clone(),
            metadata: None,
            tenant: None,
        })
        .await
        .unwrap_err();
    assert_eq!(cancel_err.code, a2a::error_code::TASK_NOT_FOUND);

    let list = client_b
        .list_tasks(&ListTasksRequest {
            context_id: None,
            status: None,
            page_size: None,
            page_token: None,
            history_length: None,
            status_timestamp_after: None,
            include_artifacts: None,
            tenant: None,
        })
        .await
        .unwrap();
    assert!(
        list.tasks.is_empty(),
        "caller B must not see caller A's tasks"
    );

    let task = client_a
        .get_task(&GetTaskRequest {
            id: task_id,
            history_length: None,
            tenant: None,
        })
        .await
        .unwrap();
    assert_eq!(task.status.state, TaskState::Completed);

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn one_open_task_per_context_is_rejected() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let token = harness.keys.create("finn", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call("call-1", "input_required", "need more")],
        ));
        queue.push_back(InferenceResponse::new("waiting".to_string(), vec![]));
    }

    let client = client_for(&harness, &token).await;
    let stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("start", None, Some("ctx-fixed"))),
    )
    .await
    .unwrap()
    .unwrap();
    drain_until_terminal(stream).await;

    let second = send_request("a second, unrelated task", None, Some("ctx-fixed"));
    let err = tokio::time::timeout(TEST_TIMEOUT, client.send_message(&second))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, a2a::error_code::INVALID_REQUEST);

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn restart_continuation_resumes_a_task_left_in_progress() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let caller = "key:gwen".to_string();
    let context_id = "ctx-restart".to_string();
    let mut metadata = HashMap::new();
    metadata.insert(
        CALLER_METADATA_KEY.to_string(),
        serde_json::Value::String(caller.clone()),
    );
    let task = a2a::Task {
        id: a2a::new_task_id(),
        context_id: context_id.clone(),
        status: a2a::TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Some(chrono::Utc::now()),
        },
        artifacts: None,
        history: Some(vec![a2a::Message::new(
            Role::User,
            vec![Part::text("original request")],
        )]),
        metadata: Some(metadata),
    };
    harness.task_store.create(task.clone()).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call("call-1", "completed", "resumed and done")],
        ));
        queue.push_back(InferenceResponse::new("turn wrap-up".to_string(), vec![]));
    }

    // Rebuild the executor and handler from the same on-disk task store,
    // simulating a fresh process picking up where the last one left off.
    let executor = SessionExecutor::new(
        Arc::clone(&harness.messenger),
        Arc::clone(&harness.session_registry),
        harness.bus_handle.clone(),
        Arc::clone(&harness.skill_state),
        Arc::clone(&harness.card_state),
        WorkspaceLayout::new(&harness.workspace_dir).agent_inbox_dir(),
        chrono_tz::UTC,
    );
    let inner = DefaultRequestHandler::new(
        executor,
        crate::a2a::task_store::DelegatingTaskStore(Arc::clone(&harness.task_store)),
    )
    .with_capabilities(a2a::AgentCapabilities {
        streaming: Some(true),
        push_notifications: Some(false),
        extensions: None,
        extended_agent_card: None,
    });
    let rebuilt_handler = Arc::new(ResiduumA2aHandler::new(
        inner,
        Arc::clone(&harness.task_store),
    ));

    tokio::time::timeout(
        TEST_TIMEOUT,
        resume_in_progress_tasks(rebuilt_handler, Arc::clone(&harness.task_store)),
    )
    .await
    .expect("resume_in_progress_tasks must not hang");

    let completed = wait_for_task_state(&harness.task_store, &task.id, TaskState::Completed).await;
    let text = completed
        .status
        .message
        .and_then(|m| m.text().map(str::to_string));
    assert_eq!(text.as_deref(), Some("resumed and done"));

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn run_completes_with_last_final_text_when_no_signal_is_sent() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let token = harness.keys.create("hana", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            "here's the answer, no tool call needed".to_string(),
            vec![],
        ));
    }

    let client = client_for(&harness, &token).await;
    let response = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_message(&send_request("question", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let task = match response {
        a2a::SendMessageResponse::Task(t) => t,
        a2a::SendMessageResponse::Message(_) => panic!("expected a task response"),
    };
    assert_eq!(task.status.state, TaskState::Completed);
    let text = task
        .status
        .message
        .and_then(|m| m.text().map(str::to_string));
    assert_eq!(
        text.as_deref(),
        Some("here's the answer, no tool call needed")
    );

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn run_stays_working_while_a_live_spawned_child_exists() {
    let harness = spawn_harness(HarnessOptions {
        response_delay: Duration::from_millis(200),
        ..Default::default()
    })
    .await;
    let token = harness.keys.create("ivy", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        // First run: completes with no explicit signal, but a live spawned
        // child is registered before it does.
        queue.push_back(InferenceResponse::new("done for now".to_string(), vec![]));
        // Second run (resumed once the child's relay wakes the parent):
        // completes for real.
        queue.push_back(InferenceResponse::new("actually done".to_string(), vec![]));
    }

    let client = client_for(&harness, &token).await;
    let stream = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("go do the thing", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let mut stream = stream;
    let first = tokio::time::timeout(TEST_TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let task_id = task_id_of(std::slice::from_ref(&first));
    let context_id = context_id_of(std::slice::from_ref(&first));
    let address = SessionExecutor::address_for("key:ivy", &context_id);

    // Register a fake live child while the parent's turn is still resolving
    // (the 200ms response delay gives us the window).
    let child_address = SessionAddress::from("spawned-fake-child-0001");
    let child_info = crate::background::registry::SessionInfo {
        address: child_address.clone(),
        run_id: "run-fake-child".to_string(),
        category: crate::background::registry::SessionCategory::Spawned,
        trigger: crate::bus::EventTrigger::Agent,
        source_label: "agent:fake-child".to_string(),
        state: crate::background::registry::SessionState::Running,
        spawner: Some(address.clone()),
        depth: 2,
        purpose: "pretending to work".to_string(),
        agent_skill: None,
        model_tier: BackgroundModelTier::Medium,
        conversation_target: None,
        started_at: chrono::Utc::now(),
        usage: crate::agent::usage::SessionUsageTotals::default(),
    };
    harness
        .session_registry
        .register(child_info, tokio_util::sync::CancellationToken::new())
        .unwrap();

    // The session's own turn still reports its final text as a `WORKING`
    // status update (per docs/systems-usage/a2a.md) even though the run
    // beneath it is about to complete — `stream_until_terminal`'s
    // "keep streaming" branch only refuses to turn *that* completion into a
    // terminal A2A status while a live spawned child exists. Collect
    // whatever arrives for a bounded window and confirm none of it is
    // terminal.
    let mut events = vec![first];
    // Stops when the stream ends or nothing arrives within the window —
    // either way, expected: stop collecting.
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_millis(500), stream.next()).await
    {
        events.push(item.unwrap());
    }
    let saw_terminal = events.iter().any(|e| {
        matches!(
            e,
            a2a::StreamResponse::StatusUpdate(u) if u.status.state.is_terminal()
        )
    });
    assert!(
        !saw_terminal,
        "the task must not complete while a live spawned child exists: {events:?}"
    );

    let still_working = harness.task_store.get(&task_id).await.unwrap().unwrap();
    assert_eq!(still_working.status.state, TaskState::Working);

    // Now let the child finish and relay to its spawner (the parent), which
    // resumes the parent session at the same address.
    harness
        .session_registry
        .remove(&child_address, "run-fake-child");
    harness
        .messenger
        .send(
            address.as_ref(),
            child_address,
            "spawned".to_string(),
            "child finished".to_string(),
            0,
        )
        .await
        .unwrap();

    let completed = wait_for_task_state(&harness.task_store, &task_id, TaskState::Completed).await;
    let text = completed
        .status
        .message
        .and_then(|m| m.text().map(str::to_string));
    assert_eq!(text.as_deref(), Some("actually done"));

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn skill_metadata_maps_to_a_matching_workspace_skill() {
    let harness = spawn_harness(HarnessOptions {
        card_skills: vec![crate::a2a::card::AgentCardSkillFile {
            id: "reporter".to_string(),
            name: "Reporter".to_string(),
            description: "writes reports".to_string(),
            tags: vec![],
            examples: None,
        }],
        workspace_skill: Some(("reporter", "Writes reports.")),
        ..Default::default()
    })
    .await;
    let token = harness.keys.create("jax", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new("ok".to_string(), vec![]));
    }

    let mut request = send_request("go", None, Some("ctx-skill"));
    let mut metadata = HashMap::new();
    metadata.insert(
        "skill".to_string(),
        serde_json::Value::String("reporter".to_string()),
    );
    request.message.metadata = Some(metadata);

    let client = client_for(&harness, &token).await;
    tokio::time::timeout(TEST_TIMEOUT, client.send_message(&request))
        .await
        .unwrap()
        .unwrap();

    let address = SessionExecutor::address_for("key:jax", "ctx-skill");
    let point = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(point) = harness.session_registry.resume_point(&address) {
                return point;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for the session to record a resume point");
    assert_eq!(
        point.agent_skill.as_ref().map(AsRef::as_ref),
        Some("reporter"),
        "the session must have run with the skill named by message metadata"
    );

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn raw_image_part_reaches_the_model_as_an_inline_image() {
    let harness = spawn_harness(HarnessOptions::default()).await;
    let token = harness.keys.create("kim", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new("saw it".to_string(), vec![]));
    }

    let mut request = send_request("look at this", None, None);
    request.message.parts.push(
        Part::raw(vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A])
            .with_media_type("image/png")
            .with_filename("pic.png"),
    );

    let client = client_for(&harness, &token).await;
    tokio::time::timeout(TEST_TIMEOUT, client.send_message(&request))
        .await
        .unwrap()
        .unwrap();

    let seen = harness.seen.lock().await;
    let saw_image = seen.iter().flatten().any(|m| !m.images.is_empty());
    assert!(
        saw_image,
        "the raw image part must reach the model as an inline image"
    );

    harness.shutdown_tx.send(true).ok();
}

#[tokio::test]
async fn closing_text_of_the_signaling_turn_never_reaches_the_next_execution() {
    let harness = spawn_harness(HarnessOptions {
        response_delay: Duration::from_millis(400),
        ..Default::default()
    })
    .await;
    let token = harness.keys.create("dana", None).await.unwrap();
    {
        let mut queue = harness.queue.lock().await;
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call(
                "call-1",
                "input_required",
                "which season?",
            )],
        ));
        // The signaling turn's own wrap-up, still being produced when the
        // caller's follow-up arrives.
        queue.push_back(InferenceResponse::new(
            "stale narration about asking them".to_string(),
            vec![],
        ));
        queue.push_back(InferenceResponse::new(
            String::new(),
            vec![task_update_call("call-2", "completed", "autumn haiku")],
        ));
        queue.push_back(InferenceResponse::new("wrap-up".to_string(), vec![]));
    }

    let client = client_for(&harness, &token).await;
    let first = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("write a haiku", None, None)),
    )
    .await
    .unwrap()
    .unwrap();
    let first_events = drain_until_terminal(first).await;
    let task_id = task_id_of(&first_events);

    // Follow up at once, while the first turn's closing text is still pending.
    let second = tokio::time::timeout(
        TEST_TIMEOUT,
        client.send_streaming_message(&send_request("autumn", Some(&task_id), None)),
    )
    .await
    .unwrap()
    .unwrap();
    let second_events = drain_until_terminal(second).await;

    let leaked = second_events.iter().any(|e| {
        matches!(e, a2a::StreamResponse::StatusUpdate(u)
            if u.status.message.as_ref().and_then(a2a::Message::text).is_some_and(|t| t.contains("stale narration")))
    });
    assert!(
        !leaked,
        "the previous turn's closing text must not be relayed to the follow-up: {second_events:?}"
    );
    let completed = second_events.iter().any(|e| {
        matches!(e, a2a::StreamResponse::StatusUpdate(u) if u.status.state == TaskState::Completed)
    });
    assert!(
        completed,
        "the follow-up must still complete: {second_events:?}"
    );
}

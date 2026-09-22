//! Gateway initialization: builds all subsystems before the event loop starts.

mod memory;
mod providers;
mod tools;

pub use providers::init_providers;

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::SessionRuntime;
use crate::background::conversation_router::ConversationRouter;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::background::store::SessionStore;
use crate::bus::EndpointRegistry;
use crate::config::Config;
use crate::inference::SharedHttpClient;
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::memory::search::HybridSearcher;
use crate::notify::channels::InboxChannel;
use crate::skills::{SharedSkillState, SkillIndex, SkillState};
use crate::tools::SharedToolsPath;
use crate::util::FatalError;
use crate::workspace::bootstrap::ensure_workspace;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use crate::background::spawn_context::SpawnContext;

use tools::{CreateAgentArgs, ToolRegistryDeps};

/// All subsystems initialized before the gateway event loop.
pub(crate) struct GatewayComponents {
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    pub observer: Observer,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub subconscious: Arc<crate::subconscious::Subconscious>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub tools_path: SharedToolsPath,
    pub skill_state: SharedSkillState,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub pulse_enabled: bool,
    pub endpoint_registry: EndpointRegistry,
    pub channel_configs: Vec<crate::notify::types::ExternalChannelConfig>,
    pub http_client: SharedHttpClient,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    pub session_store: Arc<SessionStore>,
    pub agent_messenger: Arc<AgentMessenger>,
    pub conversation_router: Arc<ConversationRouter>,
    pub spawn_context: Arc<SpawnContext>,
    pub path_policy: crate::tools::SharedPathPolicy,
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
    /// Shared tracing service. Owned here so feedback tools can register
    /// against it during agent construction; downstream consumers (web
    /// API, sub-agents) clone the Arc.
    pub tracing_service: Arc<crate::tracing_service::TracingService>,
    /// Snapshot of the runtime client context for bug-report submissions.
    pub tracing_client_context: Arc<crate::tracing_service::ClientContext>,
}

/// Bootstrap the workspace directory and return the layout and timezone.
///
/// # Errors
/// Returns `FatalError` if workspace bootstrapping fails.
pub(super) async fn init_workspace(
    cfg: &Config,
) -> Result<(WorkspaceLayout, chrono_tz::Tz), FatalError> {
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    let tz = cfg.timezone;
    ensure_workspace(&layout, cfg.name.as_deref(), Some(cfg.timezone.name())).await?;

    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        FatalError::Config(format!(
            "failed to change to workspace directory {}: {e}",
            cfg.workspace_dir.display()
        ))
    })?;
    tracing::info!(workspace = %cfg.workspace_dir.display(), "changed to workspace directory");

    Ok((layout, tz))
}

/// Load identity files and build the shared HTTP client.
///
/// # Errors
/// Returns `FatalError` if identity loading or HTTP client construction fails.
pub(super) async fn init_identity_and_http(
    layout: &WorkspaceLayout,
    cfg: &Config,
) -> Result<(IdentityFiles, SharedHttpClient), FatalError> {
    let identity = IdentityFiles::load(layout).await?;
    identity.warn_missing(layout);
    let http = SharedHttpClient::new(&crate::inference::HttpClientConfig::with_timeout(
        cfg.timeout_secs,
    ))
    .map_err(|e| FatalError::Config(format!("failed to build HTTP client: {e}")))?;
    Ok((identity, http))
}

/// Build a fresh session observer from `[observer]` config, for
/// `SpawnContext` at startup and on every config reload.
///
/// # Errors
/// Returns `FatalError::Config` if the observer provider cannot be built.
pub(crate) fn init_session_observer(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<Observer, FatalError> {
    memory::build_observer(cfg, tz, http)
}

/// Load the scheduled action store and create the notification handle.
async fn init_action_store(
    layout: &WorkspaceLayout,
) -> (
    Arc<tokio::sync::Mutex<ActionStore>>,
    Arc<tokio::sync::Notify>,
) {
    let actions_path = layout.scheduled_actions_json();
    let action_store = match ActionStore::load(&actions_path).await {
        Ok(store) => Arc::new(tokio::sync::Mutex::new(store)),
        Err(err) => {
            tracing::warn!(error = %err, "action store degraded: starting empty");
            Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
                actions_path,
            )))
        }
    };
    let action_notify = Arc::new(tokio::sync::Notify::new());
    (action_store, action_notify)
}

/// Scan for skills and return the shared state handle.
async fn init_skills(cfg: &Config) -> SharedSkillState {
    let skill_index = match SkillIndex::scan(&cfg.skills.dirs).await {
        Ok(idx) => idx,
        Err(err) => {
            tracing::warn!(error = %err, "skill index degraded: starting empty");
            SkillIndex::default()
        }
    };
    SkillState::new_shared(skill_index, cfg.skills.dirs.clone())
}

/// Build a session's own observer and the shared memory merge writer.
///
/// The session observer is independent from the main agent's own
/// (`providers.observer`) so a session fork never contends with a main
/// config-reload swap, but is built from the same `[observer]` config so
/// extraction behaves identically — rebuilt fresh in `SpawnContext` on every
/// config reload, matching the rest of it. The merge writer is shared with
/// the main agent so episode numbering and log appends never race.
///
/// # Errors
/// Returns `FatalError::Config` if the session observer's provider cannot be built.
fn build_session_memory_components(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
    layout: &WorkspaceLayout,
    reflector: crate::memory::reflector::Reflector,
    mem: &memory::MemoryComponents,
    embedding_provider: Option<Arc<dyn crate::inference::EmbeddingProvider>>,
) -> Result<(Arc<Observer>, Arc<MemoryMergeWriter>), FatalError> {
    let session_observer = Arc::new(memory::build_observer(cfg, tz, http)?);
    let merge_writer = Arc::new(MemoryMergeWriter::new(
        reflector,
        layout.clone(),
        Arc::clone(&mem.search_index),
        mem.vector_store.clone(),
        embedding_provider,
    ));
    Ok((session_observer, merge_writer))
}

/// Inputs to [`build_startup_spawn_context`], gathered because
/// `SpawnContext` itself has this many independent dependencies (mirrors
/// `reload::build_spawn_context`'s equivalent construction from a live
/// `GatewayRuntime`, which doesn't exist yet at startup).
struct StartupSpawnContextInputs<'a> {
    cfg: &'a Config,
    layout: &'a WorkspaceLayout,
    tz: chrono_tz::Tz,
    http_client: SharedHttpClient,
    session_runtime: &'a Arc<SessionRuntime>,
    session_registry: &'a Arc<SessionRegistry>,
    endpoint_registry: &'a EndpointRegistry,
    publisher: &'a crate::bus::Publisher,
    action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: &'a Arc<tokio::sync::Notify>,
    hybrid_searcher: &'a Arc<HybridSearcher>,
    skill_state: &'a SharedSkillState,
    mcp_registry: &'a SharedMcpRegistry,
    session_observer: &'a Arc<Observer>,
    merge_writer: &'a Arc<MemoryMergeWriter>,
    /// Built alongside the session registry in `init_session_runtime`, since
    /// the runtime itself now needs it too (to resume a run whose interrupt
    /// channel still held messages at teardown) — reused here rather than
    /// built a second time.
    messenger: &'a Arc<AgentMessenger>,
    /// Shared tracing service, built alongside the main agent's feedback
    /// tools — reused here so a session's own feedback tools register
    /// against the same instance.
    tracing_service: &'a Arc<crate::tracing_service::TracingService>,
    /// Runtime client context snapshot, built alongside `tracing_service`.
    tracing_client_context: &'a Arc<crate::tracing_service::ClientContext>,
    /// Standalone web search backend config, mirroring `cfg.web_search.standalone_backend`.
    web_search_backend: Option<crate::config::StandaloneBackendConfig>,
}

/// Build the `SpawnContext` every session forks from, at startup.
fn build_startup_spawn_context(inputs: StartupSpawnContextInputs<'_>) -> Arc<SpawnContext> {
    let messenger = Arc::clone(inputs.messenger);
    Arc::new(SpawnContext {
        background_config: inputs.cfg.background.clone(),
        main_provider_specs: inputs.cfg.main.clone(),
        http_client: inputs.http_client,
        max_tokens: inputs.cfg.max_tokens,
        retry_config: inputs.cfg.retry.clone(),
        options: crate::inference::CompletionOptions {
            max_tokens: Some(inputs.cfg.max_tokens),
            temperature: inputs.cfg.temperature,
            thinking: inputs.cfg.thinking.clone(),
            ..crate::inference::CompletionOptions::default()
        },
        layout: inputs.layout.clone(),
        tz: inputs.tz,
        role_overrides: inputs.cfg.role_overrides.clone(),
        session_runtime: Arc::clone(inputs.session_runtime),
        session_registry: Arc::clone(inputs.session_registry),
        endpoint_registry: inputs.endpoint_registry.clone(),
        publisher: inputs.publisher.clone(),
        action_store: Arc::clone(inputs.action_store),
        action_notify: Arc::clone(inputs.action_notify),
        hybrid_searcher: Arc::clone(inputs.hybrid_searcher),
        skill_state: Arc::clone(inputs.skill_state),
        mcp_registry: Arc::clone(inputs.mcp_registry),
        observer: Arc::clone(inputs.session_observer),
        merge_writer: Arc::clone(inputs.merge_writer),
        messenger,
        tracing_service: Arc::clone(inputs.tracing_service),
        tracing_client_context: Arc::clone(inputs.tracing_client_context),
        web_search_backend: inputs.web_search_backend,
    })
}

/// Create the session registry, store, messenger, and runtime.
///
/// The messenger is built here (rather than alongside the rest of
/// `SpawnContext`) because the runtime itself now depends on it too — to
/// resume a run whose interrupt channel still held messages at teardown —
/// so it has to exist before `SessionRuntime::new` is called.
///
/// At startup, any run left in the store from a prior process exit goes
/// through the full completion pipeline (skip check, final observation,
/// merge) from its persisted transcript before normal operation begins.
async fn init_session_runtime(
    cfg: &Config,
    layout: &WorkspaceLayout,
    publisher: &crate::bus::Publisher,
    session_observer: &Observer,
    merge_writer: &MemoryMergeWriter,
) -> (
    Arc<SessionRegistry>,
    Arc<SessionStore>,
    Arc<AgentMessenger>,
    Arc<SessionRuntime>,
    Arc<ConversationRouter>,
) {
    let registry = Arc::new(SessionRegistry::new());
    let store = Arc::new(SessionStore::new(layout.sessions_dir()));

    let recovery_env = crate::background::session_memory::SessionMemoryEnv {
        observer: session_observer,
        merge_writer,
        layout,
        episode_skip_token_floor: cfg.background.episode_skip_token_floor,
        tz: cfg.timezone,
    };
    let recovered = store.recover_incomplete_runs(&recovery_env).await;
    if recovered > 0 {
        tracing::warn!(
            recovered,
            "recovered session runs left incomplete by a prior process exit"
        );
    }

    let messenger = Arc::new(AgentMessenger::new(
        Arc::clone(&registry),
        publisher.clone(),
        Arc::clone(&store),
        crate::background::HopLimits::from(&cfg.background),
    ));

    let runtime = Arc::new(SessionRuntime::new(
        Arc::clone(&registry),
        Arc::clone(&store),
        cfg.background.max_concurrent,
        &cfg.background,
        publisher.clone(),
        cfg.timezone,
        Arc::clone(&messenger),
    ));
    let conversation_router = Arc::new(ConversationRouter::new(Arc::clone(&messenger)));
    (registry, store, messenger, runtime, conversation_router)
}

/// Load and connect workspace MCP servers.
async fn init_mcp_servers(
    layout: &WorkspaceLayout,
    tools_path: SharedToolsPath,
) -> SharedMcpRegistry {
    let mcp_registry = crate::mcp::McpRegistry::new_shared_with_tools_path(tools_path);
    match crate::workspace::config::load_mcp_servers(&layout.mcp_json()) {
        Ok(servers) => {
            if !servers.is_empty() {
                let report = mcp_registry
                    .write()
                    .await
                    .reconcile_and_connect(&servers)
                    .await;
                tracing::info!(
                    started = report.started,
                    stopped = report.stopped,
                    failures = report.failures.len(),
                    "workspace MCP servers loaded"
                );
                for (server_name, err) in &report.failures {
                    tracing::warn!(server = %server_name, error = %err, "mcp server failed to start");
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "workspace MCP servers degraded");
        }
    }
    mcp_registry
}

/// The MCP server entry for a standalone web search backend, or `None` for
/// no backend or `"ollama"` (a native tool, not an MCP server — see
/// `ToolRegistry::register_ollama_web_search_tool`).
fn web_search_mcp_entry(
    backend: &crate::config::StandaloneBackendConfig,
) -> Option<crate::mcp::types::McpServerEntry> {
    match backend.name.as_str() {
        "brave" => Some(crate::mcp::types::McpServerEntry {
            name: "brave_web_search".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@anthropic-ai/mcp-server-brave-search".to_string(),
            ],
            env: std::collections::HashMap::from([(
                "BRAVE_API_KEY".to_string(),
                backend.api_key.clone(),
            )]),
            transport: crate::mcp::types::McpTransport::Stdio,
            headers: std::collections::HashMap::new(),
        }),
        "tavily" => Some(crate::mcp::types::McpServerEntry {
            name: "tavily_web_search".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "tavily-mcp".to_string()],
            env: std::collections::HashMap::from([(
                "TAVILY_API_KEY".to_string(),
                backend.api_key.clone(),
            )]),
            transport: crate::mcp::types::McpTransport::Stdio,
            headers: std::collections::HashMap::new(),
        }),
        _ => None,
    }
}

/// Connect the standalone web search MCP server (Brave/Tavily) if
/// configured, returning the connection report (empty when no backend is
/// configured, or it names `"ollama"`).
///
/// Used at startup (`init_networking`) and, on a config reload that changes
/// the standalone backend, by `gateway::reload::reload_web_search` — which
/// disconnects whichever server was previously running first, since
/// [`McpRegistry::connect_servers`](crate::mcp::McpRegistry::connect_servers)
/// skips a name that's already tracked, even if its entry (e.g. the API key)
/// changed.
pub(crate) async fn connect_web_search_mcp(
    cfg: &Config,
    mcp_registry: &SharedMcpRegistry,
) -> crate::mcp::McpReconcileReport {
    let Some(backend) = &cfg.web_search.standalone_backend else {
        return crate::mcp::McpReconcileReport::default();
    };
    let Some(entry) = web_search_mcp_entry(backend) else {
        return crate::mcp::McpReconcileReport::default();
    };

    let report = mcp_registry.write().await.connect_servers(&[entry]).await;
    tracing::info!(
        backend = %backend.name,
        started = report.started,
        failures = report.failures.len(),
        "web search MCP server loaded"
    );
    for (name, err) in &report.failures {
        tracing::warn!(server = %name, error = %err, "failed to start web search MCP server");
    }
    report
}

/// Load channel configs and build the endpoint registry.
fn init_channels_and_registry(
    layout: &WorkspaceLayout,
    cfg: &Config,
) -> (
    Vec<crate::notify::types::ExternalChannelConfig>,
    EndpointRegistry,
) {
    let channel_configs =
        match crate::workspace::config::load_channel_configs(&layout.channels_toml()) {
            Ok(configs) => configs,
            Err(err) => {
                tracing::warn!(error = %err, "workspace channels degraded");
                Vec::new()
            }
        };
    let endpoint_registry = EndpointRegistry::from_config(cfg, &channel_configs);
    (channel_configs, endpoint_registry)
}

/// The shared PATH, MCP registry, and endpoint/channel setup a fresh
/// gateway (or a config reload) needs before it can build tools.
struct NetworkingComponents {
    tools_path: SharedToolsPath,
    mcp_registry: SharedMcpRegistry,
    channel_configs: Vec<crate::notify::types::ExternalChannelConfig>,
    endpoint_registry: EndpointRegistry,
}

/// Build the shared tools `PATH`, connect workspace MCP servers, and load
/// the channel/endpoint registry.
async fn init_networking(cfg: &Config, layout: &WorkspaceLayout) -> NetworkingComponents {
    let tools_path: SharedToolsPath =
        Arc::new(tokio::sync::RwLock::new(cfg.tools.effective_path()));
    let mcp_registry = init_mcp_servers(layout, Arc::clone(&tools_path)).await;
    connect_web_search_mcp(cfg, &mcp_registry).await;
    let (channel_configs, endpoint_registry) = init_channels_and_registry(layout, cfg);
    NetworkingComponents {
        tools_path,
        mcp_registry,
        channel_configs,
        endpoint_registry,
    }
}

/// Spawn notify subscribers for each configured channel and the inbox.
///
/// Each channel subscribes to its `TopicId::Notification(name)` topic on the bus.
/// The inbox subscribes to `TopicId::Inbox`.
pub(crate) async fn spawn_notify_subscribers(
    bus_handle: &crate::bus::BusHandle,
    channel_configs: &[crate::notify::types::ExternalChannelConfig],
    http: &SharedHttpClient,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) -> Vec<tokio::task::JoinHandle<()>> {
    use crate::bus::{NotifyName, topics};
    use crate::notify::subscriber::run_notify_subscriber;

    let external_channels =
        crate::workspace::config::build_external_channels(channel_configs, http.client()).await;

    let mut handles = Vec::new();

    // Spawn a subscriber for each external channel
    for (name, channel) in external_channels {
        let topic = topics::Notification(NotifyName::from(name.as_str()));
        match bus_handle.subscribe(topic).await {
            Ok(subscriber) => {
                let handle = tokio::spawn(run_notify_subscriber(subscriber, channel));
                handles.push(handle);
                tracing::info!(channel = %name, "notify subscriber spawned");
            }
            Err(e) => {
                tracing::warn!(channel = %name, error = %e, "failed to subscribe notify channel");
            }
        }
    }

    // Spawn inbox subscriber
    let inbox_channel = InboxChannel::new(layout.agent_inbox_dir(), tz);
    match bus_handle.subscribe(topics::Inbox).await {
        Ok(subscriber) => {
            let handle = tokio::spawn(run_notify_subscriber(subscriber, Box::new(inbox_channel)));
            handles.push(handle);
            tracing::info!("inbox notify subscriber spawned");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe inbox channel");
        }
    }

    handles
}

/// Construct the shared `TracingService` and snapshot the runtime
/// client context used by the bug-report tools and HTTP handlers.
///
/// Falls back to a fresh in-memory span buffer if no global one was
/// installed (e.g. during tests where the tracing layer wasn't wired).
fn init_tracing_service(
    cfg: &Config,
) -> (
    Arc<crate::tracing_service::TracingService>,
    Arc<crate::tracing_service::ClientContext>,
) {
    let buffer = crate::util::telemetry::global_span_buffer()
        .cloned()
        .unwrap_or_else(|| {
            let (_, handle) = crate::util::telemetry::SpanBufferLayer::new(
                &crate::util::telemetry::SpanBufferConfig::default(),
            );
            handle
        });
    let service = Arc::new(crate::tracing_service::TracingService::new(
        cfg.tracing.clone(),
        buffer,
    ));
    let client_context =
        Arc::new(crate::tracing_service::client_context::gather_for_bug_report(cfg));
    (service, client_context)
}

/// Inputs to [`build_tools_and_agent`], gathered because building the tool
/// registry and the agent that owns it needs this many independent pieces.
struct ToolsAndAgentInputs<'a> {
    cfg: &'a Config,
    layout: &'a WorkspaceLayout,
    mem: &'a memory::MemoryComponents,
    tz: chrono_tz::Tz,
    tool_deps: ToolRegistryDeps<'a>,
    mcp_registry: &'a SharedMcpRegistry,
    provider: Box<dyn crate::inference::InferenceProvider>,
    options: crate::inference::CompletionOptions,
    identity: IdentityFiles,
    /// Main's current-turn hop counter — the same instance already threaded
    /// into `tool_deps` for the `message_agent`/`subagent_spawn` tools.
    hop_counter: crate::agent::HopCounter,
}

/// Build the tool registry, reserve its names against MCP name collisions,
/// and construct the agent from it.
///
/// Split out of `initialize` (which has this many independent subsystems to
/// wire up already) so the tool/agent construction sequence reads as one
/// step there. Takes `ToolRegistryDeps` by value (built by the caller) so it
/// doesn't also need every one of its dependencies as a separate parameter.
async fn build_tools_and_agent(
    inputs: ToolsAndAgentInputs<'_>,
) -> (
    Agent,
    crate::tools::SharedPathPolicy,
    tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
) {
    let (tools, path_policy_for_runtime, output_topic_override_tx) = tools::init_tool_registry(
        inputs.cfg,
        inputs.layout,
        inputs.mem,
        inputs.tz,
        &inputs.tool_deps,
    );

    // Reserve the built-in tool namespace so any MCP tool (workspace or web
    // search) that reuses a built-in name is shadowed visibly instead of
    // silently. See src/mcp/CLAUDE.md.
    inputs
        .mcp_registry
        .write()
        .await
        .set_reserved_tool_names(tools.tool_names());

    let agent = tools::create_agent(
        CreateAgentArgs {
            provider: inputs.provider,
            options: inputs.options,
            tools,
            identity: inputs.identity,
            hop_counter: inputs.hop_counter,
        },
        inputs.mcp_registry,
        inputs.tz,
        inputs.layout,
    )
    .await;

    (agent, path_policy_for_runtime, output_topic_override_tx)
}

/// Inputs to [`build_main_agent`], gathered because building the main agent
/// needs this many independent pieces.
struct MainAgentInputs<'a> {
    cfg: &'a Config,
    layout: &'a WorkspaceLayout,
    mem: &'a memory::MemoryComponents,
    tz: chrono_tz::Tz,
    identity: IdentityFiles,
    provider: Box<dyn crate::inference::InferenceProvider>,
    options: crate::inference::CompletionOptions,
    net: &'a NetworkingComponents,
    action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: &'a Arc<tokio::sync::Notify>,
    skill_state: &'a SharedSkillState,
    session_registry: &'a Arc<SessionRegistry>,
    publisher: &'a crate::bus::Publisher,
    tracing_service: &'a Arc<crate::tracing_service::TracingService>,
    tracing_client_context: &'a Arc<crate::tracing_service::ClientContext>,
    agent_messenger: &'a Arc<AgentMessenger>,
}

/// Create main's hop counter and build the agent from it, wrapping
/// [`build_tools_and_agent`]'s `ToolsAndAgentInputs`/`ToolRegistryDeps`
/// assembly. Split out of `initialize` purely to keep that function's line
/// count down.
async fn build_main_agent(
    inputs: MainAgentInputs<'_>,
) -> (
    Agent,
    crate::tools::SharedPathPolicy,
    tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
) {
    // Main's current-turn hop counter, created once and shared between the
    // `Agent` and the `message_agent`/`subagent_spawn` tools registered
    // against it, so they always agree on the current turn's hop count.
    let hop_counter = crate::agent::HopCounter::new(0);

    build_tools_and_agent(ToolsAndAgentInputs {
        cfg: inputs.cfg,
        layout: inputs.layout,
        mem: inputs.mem,
        tz: inputs.tz,
        tool_deps: ToolRegistryDeps {
            action_store: inputs.action_store,
            action_notify: inputs.action_notify,
            skill_state: inputs.skill_state,
            tools_path: &inputs.net.tools_path,
            session_registry: inputs.session_registry,
            endpoint_registry: &inputs.net.endpoint_registry,
            publisher: inputs.publisher,
            tracing_service: inputs.tracing_service,
            tracing_client_context: inputs.tracing_client_context,
            agent_messenger: inputs.agent_messenger,
            hop_counter: &hop_counter,
        },
        mcp_registry: &inputs.net.mcp_registry,
        provider: inputs.provider,
        options: inputs.options,
        identity: inputs.identity,
        hop_counter: hop_counter.clone(),
    })
    .await
}

/// Initialize all gateway subsystems from config.
///
/// Delegates to `init_workspace`, `init_identity_and_http`, `providers::init_providers`,
/// and `memory::init_memory` for the first stages, then wires up tools, the agent,
/// and remaining subsystems.
///
/// # Errors
/// Returns `FatalError` if any subsystem fails to initialize.
pub(crate) async fn initialize(
    cfg: &Config,
    publisher: &crate::bus::Publisher,
) -> Result<GatewayComponents, FatalError> {
    let (layout, tz) = init_workspace(cfg).await?;

    let (identity, http) = init_identity_and_http(&layout, cfg).await?;
    let providers = providers::init_providers(cfg, tz, http.clone())?;
    let mem = memory::init_memory(cfg, &layout, providers.embedding_provider.as_ref()).await?;
    let subconscious = crate::subconscious::Subconscious::build(cfg, &layout, http.clone());

    let (action_store, action_notify) = init_action_store(&layout).await;
    let skill_state = init_skills(cfg).await;

    let (session_observer, merge_writer) = build_session_memory_components(
        cfg,
        tz,
        http.clone(),
        &layout,
        providers.reflector,
        &mem,
        providers.embedding_provider.clone(),
    )?;
    let (session_registry, session_store, agent_messenger, session_runtime, conversation_router) =
        init_session_runtime(cfg, &layout, publisher, &session_observer, &merge_writer).await;
    let net = init_networking(cfg, &layout).await;
    let (tracing_service, tracing_client_context) = init_tracing_service(cfg);

    let spawn_context = build_startup_spawn_context(StartupSpawnContextInputs {
        cfg,
        layout: &layout,
        tz,
        http_client: http.clone(),
        session_runtime: &session_runtime,
        session_registry: &session_registry,
        endpoint_registry: &net.endpoint_registry,
        publisher,
        action_store: &action_store,
        action_notify: &action_notify,
        hybrid_searcher: &mem.hybrid_searcher,
        skill_state: &skill_state,
        mcp_registry: &net.mcp_registry,
        session_observer: &session_observer,
        merge_writer: &merge_writer,
        messenger: &agent_messenger,
        tracing_service: &tracing_service,
        tracing_client_context: &tracing_client_context,
        web_search_backend: cfg.web_search.standalone_backend.clone(),
    });

    let (agent, path_policy_for_runtime, output_topic_override_tx) =
        build_main_agent(MainAgentInputs {
            cfg,
            layout: &layout,
            mem: &mem,
            tz,
            identity,
            provider: providers.provider,
            options: providers.options,
            net: &net,
            action_store: &action_store,
            action_notify: &action_notify,
            skill_state: &skill_state,
            session_registry: &session_registry,
            publisher,
            tracing_service: &tracing_service,
            tracing_client_context: &tracing_client_context,
            agent_messenger: &agent_messenger,
        })
        .await;

    Ok(GatewayComponents {
        layout,
        tz,
        agent,
        observer: providers.observer,
        merge_writer,
        subconscious,
        action_store,
        action_notify,
        mcp_registry: net.mcp_registry,
        tools_path: net.tools_path,
        skill_state,
        hybrid_searcher: mem.hybrid_searcher,
        pulse_enabled: cfg.pulse_enabled,
        endpoint_registry: net.endpoint_registry,
        channel_configs: net.channel_configs,
        http_client: http.clone(),
        session_runtime,
        session_registry,
        session_store,
        agent_messenger,
        conversation_router,
        spawn_context,
        path_policy: path_policy_for_runtime,
        output_topic_override_tx,
        tracing_service,
        tracing_client_context,
    })
}

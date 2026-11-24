mod memory;
mod providers;
mod tools;

pub use providers::init_providers;

use std::sync::Arc;

use residuum_agent::Agent;
use residuum_agent::subconscious::Subconscious;
use residuum_background::SessionRuntime;
use residuum_background::conversation_router::ConversationRouter;
use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_background::store::SessionStore;
use residuum_background::team::TeamLink;
use residuum_bus::EndpointRegistry;
use residuum_config::SharedToolsPath;
use residuum_config::{Config, HubConfig, WorkspaceLayout};
use residuum_core::util::FatalError;
use residuum_hub::services::HubServices;
use residuum_inference::SharedHttpClient;
use residuum_mcp::SharedMcpRegistry;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_memory::search::HybridSearcher;
use residuum_notify::channels::InboxChannel;
use residuum_proactivity::actions::store::ActionStore;
use residuum_skills::{SharedSkillState, SkillIndex, SkillState};
use residuum_tools::registration::RegistryInjections;
use residuum_workspace::bootstrap::ensure_workspace_labeled;
use residuum_workspace::identity::IdentityFiles;
use residuum_workspace::team_files::TeamWriteCoordinator;

use residuum_background::SpawnContext;

use residuum_tools::main_tools::{self, ToolRegistryDeps};
use tools::CreateAgentArgs;

pub(crate) struct GatewayComponents {
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    pub tool_injections: RegistryInjections,
    pub observer: Observer,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub subconscious: Arc<residuum_agent::subconscious::Subconscious>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub tools_path: SharedToolsPath,
    pub agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    pub skill_state: SharedSkillState,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub pulse_enabled: bool,
    pub endpoint_registry: EndpointRegistry,
    pub channel_configs: Vec<residuum_config::ExternalChannelConfig>,
    pub http_client: SharedHttpClient,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    pub session_store: Arc<SessionStore>,
    pub agent_messenger: Arc<AgentMessenger>,
    pub conversation_router: Arc<ConversationRouter>,
    pub spawn_context: Arc<SpawnContext>,
    pub lifecycle: residuum_tools::LifecycleAccess,
    pub path_policy: residuum_tools::SharedPathPolicy,
    pub auto_mode: residuum_agent::auto_mode::SharedAutoMode,
    pub team: TeamWriteCoordinator,
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
    pub a2a_hub: Arc<residuum_a2a::A2aClientHub>,
    pub a2a_tracker: Arc<residuum_a2a::RemoteTaskTracker>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub config_reload_tracker: residuum_tools::SharedConfigReloadTracker,
}

pub(super) async fn init_workspace(
    cfg: &Config,
    team: &TeamWriteCoordinator,
) -> Result<(WorkspaceLayout, chrono_tz::Tz), FatalError> {
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    let tz = cfg.timezone;
    let shown = shown_name(&cfg.config_dir, &cfg.agent_name).await;
    ensure_workspace_labeled(&layout, team, None, Some(cfg.timezone.name()), Some(&shown)).await?;

    Ok((layout, tz))
}

async fn shown_name(config_dir: &std::path::Path, folder: &str) -> String {
    let path = config_dir.join("config.toml");
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => residuum_config::display_name_in_toml(&text, folder),
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "couldn't read the agent's name from config.toml; using the folder name"
            );
            folder.to_string()
        }
    }
}

fn init_checkpoints(
    layout: &WorkspaceLayout,
    cfg: &Config,
    hub: &HubConfig,
    publisher: &residuum_bus::Publisher,
    shared: &HubServices,
) -> Result<Arc<residuum_checkpoints::CheckpointEngine>, FatalError> {
    let checkpoints_dir = residuum_config::HubPaths::new(&hub.config_dir).checkpoints_dir();
    residuum_checkpoints::CheckpointEngine::with_shared_repos(
        Arc::clone(&shared.checkpoints),
        &cfg.agent_name,
        layout.root().to_path_buf(),
        cfg.config_dir.clone(),
        &checkpoints_dir,
        Some(publisher.clone()),
    )
    .map(|engine| Arc::new(engine.with_team_coordinator(shared.team.clone())))
    .map_err(|e| {
        FatalError::Config(format!(
            "failed to open checkpoint repositories at {}: {e}",
            checkpoints_dir.display()
        ))
    })
}

pub(super) async fn init_identity_and_http(
    layout: &WorkspaceLayout,
    cfg: &Config,
) -> Result<(IdentityFiles, SharedHttpClient), FatalError> {
    let identity = IdentityFiles::load(layout).await?;
    identity.warn_missing(layout);
    let http = SharedHttpClient::new(&residuum_inference::HttpClientConfig::with_timeout(
        cfg.timeout_secs,
    ))
    .map_err(|e| FatalError::Config(format!("failed to build HTTP client: {e}")))?;
    Ok((identity, http))
}

pub(crate) fn init_session_observer(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<Observer, FatalError> {
    memory::build_observer(cfg, tz, http, None)
}

fn degradation_notice(degradations: &[String]) -> Option<String> {
    if degradations.is_empty() {
        return None;
    }
    let count = degradations.len();
    let plural = if count == 1 { "" } else { "s" };
    Some(format!(
        "Residuum started with {count} thing{plural} degraded: {}.",
        degradations.join("; ")
    ))
}

async fn init_action_store(
    layout: &WorkspaceLayout,
    publisher: &residuum_bus::Publisher,
    degradations: &mut Vec<String>,
) -> (
    Arc<tokio::sync::Mutex<ActionStore>>,
    Arc<tokio::sync::Notify>,
) {
    let actions_path = layout.scheduled_actions_json();
    let action_store = match ActionStore::load(&actions_path).await {
        Ok((store, rejected, moved_aside)) => {
            if !rejected.is_empty() {
                residuum_bus::notice::publish_notice(
                    publisher,
                    residuum_proactivity::actions::store::rejected_actions_notice(&rejected),
                )
                .await;
            }
            if let Some(moved_to) = moved_aside {
                residuum_bus::notice::publish_notice(
                    publisher,
                    residuum_proactivity::actions::store::corrupt_actions_notice(&moved_to),
                )
                .await;
            }
            Arc::new(tokio::sync::Mutex::new(store))
        }
        Err(err) => {
            tracing::warn!(error = %err, "action store degraded: starting empty");
            degradations.push(format!(
                "scheduled actions couldn't be loaded and started empty: {err}"
            ));
            Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
                actions_path,
            )))
        }
    };
    let action_notify = Arc::new(tokio::sync::Notify::new());
    (action_store, action_notify)
}

async fn init_skills(
    cfg: &Config,
    degradations: &mut Vec<String>,
    publisher: &residuum_bus::Publisher,
) -> SharedSkillState {
    let skill_index = match SkillIndex::scan(&cfg.skills.dirs).await {
        Ok(idx) => {
            for (dir, err) in idx.skipped_dirs() {
                degradations.push(format!(
                    "your skills directory \"{}\" couldn't be read and was skipped, but skills in your other directories still loaded: {err}",
                    dir.display()
                ));
            }
            idx
        }
        Err(err) => {
            tracing::warn!(error = %err, "skill index degraded: starting empty");
            degradations.push(format!(
                "skills couldn't be scanned and started empty: {err}"
            ));
            SkillIndex::default()
        }
    };
    for notice in skill_index.notices() {
        residuum_bus::notice::publish_notice(publisher, notice.clone()).await;
    }
    SkillState::new_shared(skill_index, cfg.skills.dirs.clone())
}

fn build_session_memory_components(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
    layout: &WorkspaceLayout,
    reflector: residuum_memory::reflector::Reflector,
    mem: &memory::MemoryComponents,
    embedding_provider: Option<Arc<dyn residuum_inference::EmbeddingProvider>>,
) -> (Arc<Observer>, Arc<MemoryMergeWriter>) {
    let session_observer = Arc::new(match memory::build_observer(cfg, tz, http, None) {
        Ok(observer) => observer,
        Err(err) => {
            tracing::warn!(error = %err, "session observer degraded: disabled");
            Observer::disabled(tz)
        }
    });
    let merge_writer = Arc::new(MemoryMergeWriter::new(
        reflector,
        layout.clone(),
        Arc::clone(&mem.search_index),
        mem.vector_store.clone(),
        embedding_provider,
    ));
    (session_observer, merge_writer)
}

struct StartupSpawnContextInputs<'a> {
    cfg: &'a Config,
    hub_dir: &'a std::path::Path,
    layout: &'a WorkspaceLayout,
    tz: chrono_tz::Tz,
    http_client: SharedHttpClient,
    session_runtime: &'a Arc<SessionRuntime>,
    session_registry: &'a Arc<SessionRegistry>,
    endpoint_registry: &'a EndpointRegistry,
    publisher: &'a residuum_bus::Publisher,
    action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: &'a Arc<tokio::sync::Notify>,
    hybrid_searcher: &'a Arc<HybridSearcher>,
    skill_state: &'a SharedSkillState,
    mcp_registry: &'a SharedMcpRegistry,
    session_observer: &'a Arc<Observer>,
    merge_writer: &'a Arc<MemoryMergeWriter>,
    messenger: &'a Arc<AgentMessenger>,
    tracing_service: &'a Arc<residuum_tracing_service::TracingService>,
    tracing_client_context: &'a Arc<residuum_tracing_service::ClientContext>,
    build: residuum_core::build_info::BuildInfo,
    web_search_backend: Option<residuum_config::StandaloneBackendConfig>,
    tools_path: &'a SharedToolsPath,
    path_policy: &'a residuum_tools::SharedPathPolicy,
    auto_mode: &'a residuum_agent::auto_mode::SharedAutoMode,
    agent_keys: &'a residuum_config::agent_keys::SharedAgentKeys,
    a2a_hub: &'a Arc<residuum_a2a::A2aClientHub>,
    a2a_tracker: &'a Arc<residuum_a2a::RemoteTaskTracker>,
    checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    lifecycle: &'a residuum_tools::LifecycleAccess,
    model_catalog: &'a residuum_config::SharedModelCatalog,
    mailboxes: &'a Arc<dyn residuum_workspace::inbox::mail::MailboxDirectory>,
}

fn build_startup_spawn_context(inputs: StartupSpawnContextInputs<'_>) -> Arc<SpawnContext> {
    let session_tools = Arc::new(residuum_tools::SessionTools {
        path_policy: Arc::clone(inputs.path_policy),
        tools_path: Arc::clone(inputs.tools_path),
        agent_keys: Arc::clone(inputs.agent_keys),
        checkpoints: Arc::clone(inputs.checkpoints),
        publisher: inputs.publisher.clone(),
        endpoint_registry: inputs.endpoint_registry.clone(),
        session_registry: Arc::clone(inputs.session_registry),
        action_store: Arc::clone(inputs.action_store),
        action_notify: Arc::clone(inputs.action_notify),
        hybrid_searcher: Arc::clone(inputs.hybrid_searcher),
        tracing_service: Arc::clone(inputs.tracing_service),
        tracing_client_context: Arc::clone(inputs.tracing_client_context),
        build: inputs.build,
        web_search_backend: inputs.web_search_backend,
        model_catalog: Arc::clone(inputs.model_catalog),
        a2a_hub: Arc::clone(inputs.a2a_hub),
        a2a_tracker: Arc::clone(inputs.a2a_tracker),
        lifecycle: inputs.lifecycle.clone(),
        messenger: Arc::clone(inputs.messenger),
        layout: inputs.layout.clone(),
        config_dir: inputs.cfg.config_dir.clone(),
        hub_dir: inputs.hub_dir.to_path_buf(),
        tz: inputs.tz,
        depth_cap: inputs.cfg.background.subagent_depth_cap,
        auto_mode: Some(Arc::clone(inputs.auto_mode)),
        mailboxes: Some(Arc::clone(inputs.mailboxes)),
    });
    Arc::new(SpawnContext {
        background_config: inputs.cfg.background.clone(),
        main_provider_specs: inputs.cfg.main.clone(),
        model_catalog: Arc::clone(inputs.model_catalog),
        http_client: inputs.http_client,
        max_tokens: inputs.cfg.max_tokens,
        retry_config: inputs.cfg.retry.clone(),
        options: residuum_inference::CompletionOptions {
            max_tokens: Some(inputs.cfg.max_tokens),
            temperature: inputs.cfg.temperature,
            thinking: inputs.cfg.thinking.clone(),
            ..residuum_inference::CompletionOptions::default()
        },
        max_tool_iterations: inputs.cfg.agent.max_tool_iterations,
        repeat_call_guard: inputs.cfg.agent.repeat_call_guard,
        intermediate_delivery: residuum_agent::context::IntermediateDelivery::from_config(
            inputs.cfg,
        ),
        layout: inputs.layout.clone(),
        role_overrides: inputs.cfg.role_overrides.clone(),
        session_tools,
        session_runtime: Arc::clone(inputs.session_runtime),
        session_registry: Arc::clone(inputs.session_registry),
        publisher: inputs.publisher.clone(),
        skill_state: Arc::clone(inputs.skill_state),
        mcp_registry: Arc::clone(inputs.mcp_registry),
        observer: Arc::clone(inputs.session_observer),
        merge_writer: Arc::clone(inputs.merge_writer),
        messenger: Arc::clone(inputs.messenger),
        bg_tier_active_index: residuum_background::BackgroundTierActiveIndex::default(),
        action_outcome: Some(crate::actions::action_outcome_sink(inputs.action_store)),
    })
}

async fn init_session_registry_and_messenger(
    cfg: &Config,
    layout: &WorkspaceLayout,
    publisher: &residuum_bus::Publisher,
    session_observer: &Arc<Observer>,
    merge_writer: &Arc<MemoryMergeWriter>,
    team: TeamLink,
) -> (
    Arc<SessionRegistry>,
    Arc<SessionStore>,
    Arc<AgentMessenger>,
    Arc<ConversationRouter>,
) {
    let registry = Arc::new(SessionRegistry::load(layout.resume_points_json()).await);
    let store = Arc::new(SessionStore::new(layout.sessions_dir()));

    let recovery_store = Arc::clone(&store);
    let recovery_observer = Arc::clone(session_observer);
    let recovery_merge_writer = Arc::clone(merge_writer);
    let recovery_layout = layout.clone();
    let episode_skip_token_floor = cfg.background.episode_skip_token_floor;
    let tz = cfg.timezone;
    residuum_core::util::spawn_in_span(async move {
        let recovery_env = residuum_background::SessionMemoryEnv {
            observer: &recovery_observer,
            merge_writer: &recovery_merge_writer,
            layout: &recovery_layout,
            episode_skip_token_floor,
            tz,
        };
        let recovered = recovery_store.recover_incomplete_runs(&recovery_env).await;
        if recovered > 0 {
            tracing::warn!(
                recovered,
                "recovered session runs left incomplete by a prior process exit"
            );
        }
    });

    let messenger = Arc::new(
        AgentMessenger::new(
            Arc::clone(&registry),
            publisher.clone(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits::from(&cfg.background),
        )
        .with_team(team),
    );
    let conversation_router = Arc::new(ConversationRouter::new(Arc::clone(&messenger)));
    (registry, store, messenger, conversation_router)
}

struct SessionRuntimeInputs<'a> {
    cfg: &'a Config,
    publisher: &'a residuum_bus::Publisher,
    registry: &'a Arc<SessionRegistry>,
    store: &'a Arc<SessionStore>,
    messenger: &'a Arc<AgentMessenger>,
    checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    tracing_service: &'a Arc<residuum_tracing_service::TracingService>,
    tracing_client_context: &'a Arc<residuum_tracing_service::ClientContext>,
    session_budget: &'a Arc<tokio::sync::Semaphore>,
}

fn build_session_runtime(inputs: &SessionRuntimeInputs<'_>) -> Arc<SessionRuntime> {
    Arc::new(SessionRuntime::new(
        Arc::clone(inputs.registry),
        Arc::clone(inputs.store),
        Arc::clone(inputs.session_budget),
        &inputs.cfg.background,
        residuum_background::SessionRuntimeHandles {
            publisher: inputs.publisher.clone(),
            tz: inputs.cfg.timezone,
            messenger: Arc::clone(inputs.messenger),
            checkpoints: Arc::clone(inputs.checkpoints),
            tracing_service: Arc::clone(inputs.tracing_service),
            tracing_client_context: Arc::clone(inputs.tracing_client_context),
        },
    ))
}

async fn init_mcp_servers(
    layout: &WorkspaceLayout,
    tools_path: SharedToolsPath,
    agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    secrets_dir: std::path::PathBuf,
    degradations: &mut Vec<String>,
) -> SharedMcpRegistry {
    let mcp_registry = residuum_mcp::McpRegistry::new_shared_with_spawn_env(
        tools_path,
        agent_keys,
        secrets_dir,
        layout.root().to_path_buf(),
    );
    match residuum_workspace::config::load_mcp_servers(&layout.mcp_json()) {
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
                    degradations.push(format!(
                        "the MCP server '{server_name}' failed to start: {err}"
                    ));
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "workspace MCP servers degraded");
            degradations.push(format!("workspace MCP servers couldn't be loaded: {err}"));
        }
    }
    mcp_registry
}

fn web_search_mcp_entry(
    backend: &residuum_config::StandaloneBackendConfig,
) -> Option<residuum_mcp::types::McpServerEntry> {
    match backend.name.as_str() {
        "brave" => Some(residuum_mcp::types::McpServerEntry {
            name: "brave_web_search".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@anthropic-ai/mcp-server-brave-search".to_string(),
            ],
            env: std::collections::BTreeMap::from([(
                "BRAVE_API_KEY".to_string(),
                backend.api_key.clone(),
            )]),
            transport: residuum_mcp::types::McpTransport::Stdio,
            headers: std::collections::BTreeMap::new(),
            timeout_secs: None,
        }),
        "tavily" => Some(residuum_mcp::types::McpServerEntry {
            name: "tavily_web_search".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "tavily-mcp".to_string()],
            env: std::collections::BTreeMap::from([(
                "TAVILY_API_KEY".to_string(),
                backend.api_key.clone(),
            )]),
            transport: residuum_mcp::types::McpTransport::Stdio,
            headers: std::collections::BTreeMap::new(),
            timeout_secs: None,
        }),
        _ => None,
    }
}

pub(crate) async fn connect_web_search_mcp(
    cfg: &Config,
    mcp_registry: &SharedMcpRegistry,
) -> residuum_mcp::McpReconcileReport {
    let Some(backend) = &cfg.web_search.standalone_backend else {
        return residuum_mcp::McpReconcileReport::default();
    };
    let Some(entry) = web_search_mcp_entry(backend) else {
        return residuum_mcp::McpReconcileReport::default();
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

pub(crate) fn with_web_search_mcp_entry(
    mut servers: Vec<residuum_mcp::types::McpServerEntry>,
    cfg: &Config,
) -> Vec<residuum_mcp::types::McpServerEntry> {
    let entry = cfg
        .web_search
        .standalone_backend
        .as_ref()
        .and_then(web_search_mcp_entry);
    if let Some(entry) = entry
        && !servers.iter().any(|s| s.name == entry.name)
    {
        servers.push(entry);
    }
    servers
}

async fn init_a2a_client(
    layout: &WorkspaceLayout,
    agent_keys: &residuum_config::agent_keys::SharedAgentKeys,
    messenger: Arc<AgentMessenger>,
    timezone: chrono_tz::Tz,
) -> (
    Arc<residuum_a2a::A2aClientHub>,
    Arc<residuum_a2a::RemoteTaskTracker>,
) {
    let hub = residuum_a2a::A2aClientHub::new_shared();
    hub.reload_from_file(&layout.a2a_agents_json(), agent_keys)
        .await;
    hub.spawn_background_refresh();

    let tracker = residuum_a2a::RemoteTaskTracker::load(
        layout.a2a_outbound_json(),
        Arc::clone(&hub),
        messenger,
        layout.agent_inbox_dir(),
        timezone,
    )
    .await;
    tracker.spawn_resume_watchers().await;

    (hub, tracker)
}

fn init_channels_and_registry(
    layout: &WorkspaceLayout,
    cfg: &Config,
    degradations: &mut Vec<String>,
) -> (
    Vec<residuum_config::ExternalChannelConfig>,
    EndpointRegistry,
) {
    let channel_configs =
        match residuum_workspace::config::load_channel_configs(&layout.channels_toml()) {
            Ok(configs) => configs,
            Err(err) => {
                tracing::warn!(error = %err, "workspace channels degraded");
                degradations.push(format!("notification channels couldn't be loaded: {err}"));
                Vec::new()
            }
        };
    let endpoint_registry = EndpointRegistry::from_config(cfg, &channel_configs);
    (channel_configs, endpoint_registry)
}

struct NetworkingComponents {
    tools_path: SharedToolsPath,
    agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    mcp_registry: SharedMcpRegistry,
    channel_configs: Vec<residuum_config::ExternalChannelConfig>,
    endpoint_registry: EndpointRegistry,
}

async fn init_networking(
    cfg: &Config,
    agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    secrets_dir: std::path::PathBuf,
    layout: &WorkspaceLayout,
    degradations: &mut Vec<String>,
) -> NetworkingComponents {
    let tools_path: SharedToolsPath =
        Arc::new(tokio::sync::RwLock::new(cfg.tools.effective_path()));
    let mcp_registry = init_mcp_servers(
        layout,
        Arc::clone(&tools_path),
        Arc::clone(&agent_keys),
        secrets_dir,
        degradations,
    )
    .await;
    let web_search_report = connect_web_search_mcp(cfg, &mcp_registry).await;
    for (server_name, err) in &web_search_report.failures {
        degradations.push(format!(
            "the web search server '{server_name}' failed to start: {err}"
        ));
    }
    let (channel_configs, endpoint_registry) =
        init_channels_and_registry(layout, cfg, degradations);
    NetworkingComponents {
        tools_path,
        agent_keys,
        mcp_registry,
        channel_configs,
        endpoint_registry,
    }
}

pub(crate) async fn spawn_notify_subscribers(
    bus_handle: &residuum_bus::BusHandle,
    channel_configs: &[residuum_config::ExternalChannelConfig],
    http: &SharedHttpClient,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) -> Vec<tokio::task::JoinHandle<()>> {
    use residuum_bus::{NotifyName, topics};
    use residuum_notify::subscriber::run_notify_subscriber;

    let external_channels =
        residuum_notify::build_external_channels(channel_configs, http.client()).await;

    let mut handles = Vec::new();

    for (name, channel) in external_channels {
        let topic = topics::Notification(NotifyName::from(name.as_str()));
        match bus_handle.subscribe(topic).await {
            Ok(subscriber) => {
                let handle =
                    residuum_core::util::spawn_in_span(run_notify_subscriber(subscriber, channel));
                handles.push(handle);
                tracing::info!(channel = %name, "notify subscriber spawned");
            }
            Err(e) => {
                tracing::warn!(channel = %name, error = %e, "failed to subscribe notify channel");
            }
        }
    }

    let inbox_channel = InboxChannel::new(layout.agent_inbox_dir(), tz);
    match bus_handle.subscribe(topics::Inbox).await {
        Ok(subscriber) => {
            let handle = residuum_core::util::spawn_in_span(run_notify_subscriber(
                subscriber,
                Box::new(inbox_channel),
            ));
            handles.push(handle);
            tracing::info!("inbox notify subscriber spawned");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe inbox channel");
        }
    }

    handles
}

fn init_tracing_client_context(
    cfg: &Config,
    build: residuum_core::build_info::BuildInfo,
) -> Arc<residuum_tracing_service::ClientContext> {
    Arc::new(residuum_tracing_service::client_context::gather_for_bug_report(build, cfg))
}

struct ToolsAndAgentInputs<'a> {
    cfg: &'a Config,
    layout: &'a WorkspaceLayout,
    mem: &'a memory::MemoryComponents,
    tz: chrono_tz::Tz,
    tool_deps: ToolRegistryDeps<'a>,
    mcp_registry: &'a SharedMcpRegistry,
    provider: Box<dyn residuum_inference::InferenceProvider>,
    options: residuum_inference::CompletionOptions,
    identity: IdentityFiles,
    hop_counter: residuum_agent::HopCounter,
    degradations: &'a mut Vec<String>,
}

async fn build_tools_and_agent(
    inputs: ToolsAndAgentInputs<'_>,
) -> (
    Agent,
    RegistryInjections,
    tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
) {
    let (tools, tool_injections, output_topic_override_tx) = main_tools::init_tool_registry(
        inputs.cfg,
        inputs.layout,
        &inputs.mem.hybrid_searcher,
        inputs.tz,
        &inputs.tool_deps,
    );

    if let Some(store) = tools.result_store().cloned() {
        drop(residuum_core::util::spawn_in_span(async move {
            store.sweep_expired().await;
        }));
    }

    inputs
        .mcp_registry
        .write()
        .await
        .set_reserved_tool_names(tools.tool_names());

    let mut agent = tools::create_agent(
        CreateAgentArgs {
            provider: inputs.provider,
            options: inputs.options,
            max_tool_iterations: inputs.cfg.agent.max_tool_iterations,
            repeat_call_guard: inputs.cfg.agent.repeat_call_guard,
            tools,
            identity: inputs.identity,
            hop_counter: inputs.hop_counter,
        },
        inputs.mcp_registry,
        inputs.tz,
        inputs.layout,
        inputs.degradations,
    )
    .await;
    agent.set_team(Arc::new(inputs.tool_deps.agent_messenger.team().clone()));

    (agent, tool_injections, output_topic_override_tx)
}

struct MainAgentInputs<'a> {
    cfg: &'a Config,
    hub_dir: &'a std::path::Path,
    layout: &'a WorkspaceLayout,
    mem: &'a memory::MemoryComponents,
    tz: chrono_tz::Tz,
    identity: IdentityFiles,
    provider: Box<dyn residuum_inference::InferenceProvider>,
    options: residuum_inference::CompletionOptions,
    net: &'a NetworkingComponents,
    path_policy: &'a residuum_tools::SharedPathPolicy,
    auto_mode: &'a residuum_agent::auto_mode::SharedAutoMode,
    action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: &'a Arc<tokio::sync::Notify>,
    skill_state: &'a SharedSkillState,
    session_registry: &'a Arc<SessionRegistry>,
    publisher: &'a residuum_bus::Publisher,
    tracing_service: &'a Arc<residuum_tracing_service::TracingService>,
    build: residuum_core::build_info::BuildInfo,
    tracing_client_context: &'a Arc<residuum_tracing_service::ClientContext>,
    agent_messenger: &'a Arc<AgentMessenger>,
    a2a_hub: &'a Arc<residuum_a2a::A2aClientHub>,
    a2a_tracker: &'a Arc<residuum_a2a::RemoteTaskTracker>,
    checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    lifecycle: &'a residuum_tools::LifecycleAccess,
    model_catalog: &'a residuum_config::SharedModelCatalog,
    mailboxes: &'a Arc<dyn residuum_workspace::inbox::mail::MailboxDirectory>,
    degradations: &'a mut Vec<String>,
    config_reload_tracker: &'a residuum_tools::SharedConfigReloadTracker,
}

async fn build_main_agent(
    inputs: MainAgentInputs<'_>,
) -> (
    Agent,
    RegistryInjections,
    tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
) {
    let hop_counter = residuum_agent::HopCounter::new(0);

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
            path_policy: inputs.path_policy,
            auto_mode: inputs.auto_mode,
            agent_keys: &inputs.net.agent_keys,
            session_registry: inputs.session_registry,
            endpoint_registry: &inputs.net.endpoint_registry,
            publisher: inputs.publisher,
            tracing_service: inputs.tracing_service,
            build: inputs.build,
            tracing_client_context: inputs.tracing_client_context,
            agent_messenger: inputs.agent_messenger,
            hop_counter: &hop_counter,
            a2a_hub: inputs.a2a_hub,
            a2a_tracker: inputs.a2a_tracker,
            checkpoints: inputs.checkpoints,
            config_reload_tracker: inputs.config_reload_tracker,
            hub_dir: inputs.hub_dir,
            lifecycle: inputs.lifecycle,
            model_catalog: inputs.model_catalog,
            mailboxes: Some(Arc::clone(inputs.mailboxes)),
        },
        mcp_registry: &inputs.net.mcp_registry,
        provider: inputs.provider,
        options: inputs.options,
        identity: inputs.identity,
        hop_counter: hop_counter.clone(),
        degradations: inputs.degradations,
    })
    .await
}

struct AgentInitInputs<'a> {
    cfg: &'a Config,
    hub_dir: &'a std::path::Path,
    layout: &'a WorkspaceLayout,
    tz: chrono_tz::Tz,
    http_client: SharedHttpClient,
    mem: &'a memory::MemoryComponents,
    infra: &'a SupportingInfra,
    session_runtime: &'a Arc<SessionRuntime>,
    session_registry: &'a Arc<SessionRegistry>,
    session_observer: &'a Arc<Observer>,
    merge_writer: &'a Arc<MemoryMergeWriter>,
    action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: &'a Arc<tokio::sync::Notify>,
    skill_state: &'a SharedSkillState,
    publisher: &'a residuum_bus::Publisher,
    agent_messenger: &'a Arc<AgentMessenger>,
    tracing_service: &'a Arc<residuum_tracing_service::TracingService>,
    build: residuum_core::build_info::BuildInfo,
    checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    directory: &'a residuum_background::team::DirectoryHandle,
    mailboxes: &'a residuum_workspace::inbox::mail::MailboxDirectoryHandle,
    identity: IdentityFiles,
    provider: Box<dyn residuum_inference::InferenceProvider>,
    options: residuum_inference::CompletionOptions,
    degradations: &'a mut Vec<String>,
}

struct SpawnContextAndAgent {
    spawn_context: Arc<SpawnContext>,
    lifecycle: residuum_tools::LifecycleAccess,
    agent: Agent,
    tool_injections: RegistryInjections,
    output_topic_override_tx: tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
}

async fn build_spawn_context_and_agent(inputs: AgentInitInputs<'_>) -> SpawnContextAndAgent {
    let lifecycle = residuum_tools::LifecycleAccess::new(
        inputs.directory.clone(),
        inputs.cfg.agent_name.clone(),
    );
    let model_catalog = inputs.cfg.model_catalog.clone().into_shared();
    let mailboxes: Arc<dyn residuum_workspace::inbox::mail::MailboxDirectory> =
        Arc::new(inputs.mailboxes.clone());
    let infra = inputs.infra;
    let spawn_context = build_startup_spawn_context(StartupSpawnContextInputs {
        cfg: inputs.cfg,
        hub_dir: inputs.hub_dir,
        layout: inputs.layout,
        tz: inputs.tz,
        http_client: inputs.http_client,
        session_runtime: inputs.session_runtime,
        session_registry: inputs.session_registry,
        endpoint_registry: &infra.net.endpoint_registry,
        publisher: inputs.publisher,
        action_store: inputs.action_store,
        action_notify: inputs.action_notify,
        hybrid_searcher: &inputs.mem.hybrid_searcher,
        skill_state: inputs.skill_state,
        mcp_registry: &infra.net.mcp_registry,
        session_observer: inputs.session_observer,
        merge_writer: inputs.merge_writer,
        messenger: inputs.agent_messenger,
        tracing_service: inputs.tracing_service,
        tracing_client_context: &infra.tracing_client_context,
        build: inputs.build,
        web_search_backend: inputs.cfg.web_search.standalone_backend.clone(),
        tools_path: &infra.net.tools_path,
        path_policy: &infra.path_policy,
        auto_mode: &infra.auto_mode,
        agent_keys: &infra.net.agent_keys,
        a2a_hub: &infra.a2a_hub,
        a2a_tracker: &infra.a2a_tracker,
        checkpoints: inputs.checkpoints,
        lifecycle: &lifecycle,
        model_catalog: &model_catalog,
        mailboxes: &mailboxes,
    });

    let (agent, tool_injections, output_topic_override_tx) = build_main_agent(MainAgentInputs {
        cfg: inputs.cfg,
        hub_dir: inputs.hub_dir,
        layout: inputs.layout,
        mem: inputs.mem,
        tz: inputs.tz,
        build: inputs.build,
        identity: inputs.identity,
        provider: inputs.provider,
        options: inputs.options,
        net: &infra.net,
        path_policy: &infra.path_policy,
        auto_mode: &infra.auto_mode,
        action_store: inputs.action_store,
        action_notify: inputs.action_notify,
        skill_state: inputs.skill_state,
        session_registry: inputs.session_registry,
        publisher: inputs.publisher,
        tracing_service: inputs.tracing_service,
        tracing_client_context: &infra.tracing_client_context,
        agent_messenger: inputs.agent_messenger,
        a2a_hub: &infra.a2a_hub,
        a2a_tracker: &infra.a2a_tracker,
        checkpoints: inputs.checkpoints,
        lifecycle: &lifecycle,
        model_catalog: &model_catalog,
        mailboxes: &mailboxes,
        degradations: inputs.degradations,
        config_reload_tracker: &infra.config_reload_tracker,
    })
    .await;

    SpawnContextAndAgent {
        spawn_context,
        lifecycle,
        agent,
        tool_injections,
        output_topic_override_tx,
    }
}

fn build_path_policy(
    cfg: &Config,
    layout: &WorkspaceLayout,
    hub: &HubConfig,
    team: &TeamWriteCoordinator,
) -> residuum_tools::path_policy::SharedPathPolicy {
    residuum_tools::PathPolicy::new_shared_with_team(
        residuum_tools::path_policy::blocked_write_paths(cfg, layout, &hub.config_dir),
        team.view_for_agent(&cfg.agent_name, layout.root()),
    )
}

async fn publish_degradation_notice(publisher: &residuum_bus::Publisher, degradations: &[String]) {
    if let Some(message) = degradation_notice(degradations) {
        residuum_bus::notice::publish_notice(publisher, message).await;
    }
}

async fn publish_load_notices(publisher: &residuum_bus::Publisher, cfg: &Config) {
    for notice in &cfg.load_notices {
        residuum_bus::notice::publish_notice(publisher, notice.clone()).await;
    }
}

async fn init_workspace_and_checkpoints(
    cfg: &Config,
    hub: &HubConfig,
    publisher: &residuum_bus::Publisher,
    shared: &HubServices,
) -> Result<
    (
        WorkspaceLayout,
        chrono_tz::Tz,
        Arc<residuum_checkpoints::CheckpointEngine>,
    ),
    FatalError,
> {
    let (layout, tz) = init_workspace(cfg, &shared.team).await?;
    let checkpoints = init_checkpoints(&layout, cfg, hub, publisher, shared)?;
    publish_load_notices(publisher, cfg).await;
    Ok((layout, tz, checkpoints))
}

struct SessionSubsystemInputs<'a> {
    team: TeamLink,
    cfg: &'a Config,
    layout: &'a WorkspaceLayout,
    tz: chrono_tz::Tz,
    publisher: &'a residuum_bus::Publisher,
    http: SharedHttpClient,
    reflector: residuum_memory::reflector::Reflector,
    mem: &'a memory::MemoryComponents,
    embedding_provider: Option<Arc<dyn residuum_inference::EmbeddingProvider>>,
    degradations: &'a mut Vec<String>,
}

struct SessionSubsystems {
    action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    action_notify: Arc<tokio::sync::Notify>,
    skill_state: SharedSkillState,
    session_observer: Arc<Observer>,
    merge_writer: Arc<MemoryMergeWriter>,
    session_registry: Arc<SessionRegistry>,
    session_store: Arc<SessionStore>,
    agent_messenger: Arc<AgentMessenger>,
    conversation_router: Arc<ConversationRouter>,
}

async fn init_session_subsystems(inputs: SessionSubsystemInputs<'_>) -> SessionSubsystems {
    let (action_store, action_notify) =
        init_action_store(inputs.layout, inputs.publisher, inputs.degradations).await;
    let skill_state = init_skills(inputs.cfg, inputs.degradations, inputs.publisher).await;

    let (session_observer, merge_writer) = build_session_memory_components(
        inputs.cfg,
        inputs.tz,
        inputs.http,
        inputs.layout,
        inputs.reflector,
        inputs.mem,
        inputs.embedding_provider,
    );
    let (session_registry, session_store, agent_messenger, conversation_router) =
        init_session_registry_and_messenger(
            inputs.cfg,
            inputs.layout,
            inputs.publisher,
            &session_observer,
            &merge_writer,
            inputs.team,
        )
        .await;

    SessionSubsystems {
        action_store,
        action_notify,
        skill_state,
        session_observer,
        merge_writer,
        session_registry,
        session_store,
        agent_messenger,
        conversation_router,
    }
}

struct SupportingInfra {
    net: NetworkingComponents,
    tracing_client_context: Arc<residuum_tracing_service::ClientContext>,
    path_policy: residuum_tools::SharedPathPolicy,
    auto_mode: residuum_agent::auto_mode::SharedAutoMode,
    a2a_hub: Arc<residuum_a2a::A2aClientHub>,
    a2a_tracker: Arc<residuum_a2a::RemoteTaskTracker>,
    config_reload_tracker: residuum_tools::SharedConfigReloadTracker,
}

async fn init_supporting_infra(
    cfg: &Config,
    hub: &HubConfig,
    layout: &WorkspaceLayout,
    degradations: &mut Vec<String>,
    agent_messenger: Arc<AgentMessenger>,
    shared: &HubServices,
) -> SupportingInfra {
    let net = init_networking(
        cfg,
        Arc::clone(&shared.agent_keys),
        hub.config_dir.clone(),
        layout,
        degradations,
    )
    .await;
    let tracing_client_context = init_tracing_client_context(cfg, shared.control.build);
    let path_policy = build_path_policy(cfg, layout, hub, &shared.team);
    let (a2a_hub, a2a_tracker) =
        init_a2a_client(layout, &net.agent_keys, agent_messenger, cfg.timezone).await;
    let auto_mode = residuum_agent::auto_mode::AutoModeGate::new_shared(
        cfg.agent_name.clone(),
        cfg.auto_mode.clone(),
        Arc::clone(&shared.system_one),
        residuum_inference::UsageRecorder::new(layout),
    );
    SupportingInfra {
        net,
        tracing_client_context,
        path_policy,
        auto_mode,
        a2a_hub,
        a2a_tracker,
        config_reload_tracker: residuum_tools::SharedConfigReloadTracker::new_shared(),
    }
}

struct SharedServices<'a> {
    checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    hub: &'a HubServices,
}

impl<'a> SharedServices<'a> {
    fn new(
        checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
        hub: &'a HubServices,
    ) -> Self {
        Self { checkpoints, hub }
    }
}

async fn init_infra_and_session_runtime(
    cfg: &Config,
    hub: &HubConfig,
    layout: &WorkspaceLayout,
    degradations: &mut Vec<String>,
    publisher: &residuum_bus::Publisher,
    sess: &SessionSubsystems,
    shared: &SharedServices<'_>,
) -> (SupportingInfra, Arc<SessionRuntime>) {
    let infra = init_supporting_infra(
        cfg,
        hub,
        layout,
        degradations,
        Arc::clone(&sess.agent_messenger),
        shared.hub,
    )
    .await;
    let session_runtime = build_session_runtime(&SessionRuntimeInputs {
        cfg,
        publisher,
        registry: &sess.session_registry,
        store: &sess.session_store,
        messenger: &sess.agent_messenger,
        checkpoints: shared.checkpoints,
        tracing_service: &shared.hub.tracing_service,
        tracing_client_context: &infra.tracing_client_context,
        session_budget: &shared.hub.session_budget,
    });
    (infra, session_runtime)
}

pub(crate) async fn initialize(
    cfg: &Config,
    hub: &HubConfig,
    publisher: &residuum_bus::Publisher,
    shared: &HubServices,
) -> Result<GatewayComponents, FatalError> {
    let (layout, tz, checkpoints) =
        init_workspace_and_checkpoints(cfg, hub, publisher, shared).await?;

    let mut degradations: Vec<String> = Vec::new();

    let (identity, http) = init_identity_and_http(&layout, cfg).await?;
    let providers = providers::init_providers(cfg, tz, http.clone(), publisher, &mut degradations)?;
    let embedding = providers.embedding_provider.as_ref();
    let mem = memory::init_memory(cfg, &layout, embedding, &shared.team_wiki).await?;
    let subconscious = Subconscious::build(cfg, &layout, http.clone(), publisher.clone());

    let sess = init_session_subsystems(SessionSubsystemInputs {
        cfg,
        team: TeamLink::new(&cfg.agent_name, Arc::clone(&shared.team_router)),
        layout: &layout,
        tz,
        publisher,
        http: http.clone(),
        reflector: providers.reflector,
        mem: &mem,
        embedding_provider: providers.embedding_provider.clone(),
        degradations: &mut degradations,
    })
    .await;
    let (infra, session_runtime) = init_infra_and_session_runtime(
        cfg,
        hub,
        &layout,
        &mut degradations,
        publisher,
        &sess,
        &SharedServices::new(&checkpoints, shared),
    )
    .await;

    let main = build_spawn_context_and_agent(AgentInitInputs {
        cfg,
        hub_dir: &hub.config_dir,
        layout: &layout,
        tz,
        http_client: http.clone(),
        mem: &mem,
        infra: &infra,
        session_runtime: &session_runtime,
        session_registry: &sess.session_registry,
        session_observer: &sess.session_observer,
        merge_writer: &sess.merge_writer,
        action_store: &sess.action_store,
        action_notify: &sess.action_notify,
        skill_state: &sess.skill_state,
        publisher,
        agent_messenger: &sess.agent_messenger,
        tracing_service: &shared.tracing_service,
        build: shared.control.build,
        checkpoints: &checkpoints,
        directory: &shared.directory,
        mailboxes: &shared.mailboxes,
        identity,
        provider: providers.provider,
        options: providers.options,
        degradations: &mut degradations,
    })
    .await;

    publish_degradation_notice(publisher, &degradations).await;

    Ok(GatewayComponents {
        layout,
        tz,
        agent: main.agent,
        tool_injections: main.tool_injections,
        observer: providers.observer,
        merge_writer: sess.merge_writer,
        subconscious,
        action_store: sess.action_store,
        action_notify: sess.action_notify,
        mcp_registry: infra.net.mcp_registry,
        tools_path: infra.net.tools_path,
        agent_keys: infra.net.agent_keys,
        skill_state: sess.skill_state,
        hybrid_searcher: mem.hybrid_searcher,
        pulse_enabled: cfg.pulse_enabled,
        endpoint_registry: infra.net.endpoint_registry,
        channel_configs: infra.net.channel_configs,
        http_client: http.clone(),
        session_runtime,
        session_registry: sess.session_registry,
        session_store: sess.session_store,
        agent_messenger: sess.agent_messenger,
        conversation_router: sess.conversation_router,
        spawn_context: main.spawn_context,
        lifecycle: main.lifecycle,
        path_policy: infra.path_policy,
        auto_mode: infra.auto_mode,
        team: shared.team.clone(),
        output_topic_override_tx: main.output_topic_override_tx,
        a2a_hub: infra.a2a_hub,
        a2a_tracker: infra.a2a_tracker,
        checkpoints,
        config_reload_tracker: infra.config_reload_tracker,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(name: &str) -> residuum_config::StandaloneBackendConfig {
        residuum_config::StandaloneBackendConfig {
            name: name.to_string(),
            api_key: "key".to_string(),
            base_url: None,
        }
    }

    fn config_with_backend(name: Option<&str>) -> Config {
        let mut cfg = crate::testing::test_config();
        cfg.web_search.standalone_backend = name.map(backend);
        cfg
    }

    fn workspace_entry(name: &str) -> residuum_mcp::types::McpServerEntry {
        residuum_mcp::types::McpServerEntry {
            name: name.to_string(),
            command: "residuum-test-no-such-command".to_string(),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            transport: residuum_mcp::types::McpTransport::Stdio,
            headers: std::collections::BTreeMap::new(),
            timeout_secs: None,
        }
    }

    fn names(servers: &[residuum_mcp::types::McpServerEntry]) -> Vec<&str> {
        servers.iter().map(|s| s.name.as_str()).collect()
    }

    fn tool(name: &str) -> residuum_inference::ToolDefinition {
        residuum_inference::ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn mcp_reload_list_adds_the_configured_web_search_server() {
        let cfg = config_with_backend(Some("tavily"));
        let desired = with_web_search_mcp_entry(vec![workspace_entry("fs")], &cfg);
        assert_eq!(names(&desired), ["fs", "tavily_web_search"]);
    }

    #[test]
    fn mcp_reload_list_is_unchanged_without_a_server_backed_web_search() {
        for backend in [None, Some("ollama")] {
            let cfg = config_with_backend(backend);
            let desired = with_web_search_mcp_entry(vec![workspace_entry("fs")], &cfg);
            assert_eq!(names(&desired), ["fs"], "backend {backend:?}");
        }
    }

    #[test]
    fn mcp_reload_list_keeps_a_workspace_server_of_the_same_name() {
        let cfg = config_with_backend(Some("tavily"));
        let desired = with_web_search_mcp_entry(vec![workspace_entry("tavily_web_search")], &cfg);
        assert_eq!(desired.len(), 1);
        assert_eq!(
            desired.first().map(|s| s.command.as_str()),
            Some("residuum-test-no-such-command")
        );
    }

    #[tokio::test]
    async fn mcp_reload_reconcile_leaves_the_web_search_server_running() {
        let cfg = config_with_backend(Some("tavily"));
        let mut registry = residuum_mcp::McpRegistry::new();
        registry.insert_running_server("tavily_web_search", vec![tool("tavily_search")]);
        registry.insert_running_server("old", vec![tool("old_tool")]);

        let desired = with_web_search_mcp_entry(vec![workspace_entry("added")], &cfg);
        let report = registry.reconcile_and_connect(&desired).await;

        assert_eq!(report.stopped, 1, "only the removed mcp.json server stops");
        assert_eq!(
            report.failures.len(),
            1,
            "the added mcp.json server is attempted"
        );
        let tools: Vec<String> = registry
            .tool_definitions()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tools, ["tavily_search"]);
    }

    #[test]
    fn degradation_notice_is_none_when_nothing_degraded() {
        assert_eq!(degradation_notice(&[]), None);
    }

    #[test]
    fn degradation_notice_names_a_single_degradation_without_pluralizing() {
        let message =
            degradation_notice(&["skills couldn't be scanned and started empty: boom".to_string()])
                .expect("one degradation should produce a notice");
        assert_eq!(
            message,
            "Residuum started with 1 thing degraded: skills couldn't be scanned and started \
             empty: boom."
        );
    }

    #[test]
    fn degradation_notice_lists_every_degradation_and_pluralizes() {
        let message = degradation_notice(&[
            "skills couldn't be scanned and started empty: boom".to_string(),
            "the embedding provider is unavailable, so semantic search is disabled: kaboom"
                .to_string(),
        ])
        .expect("degradations should produce a notice");
        assert!(
            message.starts_with("Residuum started with 2 things degraded: "),
            "should pluralize and count every degradation: {message}"
        );
        assert!(message.contains("skills couldn't be scanned"));
        assert!(message.contains("embedding provider is unavailable"));
    }

    struct GatedObserverProvider {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for GatedObserverProvider {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            self.entered.notify_waiters();
            self.release.notified().await;
            Ok(residuum_inference::InferenceResponse::new(
                r#"{"observations": [{"content": "recovered after startup", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#
                    .to_string(),
                vec![],
            ))
        }

        fn model_name(&self) -> &'static str {
            "gated-observer"
        }
    }

    #[tokio::test]
    async fn session_recovery_does_not_block_startup_when_observer_is_slow() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = crate::testing::test_config();
        cfg.workspace_dir = dir.path().to_path_buf();
        let layout = WorkspaceLayout::new(&cfg.workspace_dir);
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        tokio::fs::create_dir_all(layout.episodes_dir())
            .await
            .unwrap();

        let seed_store = SessionStore::new(layout.sessions_dir());
        let info = residuum_bus::session::SessionInfo {
            address: residuum_bus::SessionAddress::from("scheduled-presence-test"),
            run_id: "run-startup-nonblocking".to_string(),
            category: residuum_bus::session::SessionCategory::Scheduled,
            trigger: residuum_bus::EventTrigger::Pulse,
            source_label: "pulse:presence".to_string(),
            state: residuum_bus::session::SessionState::Running,
            spawner: None,
            depth: 1,
            purpose: "presence check".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: chrono::Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        seed_store.begin_run(&info).await;
        seed_store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[
                    residuum_inference::Message::user("check presence"),
                    residuum_inference::Message::assistant("a".repeat(9000), None),
                ],
            )
            .await;

        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let observer = Arc::new(Observer::new(
            Box::new(GatedObserverProvider {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
            residuum_memory::observer::ObserverConfig::default(),
        ));
        let search_index = Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let merge_writer = Arc::new(MemoryMergeWriter::new(
            residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC),
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let bus = residuum_bus::spawn_broker();
        let publisher = bus.publisher();

        let (_registry, store, _messenger, _router) = residuum_core::testing::wait::guarded(
            "init_session_registry_and_messenger to return without waiting for observer",
            init_session_registry_and_messenger(
                &cfg,
                &layout,
                &publisher,
                &observer,
                &merge_writer,
                TeamLink::alone("test-agent"),
            ),
        )
        .await;

        residuum_core::testing::wait::guarded(
            "background recovery task to reach the observer call",
            entered.notified(),
        )
        .await;

        release.notify_waiters();
        residuum_core::testing::wait::until(
            "the recovered run record to be marked completed",
            || async {
                let (record, _transcript) = store.read_run(&info.run_id).await.ok()??;
                (record.state == "completed").then_some(())
            },
        )
        .await;
    }
}

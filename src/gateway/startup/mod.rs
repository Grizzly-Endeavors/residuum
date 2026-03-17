//! Gateway initialization: builds all subsystems before the event loop starts.

mod memory;
mod providers;
mod tools;

pub use providers::init_providers;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::BackgroundTaskSpawner;
use crate::background::types::BackgroundResult;
use crate::bus::EndpointRegistry;
use crate::config::Config;
use crate::error::ResiduumError;
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::memory::search::{HybridSearcher, MemoryIndex};
use crate::models::{EmbeddingProvider, SharedHttpClient};
use crate::notify::channels::InboxChannel;
use crate::projects::activation::{ProjectState, SharedProjectState};
use crate::projects::scanner::ProjectIndex;
use crate::skills::{SharedSkillState, SkillIndex, SkillState};
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
    pub reflector: Reflector,
    pub search_index: Arc<MemoryIndex>,
    pub vector_store: Option<Arc<crate::memory::vector_store::VectorStore>>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub project_state: SharedProjectState,
    pub skill_state: SharedSkillState,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub pulse_enabled: bool,
    pub endpoint_registry: EndpointRegistry,
    pub channel_configs: Vec<crate::notify::types::ExternalChannelConfig>,
    pub http_client: SharedHttpClient,
    pub background_spawner: Arc<BackgroundTaskSpawner>,
    pub background_result_rx: mpsc::Receiver<BackgroundResult>,
    pub spawn_context: Arc<SpawnContext>,
    pub path_policy: crate::tools::SharedPathPolicy,
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
}

/// Bootstrap the workspace directory and return the layout and timezone.
///
/// # Errors
/// Returns `ResiduumError` if workspace bootstrapping fails.
pub(super) async fn init_workspace(
    cfg: &Config,
) -> Result<(WorkspaceLayout, chrono_tz::Tz), ResiduumError> {
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    let tz = cfg.timezone;
    ensure_workspace(&layout, cfg.name.as_deref(), Some(cfg.timezone.name())).await?;

    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        ResiduumError::Config(format!(
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
/// Returns `ResiduumError` if identity loading or HTTP client construction fails.
pub(super) async fn init_identity_and_http(
    layout: &WorkspaceLayout,
    cfg: &Config,
) -> Result<(IdentityFiles, SharedHttpClient), ResiduumError> {
    let identity = IdentityFiles::load(layout).await?;
    let http = SharedHttpClient::new(&crate::models::HttpClientConfig::with_timeout(
        cfg.timeout_secs,
    ))
    .map_err(|e| ResiduumError::Config(format!("failed to build HTTP client: {e}")))?;
    Ok((identity, http))
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

/// Scan for projects and skills and return their shared state handles.
async fn init_project_and_skills(
    cfg: &Config,
    layout: &WorkspaceLayout,
) -> (SharedProjectState, SharedSkillState) {
    let project_index = match ProjectIndex::scan(layout).await {
        Ok(idx) => idx,
        Err(err) => {
            tracing::warn!(error = %err, "project index degraded: starting empty");
            ProjectIndex::default()
        }
    };
    let project_state: SharedProjectState = Arc::new(tokio::sync::Mutex::new(ProjectState::new(
        project_index,
        layout.clone(),
    )));
    let skill_index = match SkillIndex::scan(&cfg.skills.dirs, None).await {
        Ok(idx) => idx,
        Err(err) => {
            tracing::warn!(error = %err, "skill index degraded: starting empty");
            SkillIndex::default()
        }
    };
    let skill_state: SharedSkillState =
        SkillState::new_shared(skill_index, cfg.skills.dirs.clone());
    (project_state, skill_state)
}

/// Create the background task spawner and its result channel.
fn init_background_spawner(
    cfg: &Config,
    layout: &WorkspaceLayout,
) -> (mpsc::Receiver<BackgroundResult>, Arc<BackgroundTaskSpawner>) {
    let (bg_result_tx, bg_result_rx) = mpsc::channel::<BackgroundResult>(32);
    let background_spawner = Arc::new(BackgroundTaskSpawner::new(
        bg_result_tx,
        cfg.background.max_concurrent,
        layout.root().to_path_buf(),
        layout.background_dir(),
    ));
    (bg_result_rx, background_spawner)
}

/// Load and connect workspace MCP servers.
async fn init_mcp_servers(layout: &WorkspaceLayout) -> SharedMcpRegistry {
    let mcp_registry = crate::mcp::McpRegistry::new_shared();
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

/// Connect standalone web search MCP servers (Brave/Tavily) if configured.
async fn connect_web_search_mcp(cfg: &Config, mcp_registry: &SharedMcpRegistry) {
    let Some(backend) = &cfg.web_search.standalone_backend else {
        return;
    };

    let entry = match backend.name.as_str() {
        "brave" => crate::projects::types::McpServerEntry {
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
            transport: crate::projects::types::McpTransport::Stdio,
            headers: std::collections::HashMap::new(),
        },
        "tavily" => crate::projects::types::McpServerEntry {
            name: "tavily_web_search".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "tavily-mcp".to_string()],
            env: std::collections::HashMap::from([(
                "TAVILY_API_KEY".to_string(),
                backend.api_key.clone(),
            )]),
            transport: crate::projects::types::McpTransport::Stdio,
            headers: std::collections::HashMap::new(),
        },
        // Ollama uses a native tool, not MCP
        _ => return,
    };

    let report = mcp_registry.write().await.connect_servers(&[entry]).await;
    tracing::info!(
        backend = %backend.name,
        started = report.started,
        failures = report.failures.len(),
        "web search MCP server loaded"
    );
    if !report.failures.is_empty() {
        for (name, err) in &report.failures {
            tracing::warn!(server = %name, error = %err, "failed to start web search MCP server");
        }
    }
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

/// Spawn notify subscribers for each configured channel and the inbox.
///
/// Each channel subscribes to its `TopicId::Notify(name)` topic on the bus.
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
        match bus_handle.subscribe_typed(topic).await {
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
    let inbox_channel = InboxChannel::new(layout.inbox_dir(), tz);
    match bus_handle.subscribe_typed(topics::Inbox).await {
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

/// Initialize all gateway subsystems from config.
///
/// Delegates to `init_workspace`, `init_identity_and_http`, `providers::init_providers`,
/// and `memory::init_memory` for the first stages, then wires up tools, the agent,
/// and remaining subsystems.
///
/// # Errors
/// Returns `ResiduumError` if any subsystem fails to initialize.
pub(crate) async fn initialize(
    cfg: &Config,
    publisher: &crate::bus::Publisher,
) -> Result<GatewayComponents, ResiduumError> {
    let (layout, tz) = init_workspace(cfg).await?;
    let (identity, http) = init_identity_and_http(&layout, cfg).await?;
    let providers = providers::init_providers(cfg, tz, http.clone())?;
    let mem = memory::init_memory(cfg, &layout, providers.embedding_provider.as_ref()).await?;

    let (action_store, action_notify) = init_action_store(&layout).await;
    let (project_state, skill_state) = init_project_and_skills(cfg, &layout).await;

    let (bg_result_rx, background_spawner) = init_background_spawner(cfg, &layout);

    let mcp_registry = init_mcp_servers(&layout).await;
    connect_web_search_mcp(cfg, &mcp_registry).await;
    let (channel_configs, endpoint_registry) = init_channels_and_registry(&layout, cfg);

    let spawn_context = Arc::new(SpawnContext {
        background_config: cfg.background.clone(),
        main_provider_specs: cfg.main.clone(),
        http_client: http.clone(),
        max_tokens: cfg.max_tokens,
        retry_config: cfg.retry.clone(),
        identity: identity.clone(),
        options: crate::models::CompletionOptions {
            max_tokens: Some(cfg.max_tokens),
            temperature: cfg.temperature,
            thinking: cfg.thinking.clone(),
            ..crate::models::CompletionOptions::default()
        },
        layout: layout.clone(),
        tz,
        role_overrides: cfg.role_overrides.clone(),
        background_spawner: Arc::clone(&background_spawner),
        endpoint_registry: endpoint_registry.clone(),
        publisher: publisher.clone(),
        action_store: Arc::clone(&action_store),
        action_notify: Arc::clone(&action_notify),
        hybrid_searcher: Arc::clone(&mem.hybrid_searcher),
    });

    let tool_deps = ToolRegistryDeps {
        action_store: &action_store,
        action_notify: &action_notify,
        project_state: &project_state,
        skill_state: &skill_state,
        mcp_registry: &mcp_registry,
        background_spawner: &background_spawner,
        endpoint_registry: &endpoint_registry,
        publisher,
    };
    let (tools, tool_filter, path_policy_for_runtime, output_topic_override_tx) =
        tools::init_tool_registry(cfg, &layout, &mem, tz, &tool_deps);

    let agent = tools::create_agent(
        cfg,
        CreateAgentArgs {
            provider: providers.provider,
            tools,
            tool_filter,
            identity,
        },
        &mcp_registry,
        tz,
        &layout,
    )
    .await;

    Ok(GatewayComponents {
        layout,
        tz,
        agent,
        observer: providers.observer,
        reflector: providers.reflector,
        search_index: mem.search_index,
        vector_store: mem.vector_store,
        action_store,
        action_notify,
        mcp_registry,
        project_state,
        skill_state,
        embedding_provider: providers.embedding_provider,
        hybrid_searcher: mem.hybrid_searcher,
        pulse_enabled: cfg.pulse_enabled,
        endpoint_registry,
        channel_configs,
        http_client: http.clone(),
        background_spawner,
        background_result_rx: bg_result_rx,
        spawn_context,
        path_policy: path_policy_for_runtime,
        output_topic_override_tx,
    })
}

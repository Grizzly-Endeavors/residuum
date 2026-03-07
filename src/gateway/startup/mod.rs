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
use crate::config::Config;
use crate::error::ResiduumError;
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::models::{EmbeddingProvider, SharedHttpClient};
use crate::notify::channels::InboxChannel;
use crate::notify::router::NotificationRouter;
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
    pub pulse_enabled: bool,
    pub notification_router: Arc<NotificationRouter>,
    pub http_client: SharedHttpClient,
    pub background_spawner: Arc<BackgroundTaskSpawner>,
    pub background_result_rx: mpsc::Receiver<BackgroundResult>,
    pub spawn_context: Arc<SpawnContext>,
    pub path_policy: crate::tools::SharedPathPolicy,
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
            eprintln!(
                "warning: failed to load scheduled actions, starting with empty store: {err}"
            );
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
            eprintln!("warning: failed to scan projects, starting with empty index: {err}");
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
            eprintln!("warning: failed to scan skills, starting with empty index: {err}");
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
            }
        }
        Err(err) => {
            eprintln!(
                "warning: failed to load mcp.json, starting without workspace MCP servers: {err}"
            );
            tracing::warn!(error = %err, "workspace MCP servers degraded");
        }
    }
    mcp_registry
}

/// Load notification channels from workspace config and build the router.
fn init_notification_channels(
    layout: &WorkspaceLayout,
    http: &SharedHttpClient,
    cfg: &Config,
) -> (Arc<NotificationRouter>, std::collections::HashSet<String>) {
    let channel_configs = match crate::workspace::config::load_channel_configs(
        &layout.channels_toml(),
    ) {
        Ok(configs) => configs,
        Err(err) => {
            eprintln!(
                "warning: failed to load channels.toml, starting without external channels: {err}"
            );
            tracing::warn!(error = %err, "workspace channels degraded");
            Vec::new()
        }
    };
    let valid_external_channels: std::collections::HashSet<String> =
        channel_configs.iter().map(|c| c.name.clone()).collect();
    let external_channels =
        crate::workspace::config::build_external_channels(&channel_configs, http.client());
    let inbox_channel = InboxChannel::new(layout.inbox_dir(), cfg.timezone);
    let notification_router = Arc::new(NotificationRouter::new(
        external_channels,
        Some(inbox_channel),
    ));
    (notification_router, valid_external_channels)
}

/// Initialize all gateway subsystems from config.
///
/// Delegates to `init_workspace`, `init_identity_and_http`, `providers::init_providers`,
/// and `memory::init_memory` for the first stages, then wires up tools, the agent,
/// and remaining subsystems.
///
/// # Errors
/// Returns `ResiduumError` if any subsystem fails to initialize.
pub(crate) async fn initialize(cfg: &Config) -> Result<GatewayComponents, ResiduumError> {
    let (layout, tz) = init_workspace(cfg).await?;
    let (identity, http) = init_identity_and_http(&layout, cfg).await?;
    let providers = providers::init_providers(cfg, tz, http.clone())?;
    let mem = memory::init_memory(cfg, &layout, providers.embedding_provider.as_ref()).await?;

    let (action_store, action_notify) = init_action_store(&layout).await;
    let (project_state, skill_state) = init_project_and_skills(cfg, &layout).await;

    let (bg_result_rx, background_spawner) = init_background_spawner(cfg, &layout);
    let http_for_channels = http.clone();
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
    });

    let mcp_registry = init_mcp_servers(&layout).await;
    let (notification_router, valid_external_channels) =
        init_notification_channels(&layout, &http_for_channels, cfg);

    let tool_deps = ToolRegistryDeps {
        action_store: &action_store,
        action_notify: &action_notify,
        valid_external_channels: &valid_external_channels,
        project_state: &project_state,
        skill_state: &skill_state,
        mcp_registry: &mcp_registry,
        background_spawner: &background_spawner,
        spawn_context: &spawn_context,
        notification_router: &notification_router,
    };
    let (tools, tool_filter, path_policy_for_runtime) =
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
        pulse_enabled: cfg.pulse_enabled,
        notification_router,
        http_client: http_for_channels,
        background_spawner,
        background_result_rx: bg_result_rx,
        spawn_context,
        path_policy: path_policy_for_runtime,
    })
}

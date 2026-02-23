//! Gateway initialization: builds all subsystems before the event loop starts.

use std::sync::Arc;

use crate::agent::Agent;
use crate::config::Config;
use crate::cron::store::CronStore;
use crate::error::IronclawError;
use crate::memory::observer::Observer;
use crate::memory::recent_messages::load_messages_for_agent;
use crate::memory::reflector::Reflector;
use crate::memory::search::{MemoryIndex, create_shared_index};
use crate::models::{
    CompletionOptions, HttpClientConfig, ModelProvider, SharedHttpClient,
    build_provider_from_provider_spec,
};
use crate::projects::activation::{ProjectState, SharedProjectState};
use crate::projects::scanner::ProjectIndex;
use crate::skills::{SharedSkillState, SkillIndex, SkillState};
use crate::tools::ToolRegistry;
use crate::workspace::bootstrap::ensure_workspace;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::memory::build_memory_components;

/// All subsystems initialized before the gateway event loop.
pub(super) struct GatewayComponents {
    pub(super) layout: WorkspaceLayout,
    pub(super) tz: chrono_tz::Tz,
    pub(super) agent: Agent,
    pub(super) observer: Observer,
    pub(super) reflector: Reflector,
    pub(super) search_index: Arc<MemoryIndex>,
    pub(super) cron_store: Arc<tokio::sync::Mutex<CronStore>>,
    pub(super) cron_notify: Arc<tokio::sync::Notify>,
    pub(super) project_state: SharedProjectState,
    pub(super) skill_state: SharedSkillState,
    pub(super) pulse_provider: Box<dyn ModelProvider>,
    pub(super) cron_provider: Box<dyn ModelProvider>,
    pub(super) pulse_enabled: bool,
    pub(super) cron_enabled: bool,
}

/// Initialize all gateway subsystems from config.
///
/// Bootstraps the workspace, builds model providers, memory components,
/// search index, cron/project/skill state, tool registry, and agent.
///
/// # Errors
/// Returns `IronclawError` if any subsystem fails to initialize.
pub(super) async fn initialize(cfg: &Config) -> Result<GatewayComponents, IronclawError> {
    // Workspace
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    let tz = cfg.timezone;
    ensure_workspace(&layout).await?;

    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        IronclawError::Config(format!(
            "failed to change to workspace directory {}: {e}",
            cfg.workspace_dir.display()
        ))
    })?;
    tracing::info!(workspace = %cfg.workspace_dir.display(), "changed to workspace directory");

    // Identity + HTTP client
    let identity = IdentityFiles::load(&layout).await?;
    let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(cfg.timeout_secs))
        .map_err(|e| IronclawError::Config(format!("failed to build HTTP client: {e}")))?;

    // Model providers
    let provider = build_provider_from_provider_spec(&cfg.main, cfg.max_tokens, http.clone())?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    let (observer, reflector) = build_memory_components(cfg, tz, http.clone())?;
    let pulse_provider =
        build_provider_from_provider_spec(&cfg.pulse, cfg.max_tokens, http.clone())?;
    let cron_provider = build_provider_from_provider_spec(&cfg.cron, cfg.max_tokens, http)?;

    // Search index
    let search_index = create_shared_index(&layout.search_index_dir())?;
    match search_index.rebuild(&layout.memory_dir()) {
        Ok(count) => tracing::info!(indexed = count, "search index rebuilt"),
        Err(e) => eprintln!("warning: failed to rebuild search index: {e}"),
    }

    // Cron store
    let cron_store = Arc::new(tokio::sync::Mutex::new(
        CronStore::load(layout.cron_jobs_json()).await?,
    ));
    let cron_notify = Arc::new(tokio::sync::Notify::new());

    // Project + skill state
    let project_index = ProjectIndex::scan(&layout).await?;
    let project_state: SharedProjectState = Arc::new(tokio::sync::Mutex::new(ProjectState::new(
        project_index,
        layout.clone(),
    )));
    let skill_index = SkillIndex::scan(&cfg.skills.dirs, None).await?;
    let skill_state: SharedSkillState =
        SkillState::new_shared(skill_index, cfg.skills.dirs.clone());

    // Tool registry
    let path_policy = crate::tools::PathPolicy::new_shared(layout.root().to_path_buf());
    let tool_filter =
        crate::tools::ToolFilter::new_shared(std::collections::HashSet::from(["exec"]));
    let mcp_registry = crate::mcp::McpRegistry::new_shared();
    let mut tools = ToolRegistry::new();
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker, Arc::clone(&path_policy));
    tools.register_search_tool(Arc::clone(&search_index));
    tools.register_cron_tools(Arc::clone(&cron_store), Arc::clone(&cron_notify), tz);
    tools.register_project_tools(
        Arc::clone(&project_state),
        path_policy,
        Arc::clone(&tool_filter),
        mcp_registry,
        Arc::clone(&skill_state),
        tz,
    );
    tools.register_skill_tools(Arc::clone(&skill_state));

    // Agent
    let options = CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
    };
    let mut agent = Agent::new(provider, tools, tool_filter, identity, options, tz);
    agent.reload_observations(&layout).await?;
    agent.reload_recent_context(&layout).await?;

    let restore = load_messages_for_agent(&layout.recent_messages_json()).await?;
    if !restore.messages.is_empty() {
        tracing::info!(
            count = restore.messages.len(),
            "restoring recent messages from previous run"
        );
        agent.restore_messages(restore.messages);
    }
    agent.set_last_user_message_at(restore.last_user_message_at);

    Ok(GatewayComponents {
        layout,
        tz,
        agent,
        observer,
        reflector,
        search_index,
        cron_store,
        cron_notify,
        project_state,
        skill_state,
        pulse_provider,
        cron_provider,
        pulse_enabled: cfg.pulse_enabled,
        cron_enabled: cfg.cron_enabled,
    })
}

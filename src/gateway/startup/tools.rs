//! Tool registry and agent initialization.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::agent::{Agent, AgentConfig};
use crate::background::BackgroundTaskSpawner;
use crate::config::Config;
use crate::mcp::SharedMcpRegistry;
use crate::memory::recent_messages::load_messages_for_agent;

use crate::bus::EndpointRegistry;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::memory::MemoryComponents;

/// Shared subsystem handles needed for tool registration.
pub(super) struct ToolRegistryDeps<'a> {
    pub action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: &'a Arc<tokio::sync::Notify>,
    pub valid_external_channels: &'a std::collections::HashSet<String>,
    pub project_state: &'a SharedProjectState,
    pub skill_state: &'a SharedSkillState,
    pub mcp_registry: &'a SharedMcpRegistry,
    pub background_spawner: &'a Arc<BackgroundTaskSpawner>,
    pub endpoint_registry: &'a EndpointRegistry,
    pub publisher: &'a crate::bus::Publisher,
}

/// Arguments for creating the agent, bundled to stay under the argument limit.
pub(super) struct CreateAgentArgs {
    pub provider: Box<dyn crate::models::ModelProvider>,
    pub tools: ToolRegistry,
    pub tool_filter: crate::tools::SharedToolFilter,
    pub identity: IdentityFiles,
}

/// Build the tool registry with all default and domain-specific tools.
pub(super) fn init_tool_registry(
    cfg: &Config,
    layout: &WorkspaceLayout,
    mem: &MemoryComponents,
    tz: chrono_tz::Tz,
    deps: &ToolRegistryDeps<'_>,
) -> (
    ToolRegistry,
    crate::tools::SharedToolFilter,
    crate::tools::SharedPathPolicy,
) {
    let mut blocked_paths: Vec<std::path::PathBuf> = vec![
        cfg.config_dir.join("config.toml"),
        cfg.config_dir.join("config.example.toml"),
        cfg.config_dir.join("providers.toml"),
        cfg.config_dir.join("providers.example.toml"),
    ];
    if !cfg.agent.modify_mcp {
        blocked_paths.push(layout.mcp_json());
    }
    if !cfg.agent.modify_channels {
        blocked_paths.push(layout.channels_toml());
    }
    let blocked: std::collections::HashSet<std::path::PathBuf> =
        blocked_paths.into_iter().collect();
    let path_policy =
        crate::tools::PathPolicy::new_shared_with_blocked(layout.root().to_path_buf(), blocked);
    let tool_filter = crate::tools::ToolFilter::new_shared(std::collections::HashSet::new());
    let mut tools = ToolRegistry::new();
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker, Arc::clone(&path_policy));
    tools.register_search_tool(Arc::clone(&mem.hybrid_searcher));
    tools.register_memory_get_tool(layout.episodes_dir());
    tools.register_action_tools(
        Arc::clone(deps.action_store),
        Arc::clone(deps.action_notify),
        tz,
        deps.valid_external_channels.clone(),
    );
    let path_policy_for_runtime = Arc::clone(&path_policy);
    tools.register_project_tools(
        Arc::clone(deps.project_state),
        path_policy,
        Arc::clone(&tool_filter),
        Arc::clone(deps.mcp_registry),
        Arc::clone(deps.skill_state),
        tz,
    );
    tools.register_skill_tools(Arc::clone(deps.skill_state));
    tools.register_inbox_tools(layout.inbox_dir(), layout.inbox_archive_dir(), tz);
    tools.register_background_tools(Arc::clone(deps.background_spawner));
    tools.register_spawn_tool(
        deps.publisher.clone(),
        deps.valid_external_channels.clone(),
        layout.subagents_dir(),
    );

    tools.register_send_message_tool(deps.endpoint_registry.clone(), layout.inbox_dir(), tz);
    tools.register_web_fetch_tool();

    // Register Ollama Cloud web search tool if configured
    if let Some(backend) = &cfg.web_search.standalone_backend
        && backend.name == "ollama"
    {
        let base_url = backend
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.ollama.com".to_string());
        tools.register_ollama_web_search_tool(backend.api_key.clone(), base_url);
        tracing::info!("registered ollama_web_search tool");
    }

    (tools, tool_filter, path_policy_for_runtime)
}

/// Create the agent, load observations, recent context, and restore messages.
pub(super) async fn create_agent(
    cfg: &Config,
    args: CreateAgentArgs,
    mcp_registry: &SharedMcpRegistry,
    tz: chrono_tz::Tz,
    layout: &WorkspaceLayout,
) -> Agent {
    let web_search =
        cfg.web_search
            .provider_native
            .as_ref()
            .map(|pn| crate::models::WebSearchNativeConfig {
                max_uses: pn.max_uses,
                allowed_domains: pn.allowed_domains.clone(),
                blocked_domains: pn.blocked_domains.clone(),
                search_context_size: pn.search_context_size.clone(),
                exclude_domains: pn.exclude_domains.clone(),
            });
    let mut options = cfg.completion_options_for_role("main");
    options.web_search = web_search;
    let mut agent = Agent::new(
        args.provider,
        args.tools,
        args.tool_filter,
        Arc::clone(mcp_registry),
        args.identity,
        AgentConfig {
            options,
            tz,
            inbox_dir: layout.inbox_dir(),
        },
    );
    if let Err(err) = agent.reload_observations(layout).await {
        tracing::warn!(error = %err, "observation loading degraded");
    }
    if let Err(err) = agent.reload_recent_context(layout).await {
        tracing::warn!(error = %err, "recent context loading degraded");
    }

    match load_messages_for_agent(&layout.recent_messages_json()).await {
        Ok(restore) => {
            if !restore.messages.is_empty() {
                tracing::info!(
                    count = restore.messages.len(),
                    "restoring recent messages from previous run"
                );
                agent.restore_messages(restore.messages);
            }
            agent.set_last_user_message_at(restore.last_user_message_at);
        }
        Err(err) => {
            tracing::warn!(error = %err, "message restore degraded: starting with empty history");
        }
    }

    agent
}

//! Tool registry and agent initialization.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::agent::{Agent, AgentConfig};
use crate::background::messaging::AgentMessenger;
use crate::background::registry::{MAIN_ADDRESS, MAIN_DEPTH, SessionRegistry};
use crate::config::Config;
use crate::mcp::SharedMcpRegistry;
use crate::memory::recent_messages::load_messages_for_agent;

use crate::bus::{EndpointRegistry, SessionAddress};
use crate::skills::SharedSkillState;
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::memory::MemoryComponents;

/// Shared subsystem handles needed for tool registration.
pub(super) struct ToolRegistryDeps<'a> {
    pub action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: &'a Arc<tokio::sync::Notify>,
    pub skill_state: &'a SharedSkillState,
    pub tools_path: &'a crate::tools::SharedToolsPath,
    pub session_registry: &'a Arc<SessionRegistry>,
    pub endpoint_registry: &'a EndpointRegistry,
    pub publisher: &'a crate::bus::Publisher,
    pub tracing_service: &'a Arc<crate::tracing_service::TracingService>,
    pub tracing_client_context: &'a Arc<crate::tracing_service::ClientContext>,
    pub agent_messenger: &'a Arc<AgentMessenger>,
    /// Main's current-turn hop counter, shared with the `Agent` these tools
    /// end up registered against (see `CreateAgentArgs::hop_counter`).
    pub hop_counter: &'a crate::agent::HopCounter,
}

/// Arguments for creating the agent, bundled to stay under the argument limit.
pub(super) struct CreateAgentArgs {
    pub provider: Box<dyn crate::inference::InferenceProvider>,
    pub options: crate::inference::CompletionOptions,
    pub tools: ToolRegistry,
    pub identity: IdentityFiles,
    /// Main's current-turn hop counter — the same instance already handed to
    /// the `message_agent`/`subagent_spawn` tools in `args.tools` at
    /// registration time, so the agent and its tools always agree on the
    /// current turn's hop count.
    pub hop_counter: crate::agent::HopCounter,
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
    crate::tools::SharedPathPolicy,
    tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
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
    tracing::debug!(blocked_paths = ?blocked, "path policy configured");
    let path_policy = crate::tools::PathPolicy::new_shared_with_blocked(blocked);
    let mut tools = ToolRegistry::new();
    tools.set_tools_path(Arc::clone(deps.tools_path));
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker, Arc::clone(&path_policy));
    tools.register_search_tool(Arc::clone(&mem.hybrid_searcher));
    tools.register_memory_get_tool(layout.episodes_dir(), layout.sessions_dir());
    tools.register_action_tools(
        Arc::clone(deps.action_store),
        Arc::clone(deps.action_notify),
        tz,
    );
    let path_policy_for_runtime = Arc::clone(&path_policy);
    tools.register_skill_tools(Arc::clone(deps.skill_state));
    tools.register_inbox_tools(
        layout.agent_inbox_dir(),
        layout.agent_inbox_archive_dir(),
        layout.user_inbox_dir(),
        layout.user_inbox_attachments_dir(),
        tz,
    );
    tools.register_background_tools(Arc::clone(deps.session_registry));
    tools.register_spawn_tool(
        deps.publisher.clone(),
        Arc::clone(deps.skill_state),
        SessionAddress::from(MAIN_ADDRESS),
        MAIN_DEPTH,
        cfg.background.subagent_depth_cap,
        deps.hop_counter.clone(),
    );

    tools.register_send_message_tool(
        deps.endpoint_registry.clone(),
        deps.publisher.clone(),
        false,
    );
    tools.register_list_endpoints_tool(deps.endpoint_registry.clone());
    tools.register_message_agent_tool(
        SessionAddress::from(MAIN_ADDRESS),
        MAIN_ADDRESS.to_string(),
        Arc::clone(deps.agent_messenger),
        deps.hop_counter.clone(),
    );

    let override_tx = tokio::sync::watch::Sender::new(None);
    let override_tx_for_runtime = override_tx.clone();
    tools.register_switch_endpoint_tool(
        deps.endpoint_registry.clone(),
        override_tx,
        deps.publisher.clone(),
    );

    tools.register_web_fetch_tool();

    tools.register_feedback_tools(
        Arc::clone(deps.tracing_service),
        Arc::clone(deps.tracing_client_context),
        Arc::clone(deps.session_registry),
    );

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

    (tools, path_policy_for_runtime, override_tx_for_runtime)
}

/// Create the agent, load observations, recent context, and restore messages.
pub(super) async fn create_agent(
    args: CreateAgentArgs,
    mcp_registry: &SharedMcpRegistry,
    tz: chrono_tz::Tz,
    layout: &WorkspaceLayout,
) -> Agent {
    let mut agent = Agent::new(
        args.provider,
        args.tools,
        Arc::clone(mcp_registry),
        args.identity,
        AgentConfig {
            options: args.options,
            tz,
            layout: Some(layout.clone()),
        },
        args.hop_counter,
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

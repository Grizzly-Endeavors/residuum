//! Building each session's tool registry.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::a2a::{A2aClientHub, RemoteTaskTracker};
use crate::actions::store::ActionStore;
use crate::agent::{HopCounter, ToolRegistry};
use crate::agent_keys::SharedAgentKeys;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::bus::{ConversationTarget, EndpointRegistry, EventTrigger, SessionAddress};
use crate::config::{SharedModelCatalog, SharedToolsPath};
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;

use super::background;
use super::registration::{self, RegistryInjections};
use super::{SharedFileTracker, SharedPathPolicy};

/// Dependencies for [`build_subagent_registry`].
///
/// Grouped into a struct because a session's tool registry needs every
/// dependency the main agent's does. `own_address` and `own_depth` are this
/// session's own address and depth, and `depth_cap` the configured nesting
/// limit — together they let this session's own `subagent_spawn` record the
/// right spawner/depth on anything it forks and refuse spawning once the cap
/// is reached. `own_address` is reused (cloned) as the identity
/// `message_agent` reports to the agents it messages, alongside
/// `session_category` and `messenger`. `hop_counter` is this session's
/// current-turn hop counter, shared with `message_agent`/`subagent_spawn` so
/// they compute outgoing hop counts from the same value the session runtime
/// updates. `tracing_service` and `tracing_client_context` back this
/// session's own `file_bug_report`/`submit_feedback` tools, the same as
/// main's. `web_search_backend` mirrors main's
/// `cfg.web_search.standalone_backend`: `ollama_web_search` is registered
/// only when it names the `"ollama"` backend, exactly like
/// `gateway::startup::tools::init_tool_registry`. `trigger` and
/// `conversation_target` describe what started this session (and, for a
/// conversation-triggered one, which endpoint/conversation it replies to);
/// `conversation_target` gates `a2a_task_update`, registered only when its
/// endpoint is `"a2a"` (see `SESSION_ONLY_TOOLS` in
/// `gateway::startup::tools`) — `trigger` itself isn't read by any tool yet,
/// but is carried through for one that needs it later.
pub struct SubagentToolDeps {
    pub tracker: SharedFileTracker,
    /// The main agent's write policy, shared so a session is blocked from
    /// the same paths and picks up the same config reloads.
    pub path_policy: SharedPathPolicy,
    /// The main agent's live tool `PATH`, so a session's `exec` resolves the
    /// same binaries.
    pub tools_path: SharedToolsPath,
    /// The shared agent key store.
    pub agent_keys: SharedAgentKeys,
    pub skill_state: SharedSkillState,
    pub tz: chrono_tz::Tz,
    pub hybrid_searcher: Arc<HybridSearcher>,
    /// The workspace root, for tools that need to validate a caller-supplied
    /// relative path against it (currently just `a2a_task_update`'s
    /// artifacts), and for `write_file`/`edit_file` to recognize
    /// `config/channels.toml`, `config/mcp.json`, `config/a2a.json`,
    /// `HEARTBEAT.yml`, and skill `SKILL.md` files for diagnostics.
    pub workspace_dir: PathBuf,
    /// The agent's own `config/` directory (`<agent>/config`), for
    /// `write_file`/`edit_file` to recognize `config.toml`/`providers.toml`
    /// for diagnostics.
    pub config_dir: PathBuf,
    /// The hub's directory (`~/.residuum/hub`), needed to resolve the hub
    /// config those diagnostics run against.
    pub hub_dir: PathBuf,
    pub episodes_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub agent_inbox_dir: PathBuf,
    pub agent_inbox_archive_dir: PathBuf,
    pub user_inbox_dir: PathBuf,
    pub user_inbox_attachments_dir: PathBuf,
    pub session_registry: Arc<SessionRegistry>,
    pub endpoint_registry: EndpointRegistry,
    pub publisher: crate::bus::Publisher,
    pub action_store: Arc<Mutex<ActionStore>>,
    pub action_notify: Arc<Notify>,
    /// This session's own address, recorded as the spawner on anything it
    /// forks in turn, and reused as the identity `message_agent` reports.
    pub own_address: SessionAddress,
    /// This session's own depth from the main agent (main is depth 0).
    pub own_depth: u32,
    /// Maximum depth a `subagent_spawn`-created session may have.
    pub depth_cap: u32,
    /// This session's category, for `message_agent` to report alongside
    /// `own_address`.
    pub session_category: String,
    /// What triggered this session. Not consumed by any tool yet — carried
    /// through so a future session-only tool can tell what started it.
    pub trigger: EventTrigger,
    /// The conversation this session replies to, for a conversation-triggered
    /// session. `None` for every other trigger. Not consumed by any tool
    /// yet — carried through so a future session-only tool can tell which
    /// endpoint and conversation it runs against.
    pub conversation_target: Option<ConversationTarget>,
    pub messenger: Arc<AgentMessenger>,
    pub hop_counter: HopCounter,
    pub tracing_service: Arc<crate::tracing_service::TracingService>,
    pub tracing_client_context: Arc<crate::tracing_service::ClientContext>,
    /// Standalone web search backend config, if one is configured — mirrors
    /// `cfg.web_search.standalone_backend`.
    pub web_search_backend: Option<crate::config::StandaloneBackendConfig>,
    /// Remote A2A agents this instance's client can reach, shared with main.
    pub a2a_hub: Arc<A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents, shared with
    /// main.
    pub a2a_tracker: Arc<RemoteTaskTracker>,
    /// Workspace and config checkpoint repositories, shared with main —
    /// backs `workspace_history`/`workspace_restore`.
    pub checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    /// How this session's `agent_create`/`agent_delete` reach the hub.
    pub lifecycle: super::LifecycleAccess,
    /// The live catalog this session's `subagent_spawn` and
    /// `schedule_action` check an explicit model against, shared with main.
    pub model_catalog: SharedModelCatalog,
}

/// Build a tool registry for a session.
///
/// Includes every tool available to the main agent, with the same config
/// gating, except `switch_endpoint` — the one tool that stays main-only,
/// because it redirects main's background-turn output and is meaningless
/// for a session. `tests::session_registry_matches_main_minus_documented_allowlist`
/// enforces this: it builds both registries from equivalent config and
/// asserts the session registry's tool names equal main's minus that one
/// documented exclusion, so a tool added to one registration surface but
/// not the other fails the build instead of drifting silently.
///
/// See [`SubagentToolDeps`] for what each field means.
#[must_use]
pub fn build_subagent_registry(deps: &SubagentToolDeps) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.set_agent_keys(Arc::clone(&deps.agent_keys));
    let injections = RegistryInjections {
        tools_path: Some(Arc::clone(&deps.tools_path)),
        checkpoints: Some(Arc::clone(&deps.checkpoints)),
        publisher: Some(deps.publisher.clone()),
    };

    register_session_workspace_tools(&mut registry, &injections, deps);
    register_session_agent_tools(&mut registry, deps);
    register_session_messaging_tools(&mut registry, deps);
    register_session_service_tools(&mut registry, deps);
    registry
}

/// A session's file, key, skill, memory, inbox, and checkpoint-history
/// tools.
fn register_session_workspace_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    deps: &SubagentToolDeps,
) {
    // Core I/O tools. `None`: a session never gets a `ConfigWriteWatch`
    // — see its doc comment.
    let diagnostics_paths = crate::diagnostics_dispatch::DiagnosticsPaths {
        config_dir: deps.config_dir.clone(),
        workspace_dir: deps.workspace_dir.clone(),
        hub_dir: deps.hub_dir.clone(),
    };
    registration::register_defaults(
        tools,
        injections,
        Arc::clone(&deps.tracker),
        Arc::clone(&deps.path_policy),
        diagnostics_paths,
        None,
    );
    registration::register_agent_key_tools(
        tools,
        injections,
        Arc::clone(&deps.agent_keys),
        Arc::clone(&deps.checkpoints),
    );
    registration::register_skill_tools(tools, Arc::clone(&deps.skill_state));
    registration::register_memory_tools(
        tools,
        Arc::clone(&deps.hybrid_searcher),
        deps.episodes_dir.clone(),
        deps.sessions_dir.clone(),
    );
    registration::register_inbox_tools(
        tools,
        injections,
        deps.agent_inbox_dir.clone(),
        deps.agent_inbox_archive_dir.clone(),
        deps.user_inbox_dir.clone(),
        deps.user_inbox_attachments_dir.clone(),
        deps.tz,
    );
    // Workspace checkpoint history (workspace repository only)
    registration::register_workspace_checkpoint_tools(
        tools,
        Arc::clone(&deps.checkpoints),
        Arc::clone(&deps.path_policy),
    );
}

/// A session's tools for managing other sessions and teammates:
/// `stop_agent`, `list_agents`, `subagent_spawn`, `a2a_task_update`, and
/// the teammate lifecycle tools.
fn register_session_agent_tools(tools: &mut ToolRegistry, deps: &SubagentToolDeps) {
    registration::register_background_tools(
        tools,
        Arc::clone(&deps.session_registry),
        deps.own_address.clone(),
        Arc::clone(&deps.a2a_hub),
        Arc::clone(&deps.a2a_tracker),
        deps.messenger.team().clone(),
    );
    registration::register_spawn_tool(
        tools,
        background::SpawnToolDeps {
            publisher: deps.publisher.clone(),
            skill_state: Arc::clone(&deps.skill_state),
            spawner_address: deps.own_address.clone(),
            depth: deps.own_depth,
            depth_cap: deps.depth_cap,
            hop_counter: deps.hop_counter.clone(),
            models: Arc::clone(&deps.model_catalog),
        },
    );
    registration::register_a2a_task_update_tool(
        tools,
        deps.conversation_target.as_ref(),
        deps.own_address.clone(),
        deps.publisher.clone(),
        deps.workspace_dir.clone(),
        Arc::clone(&deps.path_policy),
    );
    registration::register_agent_lifecycle_tools(
        tools,
        deps.lifecycle.clone(),
        deps.hop_counter.clone(),
    );
}

/// A session's messaging tools.
fn register_session_messaging_tools(tools: &mut ToolRegistry, deps: &SubagentToolDeps) {
    registration::register_send_message_tool(
        tools,
        deps.endpoint_registry.clone(),
        deps.publisher.clone(),
        true,
    );
    registration::register_list_endpoints_tool(tools, deps.endpoint_registry.clone());
    registration::register_message_agent_tool(
        tools,
        deps.own_address.clone(),
        deps.session_category.clone(),
        Arc::clone(&deps.messenger),
        deps.hop_counter.clone(),
        Arc::clone(&deps.a2a_hub),
        Arc::clone(&deps.a2a_tracker),
    );
}

/// A session's feedback, web, and action-scheduling tools.
fn register_session_service_tools(tools: &mut ToolRegistry, deps: &SubagentToolDeps) {
    registration::register_feedback_tools(
        tools,
        Arc::clone(&deps.tracing_service),
        Arc::clone(&deps.tracing_client_context),
        Arc::clone(&deps.session_registry),
    );
    registration::register_web_fetch_tool(tools);
    registration::register_action_tools(
        tools,
        Arc::clone(&deps.action_store),
        Arc::clone(&deps.action_notify),
        Arc::clone(&deps.model_catalog),
        deps.tz,
    );
    registration::register_ollama_web_search_tool_if_configured(
        tools,
        deps.web_search_backend.as_ref(),
    );
}

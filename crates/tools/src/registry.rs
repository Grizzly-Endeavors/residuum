//! Building each session's tool registry.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use residuum_a2a::{A2aClientHub, RemoteTaskTracker};
use residuum_agent::ToolRegistry;
use residuum_agent::auto_mode::SharedAutoMode;
use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_background::session_tools::{SessionToolFactory, SessionToolScope};
use residuum_bus::EndpointRegistry;
use residuum_config::agent_keys::SharedAgentKeys;
use residuum_config::{SharedModelCatalog, SharedToolsPath, WorkspaceLayout};
use residuum_memory::search::HybridSearcher;
use residuum_proactivity::actions::store::ActionStore;

use super::background;
use super::registration::{self, RegistryInjections};
use super::{FileTracker, SharedPathPolicy};

/// The session tool factory: the shared handles every session's tools hold,
/// from which [`SessionToolFactory::build`] makes one session run's
/// registry.
///
/// The gateway builds it beside `SpawnContext`, at startup and on every
/// config reload, from the same handles main's tools hold, so a session sees
/// the same live state as main. `tracing_service` and
/// `tracing_client_context` back the session's own
/// `file_bug_report`/`submit_feedback` tools, the same as main's.
pub struct SessionTools {
    /// The main agent's write policy, shared so a session is blocked from
    /// the same paths and picks up the same config reloads.
    pub path_policy: SharedPathPolicy,
    /// The main agent's live tool `PATH`, so a session's `exec` resolves the
    /// same binaries.
    pub tools_path: SharedToolsPath,
    /// The shared agent key store.
    pub agent_keys: SharedAgentKeys,
    /// Workspace and config checkpoint repositories, shared with main —
    /// backs `workspace_history`/`workspace_restore`.
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub publisher: residuum_bus::Publisher,
    pub endpoint_registry: EndpointRegistry,
    pub session_registry: Arc<SessionRegistry>,
    pub action_store: Arc<Mutex<ActionStore>>,
    pub action_notify: Arc<Notify>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub tracing_service: Arc<residuum_tracing_service::TracingService>,
    pub tracing_client_context: Arc<residuum_tracing_service::ClientContext>,
    /// The running build, for the feedback tools' client context.
    pub build: residuum_core::build_info::BuildInfo,
    /// Standalone web search backend config, if one is configured — mirrors
    /// main's `cfg.web_search.standalone_backend`: `ollama_web_search` is
    /// registered only when it names the `"ollama"` backend, exactly like
    /// `main_tools::init_tool_registry`.
    pub web_search_backend: Option<residuum_config::StandaloneBackendConfig>,
    /// The live catalog a session's `subagent_spawn` and `schedule_action`
    /// check an explicit model against, shared with main.
    pub model_catalog: SharedModelCatalog,
    /// Remote A2A agents this instance's client can reach, shared with main.
    pub a2a_hub: Arc<A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents, shared with
    /// main.
    pub a2a_tracker: Arc<RemoteTaskTracker>,
    /// How a session's `agent_create`/`agent_delete` reach the hub, acting as
    /// this agent.
    pub lifecycle: super::LifecycleAccess,
    /// Shared agent-messaging service, for the session's `message_agent`.
    pub messenger: Arc<AgentMessenger>,
    /// The workspace layout. Its root is what tools validate a
    /// caller-supplied relative path against (`a2a_task_update`'s
    /// artifacts), and what `write_file`/`edit_file` recognize
    /// `config/channels.toml`, `config/mcp.json`, `config/a2a.json`,
    /// `HEARTBEAT.yml`, and skill `SKILL.md` files under for diagnostics; it
    /// also names the memory and inbox directories.
    pub layout: WorkspaceLayout,
    /// The agent's own `config/` directory (`<agent>/config`), for
    /// `write_file`/`edit_file` to recognize `config.toml`/`providers.toml`
    /// for diagnostics.
    pub config_dir: PathBuf,
    /// The hub's directory (`~/.residuum/hub`), needed to resolve the hub
    /// config those diagnostics run against.
    pub hub_dir: PathBuf,
    /// Timezone used by inbox and action-scheduling tools.
    pub tz: chrono_tz::Tz,
    /// Maximum depth a `subagent_spawn`-created session may have; a session
    /// at it has its own `subagent_spawn` refuse.
    pub depth_cap: u32,
    /// The main agent's Auto Mode, shared so session tool calls are checked
    /// against the same live rules. `None` in tests that need no checks.
    pub auto_mode: Option<SharedAutoMode>,
}

impl SessionToolFactory for SessionTools {
    /// Build a tool registry for a session.
    ///
    /// Includes every tool available to the main agent, with the same config
    /// gating, except `switch_endpoint` — the one tool that stays main-only,
    /// because it redirects main's background-turn output and is meaningless
    /// for a session. The parity test `session_registry_matches_main_minus_documented_allowlist`
    /// in `crates/tools/src/main_tools.rs`'s tests enforces this: it builds both
    /// registries from equivalent config and asserts the session registry's
    /// tool names equal main's minus that one documented exclusion, so a tool
    /// added to one registration surface but not the other fails the build
    /// instead of drifting silently.
    ///
    /// `scope.conversation_target` gates `a2a_task_update`, registered only
    /// when its endpoint is `"a2a"` (see `SESSION_ONLY_TOOLS` in the same
    /// test module). No tool reads `scope.trigger`.
    fn build(&self, scope: SessionToolScope) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.set_agent_keys(Arc::clone(&self.agent_keys));
        let injections = RegistryInjections {
            tools_path: Some(Arc::clone(&self.tools_path)),
            checkpoints: Some(Arc::clone(&self.checkpoints)),
            publisher: Some(self.publisher.clone()),
        };
        register_session_workspace_tools(&mut registry, &injections, self, &scope);
        register_session_agent_tools(&mut registry, self, &scope);
        register_session_messaging_tools(&mut registry, self, &scope);
        register_session_service_tools(&mut registry, self);
        if let Some(auto_mode) = &self.auto_mode {
            registry.set_auto_mode(Arc::clone(auto_mode));
        }
        registry
    }
}

/// A session's file, key, skill, memory, inbox, and checkpoint-history
/// tools.
fn register_session_workspace_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    shared: &SessionTools,
    scope: &SessionToolScope,
) {
    // Core I/O tools. `None`: a session never gets a `ConfigWriteWatch`
    // — see its doc comment.
    let diagnostics_paths = residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
        config_dir: shared.config_dir.clone(),
        workspace_dir: shared.layout.root().to_path_buf(),
        hub_dir: shared.hub_dir.clone(),
    };
    // Fresh per session run: tracks reads within this run only.
    registration::register_defaults(
        tools,
        injections,
        FileTracker::new_shared(),
        Arc::clone(&shared.path_policy),
        diagnostics_paths,
        None,
    );
    registration::register_agent_key_tools(
        tools,
        injections,
        Arc::clone(&shared.agent_keys),
        Arc::clone(&shared.checkpoints),
    );
    registration::register_skill_tools(tools, Arc::clone(&scope.skill_state));
    registration::register_memory_tools(
        tools,
        Arc::clone(&shared.hybrid_searcher),
        shared.layout.episodes_dir(),
        shared.layout.sessions_dir(),
    );
    registration::register_inbox_tools(
        tools,
        injections,
        shared.layout.agent_inbox_dir(),
        shared.layout.agent_inbox_archive_dir(),
        shared.layout.user_inbox_dir(),
        shared.tz,
    );
    // Workspace checkpoint history (workspace repository only)
    registration::register_workspace_checkpoint_tools(
        tools,
        Arc::clone(&shared.checkpoints),
        Arc::clone(&shared.path_policy),
    );
}

/// A session's tools for managing other sessions and teammates:
/// `stop_agent`, `list_agents`, `subagent_spawn`, `a2a_task_update`, and
/// the teammate lifecycle tools.
fn register_session_agent_tools(
    tools: &mut ToolRegistry,
    shared: &SessionTools,
    scope: &SessionToolScope,
) {
    registration::register_background_tools(
        tools,
        Arc::clone(&shared.session_registry),
        scope.own_address.clone(),
        Arc::clone(&shared.a2a_hub),
        Arc::clone(&shared.a2a_tracker),
        shared.messenger.team().clone(),
    );
    registration::register_spawn_tool(
        tools,
        background::SpawnToolDeps {
            publisher: shared.publisher.clone(),
            skill_state: Arc::clone(&scope.skill_state),
            spawner_address: scope.own_address.clone(),
            depth: scope.own_depth,
            depth_cap: shared.depth_cap,
            hop_counter: scope.hop_counter.clone(),
            models: Arc::clone(&shared.model_catalog),
        },
    );
    registration::register_a2a_task_update_tool(
        tools,
        scope.conversation_target.as_ref(),
        scope.own_address.clone(),
        shared.publisher.clone(),
        shared.layout.root().to_path_buf(),
        Arc::clone(&shared.path_policy),
    );
    registration::register_agent_lifecycle_tools(
        tools,
        shared.lifecycle.clone(),
        scope.hop_counter.clone(),
    );
}

/// A session's messaging tools.
fn register_session_messaging_tools(
    tools: &mut ToolRegistry,
    shared: &SessionTools,
    scope: &SessionToolScope,
) {
    registration::register_send_message_tool(
        tools,
        shared.endpoint_registry.clone(),
        shared.publisher.clone(),
        true,
    );
    registration::register_list_endpoints_tool(tools, shared.endpoint_registry.clone());
    registration::register_message_agent_tool(
        tools,
        scope.own_address.clone(),
        scope.category.as_str().to_string(),
        Arc::clone(&shared.messenger),
        scope.hop_counter.clone(),
        Arc::clone(&shared.a2a_hub),
        Arc::clone(&shared.a2a_tracker),
    );
}

/// A session's feedback, web, and action-scheduling tools.
fn register_session_service_tools(tools: &mut ToolRegistry, shared: &SessionTools) {
    registration::register_feedback_tools(
        tools,
        shared.build,
        Arc::clone(&shared.tracing_service),
        Arc::clone(&shared.tracing_client_context),
        Arc::clone(&shared.session_registry),
    );
    registration::register_web_fetch_tool(tools);
    registration::register_action_tools(
        tools,
        Arc::clone(&shared.action_store),
        Arc::clone(&shared.action_notify),
        Arc::clone(&shared.model_catalog),
        shared.tz,
    );
    registration::register_ollama_web_search_tool_if_configured(
        tools,
        shared.web_search_backend.as_ref(),
    );
}

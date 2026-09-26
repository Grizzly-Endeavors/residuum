use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::a2a::{A2aClientHub, RemoteTaskTracker};
use crate::actions::store::ActionStore;
use crate::agent::HopCounter;
use crate::agent_keys::{Redactor, SharedAgentKeys};
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::bus::{ConversationTarget, EndpointRegistry, EventTrigger, Publisher, SessionAddress};
use crate::inference::ToolDefinition;
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;

use super::{
    SharedFileTracker, SharedPathPolicy, SharedToolsPath, Tool, ToolError, ToolResult,
    a2a_task_update, actions, agent_keys, background, edit, exec, file_bug_report, inbox,
    memory_get, memory_search, message_agent, ollama_web_search, read, send_message, skills,
    submit_feedback, web_fetch, workspace_checkpoints, write,
};

/// Registry of available tools.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// Effective `PATH` handle injected into the `exec` tool at registration.
    tools_path: Option<SharedToolsPath>,
    /// Agent key store injected into the `exec` tool at registration, and
    /// the source of the redactor applied to every tool result.
    agent_keys: Option<SharedAgentKeys>,
    /// Checkpoint engine injected into `exec` (for `store_output_as`) at
    /// registration. `None` means minting a key through `exec` isn't
    /// checkpointed (matches `agent_keys: None`'s "not available" story).
    checkpoints: Option<Arc<crate::checkpoints::CheckpointEngine>>,
    /// Bus publisher injected into `exec` and `agent_key_delete`, so they can
    /// surface a notice when they overwrite or delete a key the user
    /// created. `None` means that notice goes unpublished (the action still
    /// happens).
    publisher: Option<Publisher>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Dependencies for [`ToolRegistry::build_subagent_registry`].
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
    /// The app config directory (`~/.residuum/`), for `write_file`/
    /// `edit_file` to recognize `config.toml`/`providers.toml` for
    /// diagnostics.
    pub config_dir: PathBuf,
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
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            tools_path: None,
            agent_keys: None,
            checkpoints: None,
            publisher: None,
        }
    }

    /// Set the effective `PATH` handle injected into the `exec` tool.
    ///
    /// Call before [`register_defaults`](Self::register_defaults) so the `exec`
    /// tool prepends the configured tool directories to spawned commands.
    pub fn set_tools_path(&mut self, tools_path: SharedToolsPath) {
        self.tools_path = Some(tools_path);
    }

    /// Set the agent key store injected into the `exec` tool and used to
    /// redact tool results.
    ///
    /// Call before [`register_defaults`](Self::register_defaults) so the `exec`
    /// tool can expose and mint keys.
    pub fn set_agent_keys(&mut self, agent_keys: SharedAgentKeys) {
        self.agent_keys = Some(agent_keys);
    }

    /// Set the checkpoint engine injected into the `exec` tool, so minting a
    /// key through `store_output_as` checkpoints the config repo first.
    ///
    /// Call before [`register_defaults`](Self::register_defaults).
    pub fn set_checkpoints(&mut self, checkpoints: Arc<crate::checkpoints::CheckpointEngine>) {
        self.checkpoints = Some(checkpoints);
    }

    /// Set the bus publisher injected into `exec` and `agent_key_delete`, so
    /// overwriting or deleting a key the user created is reported.
    ///
    /// Call before [`register_defaults`](Self::register_defaults) and
    /// [`register_agent_key_tools`](Self::register_agent_key_tools).
    pub fn set_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    /// The redactor for every current agent-key value; empty when no key
    /// store is attached.
    pub async fn redactor(&self) -> Redactor {
        match &self.agent_keys {
            Some(keys) => keys.redactor().await,
            None => Redactor::default(),
        }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Remove a registered tool by name, if present.
    ///
    /// Used to reload a single conditionally-registered tool in place on a
    /// live registry (e.g. `ollama_web_search` after a config reload changes
    /// the standalone web search backend) without rebuilding the whole
    /// registry. Returns `true` if a tool was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.tools.len();
        self.tools.retain(|t| t.name() != name);
        self.tools.len() != before
    }

    /// Get tool definitions for sending to the model.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Names of all registered tools.
    ///
    /// Used to reserve the built-in tool namespace in the MCP registry so a
    /// colliding MCP tool is shadowed visibly rather than silently.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Execute a tool by name with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    #[tracing::instrument(skip_all, fields(tool.name = %name))]
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tracing::debug!("tool invocation");
        let result = tool.execute(arguments).await?;
        tracing::debug!(is_error = result.is_error, "tool result");
        Ok(result)
    }

    /// Execute a tool by name, racing it against turn-level cancellation.
    ///
    /// See [`Tool::execute_cancellable`] for what happens when `cancel`
    /// fires while the tool is running.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    #[tracing::instrument(skip_all, fields(tool.name = %name))]
    pub async fn execute_cancellable(
        &self,
        name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tracing::debug!("tool invocation");
        let result = tool.execute_cancellable(arguments, cancel).await?;
        tracing::debug!(is_error = result.is_error, "tool result");
        Ok(result)
    }

    /// Register the default set of tools (read, write, edit, exec).
    ///
    /// `diagnostics_paths` lets `write_file`/`edit_file` recognize the
    /// strictly-parsed files this instance validates (`config.toml`,
    /// `providers.toml`, `config/channels.toml`, `config/mcp.json`,
    /// `config/a2a.json`, `HEARTBEAT.yml`, skill `SKILL.md`) and append
    /// diagnostics to the tool result after a write. `config_watch` is
    /// `Some` only for main's own registry — see `ConfigWriteWatch`'s doc
    /// comment for why a session's registry never gets one — and lets
    /// `write_file`/`edit_file` recognize a write to
    /// `config.toml`/`providers.toml`/`mcp.json`/`channels.toml`/`a2a.json`/
    /// `HEARTBEAT.yml` so the reload it triggers can report back to the
    /// agent.
    pub fn register_defaults(
        &mut self,
        tracker: SharedFileTracker,
        policy: SharedPathPolicy,
        diagnostics_paths: crate::diagnostics::DiagnosticsPaths,
        config_watch: Option<super::config_reload_tracker::ConfigWriteWatch>,
    ) {
        self.register(Box::new(read::ReadTool::new(Arc::clone(&tracker))));
        let mut write_tool = write::WriteTool::new(
            Arc::clone(&tracker),
            Arc::clone(&policy),
            diagnostics_paths.clone(),
        );
        let mut edit_tool = edit::EditTool::new(tracker, policy, diagnostics_paths);
        if let Some(watch) = config_watch {
            write_tool = write_tool.with_config_watch(watch.clone());
            edit_tool = edit_tool.with_config_watch(watch);
        }
        self.register(Box::new(write_tool));
        self.register(Box::new(edit_tool));
        let mut exec_tool = exec::ExecTool::new(
            self.tools_path.clone(),
            self.agent_keys.clone(),
            self.checkpoints.clone(),
        );
        if let Some(publisher) = &self.publisher {
            exec_tool = exec_tool.with_publisher(publisher.clone());
        }
        self.register(Box::new(exec_tool));
    }

    /// Register agent key tools (`agent_keys_list`, `agent_key_delete`).
    ///
    /// `checkpoints` is checkpointed before a delete — including one on a
    /// key the user created — so it's always undoable.
    pub fn register_agent_key_tools(
        &mut self,
        keys: SharedAgentKeys,
        checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    ) {
        self.register(Box::new(agent_keys::AgentKeysListTool::new(Arc::clone(
            &keys,
        ))));
        let mut delete_tool = agent_keys::AgentKeyDeleteTool::new(keys, checkpoints);
        if let Some(publisher) = &self.publisher {
            delete_tool = delete_tool.with_publisher(publisher.clone());
        }
        self.register(Box::new(delete_tool));
    }

    /// Register the `memory_search` tool with a shared hybrid searcher.
    pub fn register_search_tool(&mut self, searcher: Arc<HybridSearcher>) {
        self.register(Box::new(memory_search::MemorySearchTool::new(searcher)));
    }

    /// Register the `memory_get` tool for episode and session-run transcript
    /// retrieval.
    pub fn register_memory_get_tool(&mut self, episodes_dir: PathBuf, sessions_dir: PathBuf) {
        self.register(Box::new(memory_get::MemoryGetTool::new(
            episodes_dir,
            sessions_dir,
        )));
    }

    /// Register skill management tools (`skill_activate`, `skill_deactivate`).
    pub fn register_skill_tools(&mut self, state: SharedSkillState) {
        self.register(Box::new(skills::SkillActivateTool::new(Arc::clone(&state))));
        self.register(Box::new(skills::SkillDeactivateTool::new(state)));
    }

    /// Register inbox management tools (`inbox_list`, `inbox_read`, `inbox_archive`, `user_inbox_add`).
    pub fn register_inbox_tools(
        &mut self,
        agent_inbox_dir: PathBuf,
        agent_archive_dir: PathBuf,
        user_inbox_dir: PathBuf,
        user_inbox_attachments_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(inbox::InboxListTool::new(agent_inbox_dir.clone())));
        self.register(Box::new(inbox::InboxReadTool::new(agent_inbox_dir.clone())));
        self.register(Box::new(inbox::InboxArchiveTool::new(
            agent_inbox_dir,
            agent_archive_dir,
        )));
        self.register(Box::new(inbox::UserInboxAddTool::new(
            user_inbox_dir,
            user_inbox_attachments_dir,
            tz,
        )));
    }

    /// Register `list_endpoints` and `list_conversations` for discovering
    /// where messages can go.
    pub fn register_list_endpoints_tool(&mut self, registry: EndpointRegistry) {
        self.register(Box::new(
            super::list_conversations::ListConversationsTool::new(registry.clone()),
        ));
        self.register(Box::new(super::list_endpoints::ListEndpointsTool::new(
            registry,
        )));
    }

    /// Register the `switch_endpoint` tool for changing the active output endpoint.
    pub fn register_switch_endpoint_tool(
        &mut self,
        registry: EndpointRegistry,
        override_tx: tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
        publisher: crate::bus::Publisher,
    ) {
        self.register(Box::new(super::switch_endpoint::SwitchEndpointTool::new(
            registry,
            override_tx,
            publisher,
        )));
    }

    /// Register the `send_message` tool for proactive message delivery.
    ///
    /// `restrict_owner_targets` is `true` for a session's registry: a session
    /// refuses the owner's DM on every chat interface and the web UI (only
    /// the main agent talks to the owner). Pass `false` for the main agent.
    pub fn register_send_message_tool(
        &mut self,
        registry: EndpointRegistry,
        publisher: crate::bus::Publisher,
        restrict_owner_targets: bool,
    ) {
        self.register(Box::new(send_message::SendMessageTool::new(
            registry,
            publisher,
            restrict_owner_targets,
        )));
    }

    /// Register session management tools (`stop_agent`, `list_agents`),
    /// identifying this registry's owner as `self_address` for the remote
    /// A2A task lookups both tools do (a caller's own open tasks).
    pub fn register_background_tools(
        &mut self,
        registry: Arc<SessionRegistry>,
        self_address: SessionAddress,
        a2a_hub: Arc<A2aClientHub>,
        a2a_tracker: Arc<RemoteTaskTracker>,
    ) {
        self.register(Box::new(background::StopAgentTool::new(
            Arc::clone(&registry),
            self_address.clone(),
            Arc::clone(&a2a_hub),
            Arc::clone(&a2a_tracker),
        )));
        self.register(Box::new(background::ListAgentsTool::new(
            registry,
            self_address,
            a2a_hub,
            a2a_tracker,
        )));
    }

    /// Register the `message_agent` tool, identifying this registry's owner
    /// as `self_address` (category `self_category`) to whoever it messages.
    pub fn register_message_agent_tool(
        &mut self,
        self_address: SessionAddress,
        self_category: String,
        messenger: Arc<AgentMessenger>,
        hop_counter: HopCounter,
        a2a_hub: Arc<A2aClientHub>,
        a2a_tracker: Arc<RemoteTaskTracker>,
    ) {
        self.register(Box::new(message_agent::MessageAgentTool::new(
            self_address,
            self_category,
            messenger,
            hop_counter,
            a2a_hub,
            a2a_tracker,
        )));
    }

    /// Register the `subagent_spawn` tool for on-demand sub-agent delegation.
    ///
    /// `spawner_address` and `depth` are the caller's own address and depth
    /// (main is `MAIN_ADDRESS`/`MAIN_DEPTH`; a session passes its own). A
    /// spawn is refused once `depth + 1` exceeds `depth_cap`.
    pub(crate) fn register_spawn_tool(
        &mut self,
        publisher: crate::bus::Publisher,
        skill_state: crate::skills::SharedSkillState,
        spawner_address: crate::bus::SessionAddress,
        depth: u32,
        depth_cap: u32,
        hop_counter: HopCounter,
    ) {
        self.register(Box::new(background::SubagentSpawnTool::new(
            publisher,
            skill_state,
            spawner_address,
            depth,
            depth_cap,
            hop_counter,
        )));
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
    pub fn build_subagent_registry(deps: SubagentToolDeps) -> Self {
        let SubagentToolDeps {
            tracker,
            path_policy,
            tools_path,
            agent_keys,
            skill_state,
            tz,
            hybrid_searcher,
            workspace_dir,
            config_dir,
            episodes_dir,
            sessions_dir,
            agent_inbox_dir,
            agent_inbox_archive_dir,
            user_inbox_dir,
            user_inbox_attachments_dir,
            session_registry,
            endpoint_registry,
            publisher,
            action_store,
            action_notify,
            own_address,
            own_depth,
            depth_cap,
            session_category,
            // Not read by any tool yet — reserved for one that needs it later
            // (see `SubagentToolDeps::trigger`).
            trigger: _trigger,
            conversation_target,
            messenger,
            hop_counter,
            tracing_service,
            tracing_client_context,
            web_search_backend,
            a2a_hub,
            a2a_tracker,
            checkpoints,
        } = deps;

        let mut registry = Self::new();
        registry.set_tools_path(tools_path);
        registry.set_agent_keys(Arc::clone(&agent_keys));
        registry.set_checkpoints(Arc::clone(&checkpoints));
        registry.set_publisher(publisher.clone());

        // Core I/O tools. `None`: a session never gets a `ConfigWriteWatch`
        // — see its doc comment.
        let diagnostics_paths = crate::diagnostics::DiagnosticsPaths {
            config_dir,
            workspace_dir: workspace_dir.clone(),
        };
        registry.register_defaults(tracker, path_policy, diagnostics_paths, None);
        registry.register_agent_key_tools(agent_keys, Arc::clone(&checkpoints));

        // Skill tools: activate, deactivate
        registry.register_skill_tools(Arc::clone(&skill_state));

        // Memory tools
        registry.register_search_tool(hybrid_searcher);
        registry.register_memory_get_tool(episodes_dir, sessions_dir);

        // Inbox tools
        registry.register_inbox_tools(
            agent_inbox_dir,
            agent_inbox_archive_dir,
            user_inbox_dir,
            user_inbox_attachments_dir,
            tz,
        );

        // Feedback tools (file_bug_report, submit_feedback)
        registry.register_feedback_tools(
            tracing_service,
            tracing_client_context,
            Arc::clone(&session_registry),
        );

        // Session management (stop_agent, list_agents, subagent_spawn)
        registry.register_background_tools(
            session_registry,
            own_address.clone(),
            Arc::clone(&a2a_hub),
            Arc::clone(&a2a_tracker),
        );
        registry.register_spawn_tool(
            publisher.clone(),
            skill_state,
            own_address.clone(),
            own_depth,
            depth_cap,
            hop_counter.clone(),
        );

        registry.register_a2a_task_update_tool(
            conversation_target.as_ref(),
            own_address.clone(),
            publisher.clone(),
            workspace_dir,
        );

        // Messaging tools
        registry.register_send_message_tool(endpoint_registry.clone(), publisher, true);
        registry.register_list_endpoints_tool(endpoint_registry);
        registry.register_message_agent_tool(
            own_address,
            session_category,
            messenger,
            hop_counter,
            a2a_hub,
            a2a_tracker,
        );

        // Web fetch
        registry.register_web_fetch_tool();

        // Workspace checkpoint history (workspace repository only)
        registry.register_workspace_checkpoint_tools(checkpoints);

        // Action scheduling tools
        registry.register_action_tools(action_store, action_notify, tz);

        registry.register_ollama_web_search_tool_if_configured(web_search_backend.as_ref());

        registry
    }

    /// Register `a2a_task_update` when `conversation_target` names the
    /// `a2a` endpoint — session-only (see `SESSION_ONLY_TOOLS` in
    /// `gateway::startup::tools`).
    fn register_a2a_task_update_tool(
        &mut self,
        conversation_target: Option<&ConversationTarget>,
        own_address: SessionAddress,
        publisher: crate::bus::Publisher,
        workspace_dir: PathBuf,
    ) {
        if conversation_target.is_some_and(|target| target.endpoint == "a2a") {
            self.register(Box::new(a2a_task_update::A2aTaskUpdateTool::new(
                own_address,
                publisher,
                workspace_dir,
            )));
        }
    }

    /// Register `ollama_web_search`, gated the same way as main's (see
    /// `gateway::startup::tools::init_tool_registry`): only when
    /// `backend` names the `"ollama"` standalone web search backend.
    fn register_ollama_web_search_tool_if_configured(
        &mut self,
        backend: Option<&crate::config::StandaloneBackendConfig>,
    ) {
        let Some(backend) = backend else { return };
        if backend.name != "ollama" {
            return;
        }
        let base_url = backend
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.ollama.com".to_string());
        self.register_ollama_web_search_tool(backend.api_key.clone(), base_url);
        tracing::info!("registered ollama_web_search tool for session");
    }

    /// Register the `web_fetch` tool for fetching web page content.
    pub fn register_web_fetch_tool(&mut self) {
        self.register(Box::new(web_fetch::WebFetchTool::new()));
    }

    /// Register the `workspace_history` and `workspace_restore` tools,
    /// scoped to the workspace checkpoint repository only.
    pub fn register_workspace_checkpoint_tools(
        &mut self,
        checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    ) {
        self.register(Box::new(workspace_checkpoints::WorkspaceHistoryTool::new(
            Arc::clone(&checkpoints),
        )));
        self.register(Box::new(workspace_checkpoints::WorkspaceRestoreTool::new(
            checkpoints,
        )));
    }

    /// Register the `file_bug_report` and `submit_feedback` tools.
    ///
    /// Both go through the shared `TracingService`. The bug-report tool
    /// also captures a snapshot of the runtime client context so each
    /// submission carries version/model/OS metadata, plus the live session
    /// registry so `active_subagents` reflects what's running right now.
    pub fn register_feedback_tools(
        &mut self,
        service: Arc<crate::tracing_service::TracingService>,
        client_context: Arc<crate::tracing_service::ClientContext>,
        session_registry: Arc<SessionRegistry>,
    ) {
        self.register(Box::new(file_bug_report::FileBugReportTool::new(
            Arc::clone(&service),
            client_context,
            session_registry,
        )));
        self.register(Box::new(submit_feedback::SubmitFeedbackTool::new(service)));
    }

    /// Register the `ollama_web_search` tool for Ollama Cloud web search.
    pub fn register_ollama_web_search_tool(&mut self, api_key: String, base_url: String) {
        self.register(Box::new(ollama_web_search::OllamaWebSearchTool::new(
            api_key, base_url,
        )));
    }

    /// Register action scheduling tools (`schedule_action`, `list_actions`, `cancel_action`).
    pub fn register_action_tools(
        &mut self,
        store: Arc<Mutex<ActionStore>>,
        notify: Arc<Notify>,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(actions::ScheduleActionTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(actions::ListActionsTool::new(
            Arc::clone(&store),
            tz,
        )));
        self.register(Box::new(actions::CancelActionTool::new(store, notify)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{FileTracker, PathPolicy};

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", Value::Null).await;
        assert!(result.is_err(), "should error on unknown tool");
        assert!(
            matches!(result.unwrap_err(), ToolError::NotFound(_)),
            "should be NotFound"
        );
    }

    #[test]
    fn registry_definitions_empty() {
        let registry = ToolRegistry::new();
        assert!(
            registry.definitions().is_empty(),
            "empty registry should have no definitions"
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared();
        registry.register_defaults(
            FileTracker::new_shared(),
            policy,
            crate::diagnostics::DiagnosticsPaths {
                config_dir: std::path::PathBuf::from("/tmp/residuum-test-config"),
                workspace_dir: std::path::PathBuf::from("/tmp/residuum-test-workspace"),
            },
            None,
        );
        let defs = registry.definitions();
        assert!(
            defs.iter().any(|d| d.name == "read_file"),
            "should have read_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "write_file"),
            "should have write_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "edit_file"),
            "should have edit_file tool"
        );
        assert!(
            defs.iter().any(|d| d.name == "exec"),
            "should have exec tool"
        );
    }
}

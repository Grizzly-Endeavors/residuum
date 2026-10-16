//! Registering the tools into a [`ToolRegistry`]: one function per group of
//! tools, which the main agent's and each session's registry builders call.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::a2a::{A2aClientHub, RemoteTaskTracker};
use crate::actions::store::ActionStore;
use crate::agent::{HopCounter, ToolRegistry};
use crate::agent_keys::SharedAgentKeys;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::background::team::TeamLink;
use crate::bus::{ConversationTarget, EndpointRegistry, Publisher, SessionAddress};
use crate::config::{SharedModelCatalog, SharedToolsPath, WorkspaceLayout};
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;

use super::{
    SharedFileTracker, SharedPathPolicy, a2a_task_update, actions, agent_keys, background, edit,
    exec, file_bug_report, inbox, memory_get, memory_search, message_agent, ollama_web_search,
    read, send_message, skills, submit_feedback, web_fetch, workspace_checkpoints, write,
};

/// Shared handles some tools take at registration, beyond their own
/// arguments. Each is optional, so a registry built without one still works:
/// the tools that would use it do without.
#[derive(Default)]
pub struct RegistryInjections {
    /// Effective `PATH` handle injected into the `exec` tool.
    pub tools_path: Option<SharedToolsPath>,
    /// Checkpoint engine injected into `exec` (for `store_output_as`).
    /// `None` means minting a key through `exec` isn't checkpointed first.
    pub checkpoints: Option<Arc<crate::checkpoints::CheckpointEngine>>,
    /// Bus publisher injected into `exec`, `agent_key_delete` and
    /// `user_inbox_add`, so they can surface a notice when they overwrite or
    /// delete a key the user created, and announce each user inbox item they
    /// save. `None` means those go unpublished (the action still happens).
    pub publisher: Option<Publisher>,
}

/// Re-register every tool that captured the user's timezone at
/// construction (`schedule_action`, `list_actions`, `user_inbox_add`)
/// with a new one, so a timezone edit reaches them without a restart.
pub fn reload_timezone_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
    models: SharedModelCatalog,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) {
    for name in ["schedule_action", "list_actions", "user_inbox_add"] {
        tools.remove(name);
    }
    tools.register(Box::new(actions::ScheduleActionTool::new(
        Arc::clone(&store),
        notify,
        tz,
        models,
    )));
    tools.register(Box::new(actions::ListActionsTool::new(store, tz)));
    register_user_inbox_add(
        tools,
        injections,
        layout.user_inbox_dir(),
        layout.user_inbox_attachments_dir(),
        tz,
    );
}

/// Reload the `ollama_web_search` tool in place from the current
/// standalone web search backend config (e.g. after a config reload).
///
/// Mirrors `tools::main_tools::init_tool_registry`'s startup-time
/// gating: removes any existing `ollama_web_search` tool, then re-adds
/// it with the current API key/base URL if `backend` names the
/// `"ollama"` backend. A no-op call (backend unchanged) still removes
/// and re-adds the tool — harmless, since the tool itself is stateless
/// beyond the credentials it's constructed with.
pub fn reload_ollama_web_search_tool(
    tools: &mut ToolRegistry,
    backend: Option<&crate::config::StandaloneBackendConfig>,
) {
    let was_registered = tools.remove("ollama_web_search");
    if let Some(backend) = backend
        && backend.name == "ollama"
    {
        let base_url = backend
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.ollama.com".to_string());
        register_ollama_web_search_tool(tools, backend.api_key.clone(), base_url);
        tracing::info!("reloaded ollama_web_search tool from config");
    } else if was_registered {
        tracing::info!("removed ollama_web_search tool: no longer configured");
    }
}

/// Register `user_inbox_add`, announcing each item it saves on the
/// injections' publisher when there is one.
fn register_user_inbox_add(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    user_inbox_dir: PathBuf,
    user_inbox_attachments_dir: PathBuf,
    tz: chrono_tz::Tz,
) {
    let tool = inbox::UserInboxAddTool::new(user_inbox_dir, user_inbox_attachments_dir, tz);
    let tool = match &injections.publisher {
        Some(publisher) => tool.with_publisher(publisher.clone()),
        None => tool,
    };
    tools.register(Box::new(tool));
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
///
/// `exec` takes the registry's agent key store, so attach it with
/// [`ToolRegistry::set_agent_keys`] first.
pub fn register_defaults(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    tracker: SharedFileTracker,
    policy: SharedPathPolicy,
    diagnostics_paths: crate::a2a::diagnostics_dispatch::DiagnosticsPaths,
    config_watch: Option<super::config_reload_tracker::ConfigWriteWatch>,
) {
    // Every file/shell tool resolves a relative path (or spawns a child
    // process) against this workspace root rather than the process's
    // current directory, which is shared across every agent hosted in
    // the same process.
    let workspace_root = diagnostics_paths.workspace_dir.clone();
    tools.register(Box::new(
        read::ReadTool::new(Arc::clone(&tracker), workspace_root.clone())
            .with_policy(Arc::clone(&policy)),
    ));
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
    tools.register(Box::new(write_tool));
    tools.register(Box::new(edit_tool));
    let mut exec_tool = exec::ExecTool::new(
        injections.tools_path.clone(),
        tools.agent_keys().cloned(),
        injections.checkpoints.clone(),
        workspace_root,
    );
    if let Some(publisher) = &injections.publisher {
        exec_tool = exec_tool.with_publisher(publisher.clone());
    }
    tools.register(Box::new(exec_tool));
}

/// Register agent key tools (`agent_keys_list`, `agent_key_delete`).
///
/// `checkpoints` is checkpointed before a delete — including one on a
/// key the user created — so it's always undoable.
pub fn register_agent_key_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    keys: SharedAgentKeys,
    checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
) {
    tools.register(Box::new(agent_keys::AgentKeysListTool::new(Arc::clone(
        &keys,
    ))));
    let mut delete_tool = agent_keys::AgentKeyDeleteTool::new(keys, checkpoints);
    if let Some(publisher) = &injections.publisher {
        delete_tool = delete_tool.with_publisher(publisher.clone());
    }
    tools.register(Box::new(delete_tool));
}

/// Register the memory tools: `memory_search` over a shared hybrid
/// searcher, and `memory_get` for episode and session-run transcript
/// retrieval.
pub fn register_memory_tools(
    tools: &mut ToolRegistry,
    searcher: Arc<HybridSearcher>,
    episodes_dir: PathBuf,
    sessions_dir: PathBuf,
) {
    tools.register(Box::new(memory_search::MemorySearchTool::new(searcher)));
    tools.register(Box::new(memory_get::MemoryGetTool::new(
        episodes_dir,
        sessions_dir,
    )));
}

/// Register skill management tools (`skill_activate`, `skill_deactivate`).
pub fn register_skill_tools(tools: &mut ToolRegistry, state: SharedSkillState) {
    tools.register(Box::new(skills::SkillActivateTool::new(Arc::clone(&state))));
    tools.register(Box::new(skills::SkillDeactivateTool::new(state)));
}

/// Register inbox management tools (`inbox_list`, `inbox_read`, `inbox_archive`,
/// `inbox_restore`, `user_inbox_add`).
///
/// `user_inbox_add` announces the items it saves on the injections'
/// publisher.
pub fn register_inbox_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    agent_inbox_dir: PathBuf,
    agent_archive_dir: PathBuf,
    user_inbox_dir: PathBuf,
    user_inbox_attachments_dir: PathBuf,
    tz: chrono_tz::Tz,
) {
    tools.register(Box::new(inbox::InboxListTool::new(
        agent_inbox_dir.clone(),
        agent_archive_dir.clone(),
    )));
    tools.register(Box::new(inbox::InboxReadTool::new(agent_inbox_dir.clone())));
    tools.register(Box::new(inbox::InboxArchiveTool::new(
        agent_inbox_dir.clone(),
        agent_archive_dir.clone(),
    )));
    tools.register(Box::new(inbox::InboxRestoreTool::new(
        agent_inbox_dir,
        agent_archive_dir,
    )));
    register_user_inbox_add(
        tools,
        injections,
        user_inbox_dir,
        user_inbox_attachments_dir,
        tz,
    );
}

/// Register `list_endpoints` and `list_conversations` for discovering
/// where messages can go.
pub fn register_list_endpoints_tool(tools: &mut ToolRegistry, registry: EndpointRegistry) {
    tools.register(Box::new(
        super::list_conversations::ListConversationsTool::new(registry.clone()),
    ));
    tools.register(Box::new(super::list_endpoints::ListEndpointsTool::new(
        registry,
    )));
}

/// Register the `switch_endpoint` tool for changing the active output endpoint.
pub fn register_switch_endpoint_tool(
    tools: &mut ToolRegistry,
    registry: EndpointRegistry,
    override_tx: tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
    publisher: Publisher,
) {
    tools.register(Box::new(super::switch_endpoint::SwitchEndpointTool::new(
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
    tools: &mut ToolRegistry,
    registry: EndpointRegistry,
    publisher: Publisher,
    restrict_owner_targets: bool,
) {
    tools.register(Box::new(send_message::SendMessageTool::new(
        registry,
        publisher,
        restrict_owner_targets,
    )));
}

/// Register session management tools (`stop_agent`, `list_agents`),
/// identifying this registry's owner as `self_address` for the remote
/// A2A task lookups both tools do (a caller's own open tasks). `team`
/// is the roster `list_agents` shows.
pub fn register_background_tools(
    tools: &mut ToolRegistry,
    registry: Arc<SessionRegistry>,
    self_address: SessionAddress,
    a2a_hub: Arc<A2aClientHub>,
    a2a_tracker: Arc<RemoteTaskTracker>,
    team: TeamLink,
) {
    tools.register(Box::new(background::StopAgentTool::new(
        Arc::clone(&registry),
        self_address.clone(),
        Arc::clone(&a2a_hub),
        Arc::clone(&a2a_tracker),
    )));
    tools.register(Box::new(background::ListAgentsTool::new(
        registry,
        self_address,
        a2a_hub,
        a2a_tracker,
        team,
    )));
}

/// Register the `message_agent` tool, identifying this registry's owner
/// as `self_address` (category `self_category`) to whoever it messages.
pub fn register_message_agent_tool(
    tools: &mut ToolRegistry,
    self_address: SessionAddress,
    self_category: String,
    messenger: Arc<AgentMessenger>,
    hop_counter: HopCounter,
    a2a_hub: Arc<A2aClientHub>,
    a2a_tracker: Arc<RemoteTaskTracker>,
) {
    tools.register(Box::new(message_agent::MessageAgentTool::new(
        self_address,
        self_category,
        messenger,
        hop_counter,
        a2a_hub,
        a2a_tracker,
    )));
}

/// Register `agent_create` and `agent_delete`, acting as the agent
/// `lifecycle` names.
pub fn register_agent_lifecycle_tools(
    tools: &mut ToolRegistry,
    lifecycle: super::LifecycleAccess,
    hop_counter: HopCounter,
) {
    tools.register(Box::new(super::agent_lifecycle::AgentCreateTool::new(
        lifecycle.clone(),
        hop_counter,
    )));
    tools.register(Box::new(super::agent_lifecycle::AgentDeleteTool::new(
        lifecycle,
    )));
}

/// Register the `subagent_spawn` tool for on-demand sub-agent delegation
/// (see [`background::SpawnToolDeps`]).
pub(crate) fn register_spawn_tool(tools: &mut ToolRegistry, deps: background::SpawnToolDeps) {
    tools.register(Box::new(background::SubagentSpawnTool::new(deps)));
}

/// Register `a2a_task_update` when `conversation_target` names the
/// `a2a` endpoint — session-only (see `SESSION_ONLY_TOOLS` in
/// `tools::main_tools`).
pub(crate) fn register_a2a_task_update_tool(
    tools: &mut ToolRegistry,
    conversation_target: Option<&ConversationTarget>,
    own_address: SessionAddress,
    publisher: Publisher,
    workspace_dir: PathBuf,
    policy: SharedPathPolicy,
) {
    if conversation_target.is_some_and(|target| target.endpoint == "a2a") {
        tools.register(Box::new(
            a2a_task_update::A2aTaskUpdateTool::new(own_address, publisher, workspace_dir)
                .with_policy(policy),
        ));
    }
}

/// Register `ollama_web_search`, gated the same way as main's (see
/// `tools::main_tools::init_tool_registry`): only when
/// `backend` names the `"ollama"` standalone web search backend.
pub(crate) fn register_ollama_web_search_tool_if_configured(
    tools: &mut ToolRegistry,
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
    register_ollama_web_search_tool(tools, backend.api_key.clone(), base_url);
    tracing::info!("registered ollama_web_search tool for session");
}

/// Register the `web_fetch` tool for fetching web page content.
pub fn register_web_fetch_tool(tools: &mut ToolRegistry) {
    tools.register(Box::new(web_fetch::WebFetchTool::new()));
}

/// Register the `workspace_history` and `workspace_restore` tools,
/// scoped to the workspace checkpoint repository only.
pub fn register_workspace_checkpoint_tools(
    tools: &mut ToolRegistry,
    checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    path_policy: SharedPathPolicy,
) {
    tools.register(Box::new(workspace_checkpoints::WorkspaceHistoryTool::new(
        Arc::clone(&checkpoints),
    )));
    tools.register(Box::new(workspace_checkpoints::WorkspaceRestoreTool::new(
        checkpoints,
        path_policy,
    )));
}

/// Register the `file_bug_report` and `submit_feedback` tools.
///
/// Both go through the shared `TracingService`. The bug-report tool
/// also captures a snapshot of the runtime client context so each
/// submission carries version/model/OS metadata, plus the live session
/// registry so `active_subagents` reflects what's running right now.
pub fn register_feedback_tools(
    tools: &mut ToolRegistry,
    build: crate::build_info::BuildInfo,
    service: Arc<crate::tracing_service::TracingService>,
    client_context: Arc<crate::tracing_service::ClientContext>,
    session_registry: Arc<SessionRegistry>,
) {
    tools.register(Box::new(file_bug_report::FileBugReportTool::new(
        Arc::clone(&service),
        client_context,
        session_registry,
    )));
    tools.register(Box::new(submit_feedback::SubmitFeedbackTool::new(
        service, build,
    )));
}

/// Register the `ollama_web_search` tool for Ollama Cloud web search.
pub fn register_ollama_web_search_tool(
    tools: &mut ToolRegistry,
    api_key: String,
    base_url: String,
) {
    tools.register(Box::new(ollama_web_search::OllamaWebSearchTool::new(
        api_key, base_url,
    )));
}

/// Register action scheduling tools (`schedule_action`, `list_actions`,
/// `cancel_action`). `models` is the live catalog `schedule_action` checks
/// an explicit model against.
pub fn register_action_tools(
    tools: &mut ToolRegistry,
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
    models: SharedModelCatalog,
    tz: chrono_tz::Tz,
) {
    tools.register(Box::new(actions::ScheduleActionTool::new(
        Arc::clone(&store),
        Arc::clone(&notify),
        tz,
        models,
    )));
    tools.register(Box::new(actions::ListActionsTool::new(
        Arc::clone(&store),
        tz,
    )));
    tools.register(Box::new(actions::CancelActionTool::new(store, notify)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::bus_wait;
    use crate::tools::{FileTracker, PathPolicy};

    #[test]
    fn reload_timezone_tools_replaces_without_duplicating() {
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            std::path::PathBuf::from("/tmp/residuum-test-actions.json"),
        )));
        let notify = Arc::new(Notify::new());
        let mut registry = ToolRegistry::new();
        let models = crate::config::ModelCatalog::default().into_shared();
        register_action_tools(
            &mut registry,
            Arc::clone(&store),
            Arc::clone(&notify),
            Arc::clone(&models),
            chrono_tz::UTC,
        );
        let layout = WorkspaceLayout::new("/tmp/residuum-test-workspace");
        for _ in 0..2 {
            reload_timezone_tools(
                &mut registry,
                &RegistryInjections::default(),
                Arc::clone(&store),
                Arc::clone(&notify),
                Arc::clone(&models),
                &layout,
                chrono_tz::America::New_York,
            );
        }
        let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
        for tool in ["schedule_action", "list_actions", "user_inbox_add"] {
            assert_eq!(
                names.iter().filter(|n| n.as_str() == tool).count(),
                1,
                "{tool} must be registered exactly once"
            );
        }
    }

    /// Adds an item through the registry's `user_inbox_add` and returns
    /// whether the registry's bus announced it.
    async fn adds_and_announces(registry: &ToolRegistry, bus: &crate::bus::BusHandle) -> bool {
        let mut added = bus
            .subscribe::<_, crate::bus::UserInboxAddedEvent>(crate::bus::topics::UserInbox)
            .await
            .unwrap();
        let result = registry
            .execute(
                "user_inbox_add",
                serde_json::json!({"title": "note", "body": "look"}),
            )
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.output);
        bus_wait::bus_barrier(bus).await;
        !added.drain().is_empty()
    }

    #[tokio::test]
    async fn user_inbox_add_announces_through_the_injected_publisher_and_after_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let inbox = layout.user_inbox_dir();
        let attachments = layout.user_inbox_attachments_dir();
        std::fs::create_dir_all(&inbox).unwrap();
        let bus = crate::bus::spawn_broker();
        let injections = RegistryInjections {
            publisher: Some(bus.publisher()),
            ..RegistryInjections::default()
        };
        let mut registry = ToolRegistry::new();
        register_inbox_tools(
            &mut registry,
            &injections,
            dir.path().join("inbox/agent"),
            dir.path().join("archive/inbox/agent"),
            inbox,
            attachments,
            chrono_tz::UTC,
        );
        assert!(adds_and_announces(&registry, &bus).await, "at registration");

        // A timezone change re-registers the tool, which must keep announcing.
        reload_timezone_tools(
            &mut registry,
            &injections,
            Arc::new(Mutex::new(ActionStore::new_empty(
                dir.path().join("scheduled_actions.json"),
            ))),
            Arc::new(Notify::new()),
            crate::config::ModelCatalog::default().into_shared(),
            &layout,
            chrono_tz::America::New_York,
        );
        assert!(adds_and_announces(&registry, &bus).await, "after a reload");
    }

    /// Registers the default tools with no injections, the way a session's
    /// registry does, with `workspace_dir` as the directory `exec` starts in.
    fn register_defaults_in(registry: &mut ToolRegistry, workspace_dir: PathBuf) {
        register_defaults(
            registry,
            &RegistryInjections::default(),
            FileTracker::new_shared(),
            PathPolicy::new_shared(),
            crate::a2a::diagnostics_dispatch::DiagnosticsPaths {
                config_dir: PathBuf::from("/tmp/residuum-test-config-unused"),
                workspace_dir,
                hub_dir: PathBuf::from("/tmp/residuum-test-hub-unused"),
            },
            None,
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        register_defaults_in(&mut registry, PathBuf::from("/tmp/residuum-test-workspace"));
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

    fn ollama_backend() -> crate::config::StandaloneBackendConfig {
        crate::config::StandaloneBackendConfig {
            name: "ollama".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        }
    }

    #[test]
    fn reload_ollama_web_search_tool_adds_it_when_backend_is_ollama() {
        let mut registry = ToolRegistry::new();
        assert!(
            !registry
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should start without the tool"
        );

        reload_ollama_web_search_tool(&mut registry, Some(&ollama_backend()));

        assert!(
            registry
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should register the tool once the backend names ollama"
        );
    }

    #[test]
    fn reload_ollama_web_search_tool_removes_it_when_backend_changes_away() {
        let mut registry = ToolRegistry::new();
        reload_ollama_web_search_tool(&mut registry, Some(&ollama_backend()));
        assert!(
            registry
                .tool_names()
                .contains(&"ollama_web_search".to_string())
        );

        reload_ollama_web_search_tool(&mut registry, None);
        assert!(
            !registry
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should remove the tool once no standalone backend is configured"
        );
    }

    #[test]
    fn reload_ollama_web_search_tool_leaves_other_tools_alone() {
        let mut registry = ToolRegistry::new();
        register_defaults_in(
            &mut registry,
            PathBuf::from("/tmp/residuum-test-workspace-unused"),
        );

        reload_ollama_web_search_tool(&mut registry, Some(&ollama_backend()));
        reload_ollama_web_search_tool(&mut registry, None);

        let names = registry.tool_names();
        assert!(
            names.contains(&"exec".to_string()),
            "unrelated tools should survive reload"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn register_defaults_gives_exec_the_registrys_agent_keys() {
        let keys_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let keys = crate::agent_keys::AgentKeys::new_shared(keys_dir.path());
        keys.set(
            "api_key",
            "sk-live-redactme42",
            None,
            crate::agent_keys::KeyCreator::User,
        )
        .await
        .unwrap();
        let mut registry = ToolRegistry::new();
        registry.set_agent_keys(keys);
        register_defaults_in(&mut registry, workspace.path().to_path_buf());

        let result = registry
            .execute(
                "exec",
                serde_json::json!({
                    "command": "echo \"Authorization: Bearer $API_KEY\"",
                    "keys": ["api_key"]
                }),
            )
            .await
            .unwrap();
        // exec received the registry's key store, so the child saw the value.
        assert!(!result.is_error, "{}", result.output);
        assert!(
            result
                .output
                .contains("Authorization: Bearer sk-live-redactme42"),
            "{}",
            result.output
        );
        // The registry's redactor, which the turn loop applies to tool output,
        // covers the value exec exposed.
        let redacted = registry
            .redactor()
            .await
            .redact(&result.output)
            .into_owned();
        assert!(!redacted.contains("sk-live-redactme42"), "{redacted}");
        assert!(
            redacted.contains("Authorization: Bearer [agent-key:api_key]"),
            "{redacted}"
        );
    }
}

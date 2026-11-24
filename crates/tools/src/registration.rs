use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use residuum_a2a::{A2aClientHub, RemoteTaskTracker};
use residuum_agent::{HopCounter, ToolRegistry, ToolResultStore};
use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_background::team::TeamLink;
use residuum_bus::{ConversationTarget, EndpointRegistry, Publisher, SessionAddress};
use residuum_config::agent_keys::SharedAgentKeys;
use residuum_config::{SharedModelCatalog, SharedToolsPath, WorkspaceLayout};
use residuum_memory::search::HybridSearcher;
use residuum_proactivity::actions::store::ActionStore;
use residuum_skills::SharedSkillState;
use residuum_workspace::inbox::mail::MailboxDirectory;

use super::{
    SharedFileTracker, SharedPathPolicy, a2a_task_update, actions, agent_keys, background, edit,
    exec, file_bug_report, inbox, memory_get, memory_search, message_agent, ollama_web_search,
    read, send_message, skills, submit_feedback, web_fetch, workspace_checkpoints, write,
};

#[derive(Default)]
pub struct RegistryInjections {
    pub tools_path: Option<SharedToolsPath>,
    pub checkpoints: Option<Arc<residuum_checkpoints::CheckpointEngine>>,
    pub publisher: Option<Publisher>,
    pub mailboxes: Option<Arc<dyn MailboxDirectory>>,
}

pub fn attach_result_store(tools: &mut ToolRegistry, hub_dir: &Path, layout: &WorkspaceLayout) {
    tools.set_result_store(ToolResultStore::new(
        residuum_config::HubPaths::new(hub_dir).tool_results_dir(),
        layout.agent_name().unwrap_or_default(),
    ));
}

pub fn reload_timezone_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    store: Arc<Mutex<ActionStore>>,
    notify: Arc<Notify>,
    models: SharedModelCatalog,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) {
    tools.register(Box::new(actions::ScheduleActionTool::new(
        Arc::clone(&store),
        notify,
        tz,
        models,
    )));
    tools.register(Box::new(actions::ListActionsTool::new(store, tz)));
    register_mail_inbox_tools(tools, injections, layout, tz);
}

pub fn reload_ollama_web_search_tool(
    tools: &mut ToolRegistry,
    backend: Option<&residuum_config::StandaloneBackendConfig>,
) {
    match backend.filter(|backend| backend.name == "ollama") {
        Some(backend) => {
            let base_url = backend
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.ollama.com".to_string());
            register_ollama_web_search_tool(tools, backend.api_key.clone(), base_url);
            tracing::info!("reloaded ollama_web_search tool from config");
        }
        None => {
            if tools.remove("ollama_web_search") {
                tracing::info!("removed ollama_web_search tool: no longer configured");
            }
        }
    }
}

fn register_mail_inbox_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) {
    let mail = inbox::MailAccess::new(injections.mailboxes.clone(), layout, tz);
    tools.register(Box::new(inbox::InboxListTool::new(mail.clone())));
    tools.register(Box::new(inbox::InboxReadTool::new(mail.clone())));
    tools.register(Box::new(inbox::UserInboxListTool::new(mail.clone())));
    let add = inbox::UserInboxAddTool::new(mail);
    let add = match &injections.publisher {
        Some(publisher) => add.with_publisher(publisher.clone()),
        None => add,
    };
    tools.register(Box::new(add));
}

pub fn register_defaults(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    tracker: SharedFileTracker,
    policy: SharedPathPolicy,
    diagnostics_paths: residuum_a2a::diagnostics_dispatch::DiagnosticsPaths,
    config_watch: Option<super::config_reload_tracker::ConfigWriteWatch>,
) {
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

pub fn register_agent_key_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    keys: SharedAgentKeys,
    checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
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

pub fn register_skill_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    state: SharedSkillState,
) {
    let mut activate = skills::SkillActivateTool::new(Arc::clone(&state));
    if let Some(publisher) = &injections.publisher {
        activate = activate.with_publisher(publisher.clone());
    }
    tools.register(Box::new(activate));
    tools.register(Box::new(skills::SkillDeactivateTool::new(state)));
}

pub fn register_inbox_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) {
    let agent_inbox_dir = layout.agent_inbox_dir();
    let agent_archive_dir = layout.agent_inbox_archive_dir();
    tools.register(Box::new(inbox::InboxArchiveTool::new(
        agent_inbox_dir.clone(),
        agent_archive_dir.clone(),
    )));
    tools.register(Box::new(inbox::InboxRestoreTool::new(
        agent_inbox_dir,
        agent_archive_dir,
    )));
    register_mail_inbox_tools(tools, injections, layout, tz);
}

pub fn register_list_endpoints_tool(tools: &mut ToolRegistry, registry: EndpointRegistry) {
    tools.register(Box::new(
        super::list_conversations::ListConversationsTool::new(registry.clone()),
    ));
    tools.register(Box::new(super::list_endpoints::ListEndpointsTool::new(
        registry,
    )));
}

pub fn register_switch_endpoint_tool(
    tools: &mut ToolRegistry,
    registry: EndpointRegistry,
    override_tx: tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
    publisher: Publisher,
) {
    tools.register(Box::new(super::switch_endpoint::SwitchEndpointTool::new(
        registry,
        override_tx,
        publisher,
    )));
}

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

pub(crate) fn register_spawn_tool(tools: &mut ToolRegistry, deps: background::SpawnToolDeps) {
    tools.register(Box::new(background::SubagentSpawnTool::new(deps)));
}

pub(crate) fn register_a2a_task_update_tool(
    tools: &mut ToolRegistry,
    conversation_target: Option<&ConversationTarget>,
    own_address: SessionAddress,
    publisher: Publisher,
    workspace_dir: PathBuf,
    policy: SharedPathPolicy,
) {
    if conversation_target
        .is_some_and(|target| target.endpoint == residuum_bus::origin::A2A_ENDPOINT)
    {
        tools.register(Box::new(
            a2a_task_update::A2aTaskUpdateTool::new(own_address, publisher, workspace_dir)
                .with_policy(policy),
        ));
    }
}

pub(crate) fn register_ollama_web_search_tool_if_configured(
    tools: &mut ToolRegistry,
    backend: Option<&residuum_config::StandaloneBackendConfig>,
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

pub fn register_web_fetch_tool(tools: &mut ToolRegistry) {
    tools.register(Box::new(web_fetch::WebFetchTool::new()));
}

pub fn register_workspace_checkpoint_tools(
    tools: &mut ToolRegistry,
    checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
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

pub fn register_feedback_tools(
    tools: &mut ToolRegistry,
    build: residuum_core::build_info::BuildInfo,
    service: Arc<residuum_tracing_service::TracingService>,
    client_context: Arc<residuum_tracing_service::ClientContext>,
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

pub fn register_ollama_web_search_tool(
    tools: &mut ToolRegistry,
    api_key: String,
    base_url: String,
) {
    tools.register(Box::new(ollama_web_search::OllamaWebSearchTool::new(
        api_key, base_url,
    )));
}

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
    use crate::{FileTracker, PathPolicy};
    use residuum_bus::testing::bus_barrier;

    #[test]
    fn reload_timezone_tools_replaces_without_duplicating() {
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            std::path::PathBuf::from("/tmp/residuum-test-actions.json"),
        )));
        let notify = Arc::new(Notify::new());
        let mut registry = ToolRegistry::new();
        let models = residuum_config::ModelCatalog::default().into_shared();
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
        for tool in [
            "schedule_action",
            "list_actions",
            "inbox_read",
            "user_inbox_add",
            "user_inbox_list",
        ] {
            assert_eq!(
                names.iter().filter(|n| n.as_str() == tool).count(),
                1,
                "{tool} must be registered exactly once"
            );
        }
    }

    async fn adds_and_announces(registry: &ToolRegistry, bus: &residuum_bus::BusHandle) -> bool {
        let mut added = bus
            .subscribe::<_, residuum_bus::UserInboxAddedEvent>(residuum_bus::topics::UserInbox)
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
        bus_barrier(bus).await;
        !added.drain().is_empty()
    }

    #[tokio::test]
    async fn user_inbox_add_announces_through_the_injected_publisher_and_after_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let bus = residuum_bus::spawn_broker();
        let injections = RegistryInjections {
            publisher: Some(bus.publisher()),
            ..RegistryInjections::default()
        };
        let mut registry = ToolRegistry::new();
        register_inbox_tools(&mut registry, &injections, &layout, chrono_tz::UTC);
        assert!(adds_and_announces(&registry, &bus).await, "at registration");

        reload_timezone_tools(
            &mut registry,
            &injections,
            Arc::new(Mutex::new(ActionStore::new_empty(
                dir.path().join("scheduled_actions.json"),
            ))),
            Arc::new(Notify::new()),
            residuum_config::ModelCatalog::default().into_shared(),
            &layout,
            chrono_tz::America::New_York,
        );
        assert!(adds_and_announces(&registry, &bus).await, "after a reload");
    }

    fn register_defaults_in(registry: &mut ToolRegistry, workspace_dir: PathBuf) {
        register_defaults(
            registry,
            &RegistryInjections::default(),
            FileTracker::new_shared(),
            PathPolicy::new_shared(),
            residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
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

    #[tokio::test]
    async fn default_request_has_system_prompt_first_and_tools_in_registry_order() {
        use residuum_agent::testing::{RequestInputs, render_model_request};
        use residuum_inference::Role;

        let mut registry = ToolRegistry::new();
        register_defaults_in(&mut registry, PathBuf::from("/tmp/residuum-test-workspace"));

        let request = render_model_request(&RequestInputs::default(), &registry).await;

        let [system, ..] = request.messages.as_slice() else {
            panic!("the request carries at least the system prompt: {request:?}");
        };
        assert_eq!(system.role, Role::System);
        let sent: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(sent, registry.tool_names());
        assert!(
            sent.iter().any(|name| name == "exec"),
            "exec is sent: {sent:?}"
        );
    }

    fn ollama_backend() -> residuum_config::StandaloneBackendConfig {
        residuum_config::StandaloneBackendConfig {
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

    fn definitions_json(registry: &ToolRegistry) -> String {
        serde_json::to_string(&registry.definitions()).unwrap()
    }

    #[test]
    fn reload_timezone_tools_without_a_change_keeps_definitions_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("scheduled_actions.json"),
        )));
        let notify = Arc::new(Notify::new());
        let models = residuum_config::ModelCatalog::default().into_shared();
        let injections = RegistryInjections::default();
        let tz = chrono_tz::America::New_York;
        let mut registry = ToolRegistry::new();
        register_defaults_in(&mut registry, dir.path().to_path_buf());
        register_action_tools(
            &mut registry,
            Arc::clone(&store),
            Arc::clone(&notify),
            Arc::clone(&models),
            tz,
        );
        register_inbox_tools(&mut registry, &injections, &layout, tz);
        register_web_fetch_tool(&mut registry);
        let before = definitions_json(&registry);

        reload_timezone_tools(
            &mut registry,
            &injections,
            Arc::clone(&store),
            Arc::clone(&notify),
            Arc::clone(&models),
            &layout,
            tz,
        );

        assert_eq!(
            definitions_json(&registry),
            before,
            "a reload that changes nothing must not move or alter any tool"
        );
    }

    #[test]
    fn reload_timezone_tools_to_a_new_timezone_keeps_the_order() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let store = Arc::new(Mutex::new(ActionStore::new_empty(
            dir.path().join("scheduled_actions.json"),
        )));
        let notify = Arc::new(Notify::new());
        let models = residuum_config::ModelCatalog::default().into_shared();
        let injections = RegistryInjections::default();
        let mut registry = ToolRegistry::new();
        register_defaults_in(&mut registry, dir.path().to_path_buf());
        register_action_tools(
            &mut registry,
            Arc::clone(&store),
            Arc::clone(&notify),
            Arc::clone(&models),
            chrono_tz::UTC,
        );
        register_inbox_tools(&mut registry, &injections, &layout, chrono_tz::UTC);
        register_web_fetch_tool(&mut registry);
        let names_before = registry.tool_names();

        reload_timezone_tools(
            &mut registry,
            &injections,
            Arc::clone(&store),
            Arc::clone(&notify),
            Arc::clone(&models),
            &layout,
            chrono_tz::Asia::Tokyo,
        );

        assert_eq!(
            registry.tool_names(),
            names_before,
            "timezone tools are replaced where they stand"
        );
    }

    #[test]
    fn reload_ollama_web_search_tool_without_a_change_keeps_definitions_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        register_defaults_in(&mut registry, dir.path().to_path_buf());
        reload_ollama_web_search_tool(&mut registry, Some(&ollama_backend()));
        register_web_fetch_tool(&mut registry);
        let before = definitions_json(&registry);
        let names_before = registry.tool_names();

        reload_ollama_web_search_tool(&mut registry, Some(&ollama_backend()));

        assert_eq!(definitions_json(&registry), before);
        assert_eq!(registry.tool_names(), names_before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn register_defaults_gives_exec_the_registrys_agent_keys() {
        let keys_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let keys = residuum_config::agent_keys::AgentKeys::new_shared(keys_dir.path());
        keys.set(
            "api_key",
            "sk-live-redactme42",
            None,
            residuum_config::agent_keys::KeyCreator::User,
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
        assert!(!result.is_error, "{}", result.output);
        assert!(
            result
                .output
                .contains("Authorization: Bearer sk-live-redactme42"),
            "{}",
            result.output
        );
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

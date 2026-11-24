use std::sync::Arc;

use residuum_agent::ToolRegistry;
use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_bus::session::{MAIN_ADDRESS, MAIN_DEPTH};
use residuum_bus::{EndpointRegistry, SessionAddress};
use residuum_config::{Config, WorkspaceLayout};
use residuum_memory::search::HybridSearcher;
use residuum_proactivity::actions::store::ActionStore;
use residuum_skills::SharedSkillState;

use super::registration::{self, RegistryInjections};

pub struct ToolRegistryDeps<'a> {
    pub action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: &'a Arc<tokio::sync::Notify>,
    pub skill_state: &'a SharedSkillState,
    pub tools_path: &'a residuum_config::SharedToolsPath,
    pub path_policy: &'a super::SharedPathPolicy,
    pub auto_mode: &'a residuum_agent::auto_mode::SharedAutoMode,
    pub agent_keys: &'a residuum_config::agent_keys::SharedAgentKeys,
    pub session_registry: &'a Arc<SessionRegistry>,
    pub endpoint_registry: &'a EndpointRegistry,
    pub publisher: &'a residuum_bus::Publisher,
    pub build: residuum_core::build_info::BuildInfo,
    pub tracing_service: &'a Arc<residuum_tracing_service::TracingService>,
    pub tracing_client_context: &'a Arc<residuum_tracing_service::ClientContext>,
    pub agent_messenger: &'a Arc<AgentMessenger>,
    pub hop_counter: &'a residuum_agent::HopCounter,
    pub a2a_hub: &'a Arc<residuum_a2a::A2aClientHub>,
    pub a2a_tracker: &'a Arc<residuum_a2a::RemoteTaskTracker>,
    pub checkpoints: &'a Arc<residuum_checkpoints::CheckpointEngine>,
    pub config_reload_tracker: &'a crate::SharedConfigReloadTracker,
    pub hub_dir: &'a std::path::Path,
    pub lifecycle: &'a crate::LifecycleAccess,
    pub model_catalog: &'a residuum_config::SharedModelCatalog,
    pub mailboxes: Option<Arc<dyn residuum_workspace::inbox::mail::MailboxDirectory>>,
}

fn register_main_session_tools(
    tools: &mut ToolRegistry,
    cfg: &Config,
    deps: &ToolRegistryDeps<'_>,
) {
    registration::register_background_tools(
        tools,
        Arc::clone(deps.session_registry),
        SessionAddress::from(MAIN_ADDRESS),
        Arc::clone(deps.a2a_hub),
        Arc::clone(deps.a2a_tracker),
        deps.agent_messenger.team().clone(),
    );
    registration::register_spawn_tool(
        tools,
        super::background::SpawnToolDeps {
            publisher: deps.publisher.clone(),
            skill_state: Arc::clone(deps.skill_state),
            spawner_address: SessionAddress::from(MAIN_ADDRESS),
            depth: MAIN_DEPTH,
            depth_cap: cfg.background.subagent_depth_cap,
            hop_counter: deps.hop_counter.clone(),
            models: Arc::clone(deps.model_catalog),
        },
    );
    registration::register_agent_lifecycle_tools(
        tools,
        deps.lifecycle.clone(),
        deps.hop_counter.clone(),
    );
}

fn register_main_workspace_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    cfg: &Config,
    layout: &WorkspaceLayout,
    hybrid_searcher: &Arc<HybridSearcher>,
    tz: chrono_tz::Tz,
    deps: &ToolRegistryDeps<'_>,
) {
    let file_tracker = super::FileTracker::new_shared();
    let diagnostics_paths = residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
        config_dir: cfg.config_dir.clone(),
        workspace_dir: cfg.workspace_dir.clone(),
        hub_dir: deps.hub_dir.to_path_buf(),
    };
    let config_watch = super::ConfigWriteWatch {
        recognized: super::config_reload_tracker::RecognizedConfigPaths::new(
            &cfg.config_dir,
            deps.hub_dir,
            layout,
        ),
        tracker: deps.config_reload_tracker.clone(),
    };
    registration::register_defaults(
        tools,
        injections,
        file_tracker,
        Arc::clone(deps.path_policy),
        diagnostics_paths,
        Some(config_watch),
    );
    registration::register_agent_key_tools(
        tools,
        injections,
        Arc::clone(deps.agent_keys),
        Arc::clone(deps.checkpoints),
    );
    let (episodes_dir, sessions_dir) = (layout.episodes_dir(), layout.sessions_dir());
    registration::register_memory_tools(
        tools,
        Arc::clone(hybrid_searcher),
        episodes_dir,
        sessions_dir,
    );
    registration::register_action_tools(
        tools,
        Arc::clone(deps.action_store),
        Arc::clone(deps.action_notify),
        Arc::clone(deps.model_catalog),
        tz,
    );
    registration::register_skill_tools(tools, injections, Arc::clone(deps.skill_state));
    registration::register_inbox_tools(tools, injections, layout, tz);
}

#[must_use]
pub fn init_tool_registry(
    cfg: &Config,
    layout: &WorkspaceLayout,
    hybrid_searcher: &Arc<HybridSearcher>,
    tz: chrono_tz::Tz,
    deps: &ToolRegistryDeps<'_>,
) -> (
    ToolRegistry,
    RegistryInjections,
    tokio::sync::watch::Sender<Option<residuum_bus::EndpointName>>,
) {
    let injections = RegistryInjections {
        tools_path: Some(Arc::clone(deps.tools_path)),
        checkpoints: Some(Arc::clone(deps.checkpoints)),
        publisher: Some(deps.publisher.clone()),
        mailboxes: deps.mailboxes.clone(),
    };
    let mut tools = ToolRegistry::new();
    tools.set_agent_keys(Arc::clone(deps.agent_keys));
    tools.set_auto_mode(Arc::clone(deps.auto_mode));
    registration::attach_result_store(&mut tools, deps.hub_dir, layout);
    register_main_workspace_tools(
        &mut tools,
        &injections,
        cfg,
        layout,
        hybrid_searcher,
        tz,
        deps,
    );
    register_main_session_tools(&mut tools, cfg, deps);

    registration::register_send_message_tool(
        &mut tools,
        deps.endpoint_registry.clone(),
        deps.publisher.clone(),
        false,
    );
    registration::register_list_endpoints_tool(&mut tools, deps.endpoint_registry.clone());
    registration::register_message_agent_tool(
        &mut tools,
        SessionAddress::from(MAIN_ADDRESS),
        MAIN_ADDRESS.to_string(),
        Arc::clone(deps.agent_messenger),
        deps.hop_counter.clone(),
        Arc::clone(deps.a2a_hub),
        Arc::clone(deps.a2a_tracker),
    );

    let override_tx = tokio::sync::watch::Sender::new(None);
    let override_tx_for_runtime = override_tx.clone();
    registration::register_switch_endpoint_tool(
        &mut tools,
        deps.endpoint_registry.clone(),
        override_tx,
        deps.publisher.clone(),
    );

    registration::register_web_fetch_tool(&mut tools);

    registration::register_feedback_tools(
        &mut tools,
        deps.build,
        Arc::clone(deps.tracing_service),
        Arc::clone(deps.tracing_client_context),
        Arc::clone(deps.session_registry),
    );

    registration::register_workspace_checkpoint_tools(
        &mut tools,
        Arc::clone(deps.checkpoints),
        Arc::clone(deps.path_policy),
    );

    if let Some(backend) = &cfg.web_search.standalone_backend
        && backend.name == "ollama"
    {
        let base_url = backend
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.ollama.com".to_string());
        registration::register_ollama_web_search_tool(
            &mut tools,
            backend.api_key.clone(),
            base_url,
        );
        tracing::info!("registered ollama_web_search tool");
    }

    (tools, injections, override_tx_for_runtime)
}

#[cfg(test)]
mod tests {
    use crate::testing::RegistryHarness;

    const MAIN_ONLY_TOOLS: &[&str] = &["switch_endpoint"];

    const SESSION_ONLY_TOOLS: &[&str] = &["a2a_task_update"];

    const TOOLS_MD: &str = include_str!("TOOLS.md");

    fn quote_as_tools_md(description: &str) -> String {
        description
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    ">".to_string()
                } else {
                    format!("> {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn every_main_tool_description_is_quoted_in_tools_md() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = RegistryHarness::build(dir.path()).await;

        // A Windows checkout with autocrlf turns TOOLS.md's line endings into
        // CRLF, while the descriptions are LF, so compare on LF.
        let tools_md = TOOLS_MD.replace("\r\n", "\n");
        let definitions = h.main_registry().definitions();
        assert!(!definitions.is_empty(), "main registers no tools");
        for definition in definitions {
            let expected = format!("\n{}\n", quote_as_tools_md(&definition.description));
            assert!(
                tools_md.contains(&expected),
                "crates/tools/src/TOOLS.md does not quote the current description of `{}`: \
                 update that tool's \"Description sent to LLM\" block to match its \
                 definition() exactly",
                definition.name
            );
        }
    }

    fn a2a_conversation_target() -> residuum_bus::ConversationTarget {
        residuum_bus::ConversationTarget {
            endpoint: "a2a".to_string(),
            conversation_id: "key:tester/ctx-1".to_string(),
        }
    }

    #[tokio::test]
    async fn session_registry_matches_main_minus_documented_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = RegistryHarness::build(dir.path()).await;

        let main_tools = h.main_registry();
        let mut main_names = main_tools.tool_names();
        main_names.sort();

        let session_tools = h.session_registry_for(
            "spawned-test-0001",
            "spawned",
            Some(a2a_conversation_target()),
        );
        let mut session_names = session_tools.tool_names();
        session_names.sort();

        let mut expected: Vec<String> = main_names
            .iter()
            .filter(|name| !MAIN_ONLY_TOOLS.contains(&name.as_str()))
            .cloned()
            .chain(SESSION_ONLY_TOOLS.iter().map(ToString::to_string))
            .collect();
        expected.sort();

        assert_eq!(
            session_names, expected,
            "session registry must equal main minus the main-only allowlist \
             ({MAIN_ONLY_TOOLS:?}) plus the session-only allowlist ({SESSION_ONLY_TOOLS:?})"
        );

        let main_store = main_tools
            .result_store()
            .expect("main's registry saves oversized tool results");
        let session_store = session_tools
            .result_store()
            .expect("a session's registry saves oversized tool results");
        assert_eq!(main_store.dir(), session_store.dir());
        assert!(
            main_store
                .dir()
                .starts_with(residuum_config::HubPaths::new(&h.hub_dir).tool_results_dir()),
            "saved results live under hub/tool-results: {}",
            main_store.dir().display()
        );
    }

    #[tokio::test]
    async fn artifact_session_keeps_every_session_tool_but_cannot_message_main() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = RegistryHarness::build(dir.path()).await;

        let mut spawned_names = h
            .session_registry_for("spawned-test-0001", "spawned", None)
            .tool_names();
        spawned_names.sort();
        let artifact_tools = h.session_registry_for("artifact-wiki-0001", "artifact", None);
        let mut artifact_names = artifact_tools.tool_names();
        artifact_names.sort();
        assert_eq!(artifact_names, spawned_names);

        let to_main = artifact_tools
            .execute(
                "message_agent",
                serde_json::json!({ "to": "main", "message": "done" }),
            )
            .await
            .expect("message_agent returns a tool error, not a failure");
        assert!(to_main.is_error);
        assert!(
            to_main.output.contains("can't reach the main conversation"),
            "got: {}",
            to_main.output
        );

        std::fs::create_dir_all(h.layout.user_inbox_dir()).unwrap();
        let inbox = artifact_tools
            .execute(
                "user_inbox_add",
                serde_json::json!({ "title": "Wiki refreshed", "body": "3 pages updated" }),
            )
            .await
            .expect("user_inbox_add runs");
        assert!(!inbox.is_error, "got: {}", inbox.output);
        let filed = std::fs::read_dir(h.layout.user_inbox_dir())
            .expect("user inbox dir exists after an add")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .count();
        assert_eq!(filed, 1, "the item lands in the user's inbox");
    }

    #[tokio::test]
    async fn only_an_a2a_conversation_session_gets_the_a2a_task_update_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = RegistryHarness::build(dir.path()).await;

        let a2a_tools = h.session_registry_for(
            "external-a2a-0001",
            "external",
            Some(a2a_conversation_target()),
        );
        assert!(
            a2a_tools
                .tool_names()
                .contains(&"a2a_task_update".to_string()),
            "an a2a conversation session must get a2a_task_update"
        );

        let plain_session = h.session_registry_for("spawned-test-0002", "spawned", None);
        assert!(
            !plain_session
                .tool_names()
                .contains(&"a2a_task_update".to_string()),
            "a non-a2a session must not get a2a_task_update"
        );

        let discord_target = residuum_bus::ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "chan-1".to_string(),
        };
        let discord_session =
            h.session_registry_for("external-discord-0001", "external", Some(discord_target));
        assert!(
            !discord_session
                .tool_names()
                .contains(&"a2a_task_update".to_string()),
            "a conversation session for another endpoint must not get a2a_task_update"
        );
    }
}

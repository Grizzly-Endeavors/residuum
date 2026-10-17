//! The main agent's tool registry: the builder that registers every tool main
//! carries, beside the session builder (`SessionTools::build`) that the
//! parity test in this file holds it against.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::agent::ToolRegistry;
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::bus::session::{MAIN_ADDRESS, MAIN_DEPTH};
use crate::bus::{EndpointRegistry, SessionAddress};
use crate::config::{Config, WorkspaceLayout};
use crate::memory::search::HybridSearcher;
use crate::skills::SharedSkillState;

use super::registration::{self, RegistryInjections};

/// Shared subsystem handles needed for tool registration.
pub(crate) struct ToolRegistryDeps<'a> {
    pub action_store: &'a Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: &'a Arc<tokio::sync::Notify>,
    pub skill_state: &'a SharedSkillState,
    pub tools_path: &'a crate::config::SharedToolsPath,
    /// Shared write policy — the same instance sessions fork with and
    /// config reloads update.
    pub path_policy: &'a super::SharedPathPolicy,
    /// The agent's Auto Mode, attached to main's registry so the turn loop
    /// checks main's tool calls against it.
    pub auto_mode: &'a crate::agent::auto_mode::SharedAutoMode,
    pub agent_keys: &'a crate::agent_keys::SharedAgentKeys,
    pub session_registry: &'a Arc<SessionRegistry>,
    pub endpoint_registry: &'a EndpointRegistry,
    pub publisher: &'a crate::bus::Publisher,
    /// The running build, for the feedback tools' client context.
    pub build: crate::build_info::BuildInfo,
    pub tracing_service: &'a Arc<crate::tracing_service::TracingService>,
    pub tracing_client_context: &'a Arc<crate::tracing_service::ClientContext>,
    pub agent_messenger: &'a Arc<AgentMessenger>,
    /// Main's current-turn hop counter, shared with the `Agent` these tools
    /// end up registered against (see `CreateAgentArgs::hop_counter`).
    pub hop_counter: &'a crate::agent::HopCounter,
    /// Remote A2A agents this instance's client can reach.
    pub a2a_hub: &'a Arc<crate::a2a::A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents.
    pub a2a_tracker: &'a Arc<crate::a2a::RemoteTaskTracker>,
    /// Workspace and config checkpoint repositories.
    pub checkpoints: &'a Arc<crate::checkpoints::CheckpointEngine>,
    /// Tracks the agent's own config-file writes so the reload each one
    /// triggers can report back into its transcript. Wired only into main's
    /// `write_file`/`edit_file` — see `ConfigWriteWatch`'s doc comment.
    pub config_reload_tracker: &'a crate::tools::SharedConfigReloadTracker,
    /// The hub's directory (`~/.residuum/hub`), needed to resolve the hub
    /// config that `config.toml`/`providers.toml` diagnostics run against.
    pub hub_dir: &'a std::path::Path,
    /// How `agent_create`/`agent_delete` reach the hub.
    pub lifecycle: &'a crate::tools::LifecycleAccess,
    /// The live catalog `subagent_spawn` and `schedule_action` check an
    /// explicit model against — the same instance sessions fork with and
    /// config reloads update.
    pub model_catalog: &'a crate::config::SharedModelCatalog,
}

/// Main's tools for managing sessions and teammates: `stop_agent`,
/// `list_agents`, `subagent_spawn`, and the teammate lifecycle tools.
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

/// Main's file, key, memory, action-scheduling, skill, and inbox tools.
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
    let diagnostics_paths = crate::a2a::diagnostics_dispatch::DiagnosticsPaths {
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
    registration::register_skill_tools(tools, Arc::clone(deps.skill_state));
    registration::register_inbox_tools(
        tools,
        injections,
        layout.agent_inbox_dir(),
        layout.agent_inbox_archive_dir(),
        layout.user_inbox_dir(),
        layout.user_inbox_attachments_dir(),
        tz,
    );
}

/// Build main's tool registry with all default and domain-specific tools.
///
/// Returns the registry, the injections its tools took at registration (kept
/// beside the agent so a reload that re-registers a tool injects the same
/// ones), and the sender `switch_endpoint` writes the output override to.
pub(crate) fn init_tool_registry(
    cfg: &Config,
    layout: &WorkspaceLayout,
    hybrid_searcher: &Arc<HybridSearcher>,
    tz: chrono_tz::Tz,
    deps: &ToolRegistryDeps<'_>,
) -> (
    ToolRegistry,
    RegistryInjections,
    tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
) {
    let injections = RegistryInjections {
        tools_path: Some(Arc::clone(deps.tools_path)),
        checkpoints: Some(Arc::clone(deps.checkpoints)),
        publisher: Some(deps.publisher.clone()),
    };
    let mut tools = ToolRegistry::new();
    tools.set_agent_keys(Arc::clone(deps.agent_keys));
    tools.set_auto_mode(Arc::clone(deps.auto_mode));
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

    // Workspace checkpoint history (workspace repository only)
    registration::register_workspace_checkpoint_tools(
        &mut tools,
        Arc::clone(deps.checkpoints),
        Arc::clone(deps.path_policy),
    );

    // Register Ollama Cloud web search tool if configured
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
    use super::*;
    use crate::tools::testing::RegistryHarness;

    /// Tools that must stay main-only, never registered for a session. This
    /// is the one documented exclusion from the session registry
    /// `crate::tools::SessionTools` builds (see its `build` doc comment) —
    /// keep the two lists in sync.
    const MAIN_ONLY_TOOLS: &[&str] = &["switch_endpoint"];

    /// Tools registered only for a session, never for main — the reverse of
    /// [`MAIN_ONLY_TOOLS`]. `a2a_task_update` is session-only because it
    /// reports a delegated A2A task's outcome, which only makes sense for a
    /// session started from the `a2a` endpoint — main never handles A2A
    /// callers directly (see `docs/systems-usage/a2a.md`).
    const SESSION_ONLY_TOOLS: &[&str] = &["a2a_task_update"];

    /// A `ConversationTarget` naming the `a2a` endpoint, as an a2a
    /// conversation session's tool scope carries.
    fn a2a_conversation_target() -> crate::bus::ConversationTarget {
        crate::bus::ConversationTarget {
            endpoint: "a2a".to_string(),
            conversation_id: "key:tester/ctx-1".to_string(),
        }
    }

    /// Enforces "every tool registered for main is also registered for
    /// sessions, except the documented main-only allowlist" by building both
    /// registration surfaces from equivalent config (every optional tool
    /// gate turned on) and comparing their tool names directly. A tool added
    /// to one registry but not the other fails this test instead of drifting
    /// silently.
    #[tokio::test]
    async fn session_registry_matches_main_minus_documented_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = RegistryHarness::build(dir.path()).await;

        let auto_mode = crate::agent::auto_mode::AutoModeGate::new_shared(
            "test",
            crate::config::AutoModeConfig::default(),
            crate::inference::system_one::SystemOneService::new(None),
        );
        let model_catalog = crate::config::ModelCatalog::default().into_shared();
        let deps = ToolRegistryDeps {
            action_store: &h.action_store,
            action_notify: &h.action_notify,
            skill_state: &h.skill_state,
            tools_path: &h.tools_path,
            path_policy: &h.path_policy,
            auto_mode: &auto_mode,
            agent_keys: &h.agent_keys,
            session_registry: &h.session_registry,
            endpoint_registry: &h.endpoint_registry,
            publisher: &h.publisher,
            build: residuum_core::testing::HARNESS_BUILD,
            tracing_service: &h.tracing_service,
            tracing_client_context: &h.tracing_client_context,
            agent_messenger: &h.agent_messenger,
            hop_counter: &h.hop_counter,
            a2a_hub: &h.a2a_hub,
            a2a_tracker: &h.a2a_tracker,
            checkpoints: &h.checkpoints,
            config_reload_tracker: &h.config_reload_tracker,
            hub_dir: &h.hub_dir,
            lifecycle: &h.lifecycle,
            model_catalog: &model_catalog,
        };
        let (main_tools, _, _) =
            init_tool_registry(&h.cfg, &h.layout, &h.hybrid_searcher, chrono_tz::UTC, &deps);
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
    }

    /// An `artifact` session gets the same tools as a spawned one; only its
    /// `message_agent` to `main` is refused, while its user-inbox tool still
    /// files items.
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

        // Workspace bootstrap creates the inbox folders in a real install.
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

    /// Only a session started from the `a2a` endpoint gets `a2a_task_update`
    /// — an ordinary session (any other `conversation_target`, including
    /// none at all) never does.
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

        let discord_target = crate::bus::ConversationTarget {
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

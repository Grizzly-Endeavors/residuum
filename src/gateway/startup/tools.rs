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
    /// Shared write policy — the same instance sessions fork with and
    /// config reloads update.
    pub path_policy: &'a crate::tools::SharedPathPolicy,
    pub agent_keys: &'a crate::agent_keys::SharedAgentKeys,
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
    tokio::sync::watch::Sender<Option<crate::bus::EndpointName>>,
) {
    let mut tools = ToolRegistry::new();
    tools.set_tools_path(Arc::clone(deps.tools_path));
    tools.set_agent_keys(Arc::clone(deps.agent_keys));
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker, Arc::clone(deps.path_policy));
    tools.register_agent_key_tools(Arc::clone(deps.agent_keys));
    tools.register_search_tool(Arc::clone(&mem.hybrid_searcher));
    tools.register_memory_get_tool(layout.episodes_dir(), layout.sessions_dir());
    tools.register_action_tools(
        Arc::clone(deps.action_store),
        Arc::clone(deps.action_notify),
        tz,
    );
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

    (tools, override_tx_for_runtime)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::store::ActionStore;
    use crate::agent::HopCounter;
    use crate::background::HopLimits;
    use crate::background::messaging::AgentMessenger;
    use crate::background::registry::SessionRegistry;
    use crate::background::store::SessionStore;
    use crate::bus::{EndpointRegistry, Publisher, SessionAddress};
    use crate::config::{
        AgentAbilitiesConfig, BackgroundConfig, GatewayConfig, IdleConfig, LearningConfig,
        MemoryConfig, SearchConfig, SkillsConfig, StandaloneBackendConfig, SubconsciousSettings,
        ToolsConfig, TracingConfig, WebSearchConfig,
    };
    use crate::inference::retry::RetryConfig;
    use crate::memory::search::{HybridSearcher, MemoryIndex};
    use crate::skills::{SkillIndex, SkillState};
    use crate::tools::{FileTracker, PathPolicy};
    use crate::tracing_service::TracingService;
    use crate::util::telemetry::{SpanBufferConfig, SpanBufferLayer};
    use std::collections::HashMap;

    /// Tools that must stay main-only, never registered for a session. This
    /// is the one documented exclusion from `ToolRegistry::build_subagent_registry`
    /// (see its doc comment) — keep the two lists in sync.
    const MAIN_ONLY_TOOLS: &[&str] = &["switch_endpoint"];

    /// A minimal but fully populated `Config`, with every optional
    /// tool-gating switch turned on (here: an Ollama standalone web search
    /// backend), so `session_registry_matches_main_minus_documented_allowlist`
    /// exercises every conditionally-registered tool on both surfaces.
    fn test_config(dir: &std::path::Path) -> Config {
        Config {
            name: None,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            subconscious: vec![],
            embedding: None,
            workspace_dir: dir.to_path_buf(),
            timeout_secs: 30,
            max_tokens: 4096,
            memory: MemoryConfig::default(),
            pulse_enabled: false,
            subconscious_settings: SubconsciousSettings::default(),
            learning: LearningConfig::default(),
            gateway: GatewayConfig::default(),
            timezone: chrono_tz::UTC,
            cloud: None,
            discord: None,
            telegram: None,
            teams: None,
            webhooks: HashMap::new(),
            skills: SkillsConfig { dirs: vec![] },
            tools: ToolsConfig { dirs: vec![] },
            retry: RetryConfig::default(),
            background: BackgroundConfig::default(),
            agent: AgentAbilitiesConfig::default(),
            idle: IdleConfig::default(),
            temperature: None,
            thinking: None,
            web_search: WebSearchConfig {
                provider_native: None,
                standalone_backend: Some(StandaloneBackendConfig {
                    name: "ollama".to_string(),
                    api_key: "test-key".to_string(),
                    base_url: None,
                }),
            },
            tracing: TracingConfig::default(),
            role_overrides: HashMap::new(),
            config_dir: dir.to_path_buf(),
        }
    }

    /// Shared scaffolding both registries are built from, so the comparison
    /// in the test below isolates the one thing it actually cares about
    /// (which tools got registered) rather than incidental config drift
    /// between two independently hand-built setups.
    struct Harness {
        cfg: Config,
        layout: WorkspaceLayout,
        mem: super::super::memory::MemoryComponents,
        action_store: Arc<tokio::sync::Mutex<ActionStore>>,
        action_notify: Arc<tokio::sync::Notify>,
        skill_state: SharedSkillState,
        tools_path: crate::tools::SharedToolsPath,
        path_policy: crate::tools::SharedPathPolicy,
        agent_keys: crate::agent_keys::SharedAgentKeys,
        session_registry: Arc<SessionRegistry>,
        endpoint_registry: EndpointRegistry,
        publisher: Publisher,
        tracing_service: Arc<TracingService>,
        tracing_client_context: Arc<crate::tracing_service::ClientContext>,
        agent_messenger: Arc<AgentMessenger>,
        hop_counter: HopCounter,
    }

    fn build_harness(dir: &std::path::Path) -> Harness {
        let cfg = test_config(dir);
        let layout = WorkspaceLayout::new(dir);

        let search_index = Arc::new(MemoryIndex::empty().expect("empty search index"));
        let hybrid_searcher = Arc::new(HybridSearcher::new(
            Arc::clone(&search_index),
            None,
            None,
            SearchConfig::default(),
        ));
        let mem = super::super::memory::MemoryComponents {
            search_index,
            hybrid_searcher,
            vector_store: None,
        };

        let action_store = Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
            layout.scheduled_actions_json(),
        )));
        let action_notify = Arc::new(tokio::sync::Notify::new());
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let tools_path: crate::tools::SharedToolsPath = Arc::new(tokio::sync::RwLock::new(None));
        let path_policy = PathPolicy::new_shared();
        let agent_keys = crate::agent_keys::AgentKeys::new_shared(dir);
        let session_registry = Arc::new(SessionRegistry::new());
        let endpoint_registry = EndpointRegistry::from_config(&cfg, &[]);
        let publisher = Publisher::noop();

        let (_, span_buffer) = SpanBufferLayer::new(&SpanBufferConfig::default());
        let tracing_service = Arc::new(TracingService::new(cfg.tracing.clone(), span_buffer));
        let tracing_client_context =
            Arc::new(crate::tracing_service::client_context::gather_for_bug_report(&cfg));

        let session_store = Arc::new(SessionStore::new(layout.sessions_dir()));
        let agent_messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&session_registry),
            publisher.clone(),
            session_store,
            HopLimits::from(&cfg.background),
        ));
        let hop_counter = HopCounter::new(0);

        Harness {
            cfg,
            layout,
            mem,
            action_store,
            action_notify,
            skill_state,
            tools_path,
            path_policy,
            agent_keys,
            session_registry,
            endpoint_registry,
            publisher,
            tracing_service,
            tracing_client_context,
            agent_messenger,
            hop_counter,
        }
    }

    /// Build a session's tool registry from the harness, as the fork path
    /// does for a session at `address` in `category`.
    fn session_registry_for(
        h: &Harness,
        address: &str,
        category: &str,
    ) -> crate::tools::ToolRegistry {
        crate::tools::ToolRegistry::build_subagent_registry(crate::tools::SubagentToolDeps {
            tracker: FileTracker::new_shared(),
            path_policy: Arc::clone(&h.path_policy),
            tools_path: Arc::clone(&h.tools_path),
            agent_keys: Arc::clone(&h.agent_keys),
            skill_state: Arc::clone(&h.skill_state),
            tz: chrono_tz::UTC,
            hybrid_searcher: Arc::clone(&h.mem.hybrid_searcher),
            episodes_dir: h.layout.episodes_dir(),
            sessions_dir: h.layout.sessions_dir(),
            agent_inbox_dir: h.layout.agent_inbox_dir(),
            agent_inbox_archive_dir: h.layout.agent_inbox_archive_dir(),
            user_inbox_dir: h.layout.user_inbox_dir(),
            user_inbox_attachments_dir: h.layout.user_inbox_attachments_dir(),
            session_registry: Arc::clone(&h.session_registry),
            endpoint_registry: h.endpoint_registry.clone(),
            publisher: h.publisher.clone(),
            action_store: Arc::clone(&h.action_store),
            action_notify: Arc::clone(&h.action_notify),
            own_address: SessionAddress::from(address),
            own_depth: 1,
            depth_cap: h.cfg.background.subagent_depth_cap,
            session_category: category.to_string(),
            messenger: Arc::clone(&h.agent_messenger),
            hop_counter: h.hop_counter.clone(),
            tracing_service: Arc::clone(&h.tracing_service),
            tracing_client_context: Arc::clone(&h.tracing_client_context),
            web_search_backend: h.cfg.web_search.standalone_backend.clone(),
        })
    }

    /// Enforces "every tool registered for main is also registered for
    /// sessions, except the documented main-only allowlist" by building both
    /// registration surfaces from equivalent config (every optional tool
    /// gate turned on) and comparing their tool names directly. A tool added
    /// to one registry but not the other fails this test instead of drifting
    /// silently — see the "Past gap" history this replaced in
    /// `src/tools/CLAUDE.md`.
    #[test]
    fn session_registry_matches_main_minus_documented_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = build_harness(dir.path());

        let deps = ToolRegistryDeps {
            action_store: &h.action_store,
            action_notify: &h.action_notify,
            skill_state: &h.skill_state,
            tools_path: &h.tools_path,
            path_policy: &h.path_policy,
            agent_keys: &h.agent_keys,
            session_registry: &h.session_registry,
            endpoint_registry: &h.endpoint_registry,
            publisher: &h.publisher,
            tracing_service: &h.tracing_service,
            tracing_client_context: &h.tracing_client_context,
            agent_messenger: &h.agent_messenger,
            hop_counter: &h.hop_counter,
        };
        let (main_tools, _) = init_tool_registry(&h.cfg, &h.layout, &h.mem, chrono_tz::UTC, &deps);
        let mut main_names = main_tools.tool_names();
        main_names.sort();

        let session_tools = session_registry_for(&h, "spawned-test-0001", "spawned");
        let mut session_names = session_tools.tool_names();
        session_names.sort();

        let mut expected: Vec<String> = main_names
            .iter()
            .filter(|name| !MAIN_ONLY_TOOLS.contains(&name.as_str()))
            .cloned()
            .collect();
        expected.sort();

        assert_eq!(
            session_names, expected,
            "session registry must carry every main tool except the documented \
             main-only allowlist ({MAIN_ONLY_TOOLS:?})"
        );
    }

    /// An `artifact` session gets the same tools as a spawned one; only its
    /// `message_agent` to `main` is refused, while its user-inbox tool still
    /// files items.
    #[tokio::test]
    async fn artifact_session_keeps_every_session_tool_but_cannot_message_main() {
        let dir = tempfile::tempdir().expect("tempdir");
        let h = build_harness(dir.path());

        let mut spawned_names =
            session_registry_for(&h, "spawned-test-0001", "spawned").tool_names();
        spawned_names.sort();
        let artifact_tools = session_registry_for(&h, "artifact-wiki-0001", "artifact");
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
}

//! Shared scaffolding for the registry tests: one set of handles from which
//! both the main registry and a session registry are built, so a comparison
//! between them isolates what it cares about (which tools got registered)
//! rather than incidental config drift between two hand-built setups.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::{FileTracker, PathPolicy};
use residuum_a2a::diagnostics_dispatch::DiagnosticsPaths;
use residuum_agent::hop::HopLimits;
use residuum_agent::{HopCounter, ToolRegistry};
use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_background::session_tools::{SessionToolFactory as _, SessionToolScope};
use residuum_background::store::SessionStore;
use residuum_bus::session::SessionCategory;
use residuum_bus::{ConversationTarget, EndpointRegistry, EventTrigger, Publisher, SessionAddress};
use residuum_config::{
    AgentAbilitiesConfig, BackgroundConfig, Config, GatewayConfig, IdleConfig, LearningConfig,
    MemoryConfig, RetryConfig, SearchConfig, SkillsConfig, StandaloneBackendConfig,
    SubconsciousSettings, ToolsConfig, TracingConfig, WebSearchConfig, WorkspaceLayout,
};
use residuum_core::util::telemetry::{SpanBufferConfig, SpanBufferLayer};
use residuum_memory::search::{HybridSearcher, MemoryIndex};
use residuum_proactivity::actions::store::ActionStore;
use residuum_skills::{SharedSkillState, SkillIndex, SkillState};
use residuum_tracing_service::TracingService;
use residuum_workspace::testing::TeamHub;

/// A minimal but fully populated `Config`, with every optional
/// tool-gating switch turned on (here: an Ollama standalone web search
/// backend), so the parity test exercises every conditionally-registered
/// tool on both surfaces.
fn test_config(dir: &Path) -> Config {
    Config {
        agent_name: "test-agent".to_string(),
        autostart: true,
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
        a2a: residuum_config::A2aConfig::default(),
        webhooks: HashMap::new(),
        skills: SkillsConfig { dirs: vec![] },
        tools: ToolsConfig { dirs: vec![] },
        retry: RetryConfig::default(),
        background: BackgroundConfig::default(),
        agent: AgentAbilitiesConfig::default(),
        auto_mode: residuum_config::AutoModeConfig::default(),
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
        model_catalog: residuum_config::ModelCatalog::default(),
        config_dir: dir.to_path_buf(),
        load_notices: vec![],
    }
}

/// The shared handles both registries are built from.
pub struct RegistryHarness {
    pub cfg: Config,
    pub layout: WorkspaceLayout,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub skill_state: SharedSkillState,
    pub tools_path: residuum_config::SharedToolsPath,
    pub path_policy: crate::SharedPathPolicy,
    pub agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    pub session_registry: Arc<SessionRegistry>,
    pub endpoint_registry: EndpointRegistry,
    pub publisher: Publisher,
    pub tracing_service: Arc<TracingService>,
    pub tracing_client_context: Arc<residuum_tracing_service::ClientContext>,
    pub agent_messenger: Arc<AgentMessenger>,
    pub hop_counter: HopCounter,
    pub a2a_hub: Arc<residuum_a2a::A2aClientHub>,
    pub a2a_tracker: Arc<residuum_a2a::RemoteTaskTracker>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub config_reload_tracker: crate::SharedConfigReloadTracker,
    pub hub_dir: std::path::PathBuf,
    pub lifecycle: crate::LifecycleAccess,
}

impl RegistryHarness {
    /// Build the handles over a fresh config rooted at `dir`.
    ///
    /// # Panics
    ///
    /// Panics when the search index, the checkpoint repositories or the
    /// A2A task tracker cannot be opened under `dir`.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
    )]
    pub async fn build(dir: &Path) -> Self {
        let cfg = test_config(dir);
        let layout = WorkspaceLayout::new(dir);

        let search_index = Arc::new(MemoryIndex::empty().expect("empty search index"));
        let hybrid_searcher = Arc::new(HybridSearcher::new(
            search_index,
            None,
            None,
            SearchConfig::default(),
        ));

        let action_store = Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
            layout.scheduled_actions_json(),
        )));
        let action_notify = Arc::new(tokio::sync::Notify::new());
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let tools_path: residuum_config::SharedToolsPath = Arc::new(tokio::sync::RwLock::new(None));
        let path_policy = PathPolicy::new_shared();
        let agent_keys = residuum_config::agent_keys::AgentKeys::new_shared(dir);
        let session_registry = Arc::new(SessionRegistry::new());
        let endpoint_registry = EndpointRegistry::from_config(&cfg, &[]);
        let publisher = Publisher::noop();

        let (_, span_buffer) = SpanBufferLayer::new(&SpanBufferConfig::default());
        let tracing_service = Arc::new(TracingService::new(cfg.tracing.clone(), span_buffer));
        let tracing_client_context = Arc::new(
            residuum_tracing_service::client_context::gather_for_bug_report(
                residuum_core::testing::HARNESS_BUILD,
                &cfg,
            ),
        );

        let session_store = Arc::new(SessionStore::new(layout.sessions_dir()));
        let agent_messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&session_registry),
            publisher.clone(),
            session_store,
            HopLimits::from(&cfg.background),
        ));
        let hop_counter = HopCounter::new(0);
        let a2a_hub = residuum_a2a::A2aClientHub::new_shared();
        let a2a_tracker = residuum_a2a::RemoteTaskTracker::load(
            layout.a2a_outbound_json(),
            Arc::clone(&a2a_hub),
            Arc::clone(&agent_messenger),
            layout.agent_inbox_dir(),
            chrono_tz::UTC,
        )
        .await;
        let checkpoints = Arc::new(
            residuum_checkpoints::CheckpointEngine::new(
                "test-agent",
                layout.root().to_path_buf(),
                &residuum_config::paths::TeamPaths::new(dir.to_path_buf().join("team")),
                dir.join("agent-config"),
                dir,
                &dir.join("checkpoints"),
                None,
            )
            .expect("checkpoint repos should open in a fresh tempdir"),
        );

        Self {
            cfg,
            layout,
            hybrid_searcher,
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
            a2a_hub,
            a2a_tracker,
            checkpoints,
            config_reload_tracker: crate::SharedConfigReloadTracker::new_shared(),
            hub_dir: dir.join("hub"),
            lifecycle: crate::LifecycleAccess::new(
                residuum_background::team::DirectoryHandle::unbound(),
                "test",
            ),
        }
    }

    /// Build a session's tool registry, as the fork path does for a session
    /// at `address` in `category`. `conversation_target` mirrors what a
    /// conversation-triggered session carries (`None` for every other
    /// trigger) — it gates `a2a_task_update`.
    ///
    /// # Panics
    ///
    /// Panics when `category` is not a known session category.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test-support helper: a harness naming an unknown category fails the test that asked for it"
    )]
    pub fn session_registry_for(
        &self,
        address: &str,
        category: &str,
        conversation_target: Option<ConversationTarget>,
    ) -> ToolRegistry {
        let session_tools = crate::SessionTools {
            path_policy: Arc::clone(&self.path_policy),
            tools_path: Arc::clone(&self.tools_path),
            agent_keys: Arc::clone(&self.agent_keys),
            checkpoints: Arc::clone(&self.checkpoints),
            publisher: self.publisher.clone(),
            endpoint_registry: self.endpoint_registry.clone(),
            session_registry: Arc::clone(&self.session_registry),
            action_store: Arc::clone(&self.action_store),
            action_notify: Arc::clone(&self.action_notify),
            hybrid_searcher: Arc::clone(&self.hybrid_searcher),
            tracing_service: Arc::clone(&self.tracing_service),
            tracing_client_context: Arc::clone(&self.tracing_client_context),
            build: residuum_core::testing::HARNESS_BUILD,
            web_search_backend: self.cfg.web_search.standalone_backend.clone(),
            model_catalog: residuum_config::ModelCatalog::default().into_shared(),
            a2a_hub: Arc::clone(&self.a2a_hub),
            a2a_tracker: Arc::clone(&self.a2a_tracker),
            lifecycle: self.lifecycle.clone(),
            messenger: Arc::clone(&self.agent_messenger),
            layout: self.layout.clone(),
            config_dir: self.cfg.config_dir.clone(),
            hub_dir: self.hub_dir.clone(),
            tz: chrono_tz::UTC,
            depth_cap: self.cfg.background.subagent_depth_cap,
            auto_mode: None,
            mailboxes: None,
        };
        session_tools.build(SessionToolScope {
            own_address: SessionAddress::from(address),
            own_depth: 1,
            category: SessionCategory::from_label(category)
                .expect("the harness names a known session category"),
            trigger: EventTrigger::Agent,
            conversation_target,
            hop_counter: self.hop_counter.clone(),
            skill_state: Arc::clone(&self.skill_state),
        })
    }
}

/// A fresh tool set for the agent `name` over `hub`: its own read tracker,
/// sharing the hub's coordinator.
#[must_use]
pub fn tools_for(hub: &TeamHub, name: &str) -> ToolRegistry {
    let agent_dir = hub.agent_dir(name);
    let policy = PathPolicy::new_shared_with_team(
        HashSet::new(),
        hub.coordinator.view_for_agent(name, &agent_dir),
    );
    let mut registry = ToolRegistry::new();
    super::registration::register_defaults(
        &mut registry,
        &super::registration::RegistryInjections::default(),
        FileTracker::new_shared(),
        policy,
        DiagnosticsPaths {
            config_dir: agent_dir.join("config"),
            workspace_dir: agent_dir,
            hub_dir: hub.dir.path().join("hub"),
        },
        None,
    );
    registry
}

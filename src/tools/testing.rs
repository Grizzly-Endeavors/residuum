//! Shared scaffolding for the registry tests: one set of handles from which
//! both the main registry and a session registry are built, so a comparison
//! between them isolates what it cares about (which tools got registered)
//! rather than incidental config drift between two hand-built setups.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::agent::hop::HopLimits;
use crate::agent::{HopCounter, ToolRegistry};
use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::background::session_tools::{SessionToolFactory as _, SessionToolScope};
use crate::background::store::SessionStore;
use crate::bus::session::SessionCategory;
use crate::bus::{ConversationTarget, EndpointRegistry, EventTrigger, Publisher, SessionAddress};
use crate::config::{
    AgentAbilitiesConfig, BackgroundConfig, Config, GatewayConfig, IdleConfig, LearningConfig,
    MemoryConfig, RetryConfig, SearchConfig, SkillsConfig, StandaloneBackendConfig,
    SubconsciousSettings, ToolsConfig, TracingConfig, WebSearchConfig, WorkspaceLayout,
};
use crate::memory::search::{HybridSearcher, MemoryIndex};
use crate::skills::{SharedSkillState, SkillIndex, SkillState};
use crate::tools::PathPolicy;
use crate::tracing_service::TracingService;
use crate::util::telemetry::{SpanBufferConfig, SpanBufferLayer};

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
        a2a: crate::config::A2aConfig::default(),
        webhooks: HashMap::new(),
        skills: SkillsConfig { dirs: vec![] },
        tools: ToolsConfig { dirs: vec![] },
        retry: RetryConfig::default(),
        background: BackgroundConfig::default(),
        agent: AgentAbilitiesConfig::default(),
        auto_mode: crate::config::AutoModeConfig::default(),
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
        model_catalog: crate::config::ModelCatalog::default(),
        config_dir: dir.to_path_buf(),
        load_notices: vec![],
    }
}

/// The shared handles both registries are built from.
pub(crate) struct RegistryHarness {
    pub(crate) cfg: Config,
    pub(crate) layout: WorkspaceLayout,
    pub(crate) hybrid_searcher: Arc<HybridSearcher>,
    pub(crate) action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub(crate) action_notify: Arc<tokio::sync::Notify>,
    pub(crate) skill_state: SharedSkillState,
    pub(crate) tools_path: crate::config::SharedToolsPath,
    pub(crate) path_policy: crate::tools::SharedPathPolicy,
    pub(crate) agent_keys: crate::agent_keys::SharedAgentKeys,
    pub(crate) session_registry: Arc<SessionRegistry>,
    pub(crate) endpoint_registry: EndpointRegistry,
    pub(crate) publisher: Publisher,
    pub(crate) tracing_service: Arc<TracingService>,
    pub(crate) tracing_client_context: Arc<crate::tracing_service::ClientContext>,
    pub(crate) agent_messenger: Arc<AgentMessenger>,
    pub(crate) hop_counter: HopCounter,
    pub(crate) a2a_hub: Arc<crate::a2a::A2aClientHub>,
    pub(crate) a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
    pub(crate) checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    pub(crate) config_reload_tracker: crate::tools::SharedConfigReloadTracker,
    pub(crate) hub_dir: std::path::PathBuf,
    pub(crate) lifecycle: crate::tools::LifecycleAccess,
}

impl RegistryHarness {
    pub(crate) async fn build(dir: &Path) -> Self {
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
        let tools_path: crate::config::SharedToolsPath = Arc::new(tokio::sync::RwLock::new(None));
        let path_policy = PathPolicy::new_shared();
        let agent_keys = crate::agent_keys::AgentKeys::new_shared(dir);
        let session_registry = Arc::new(SessionRegistry::new());
        let endpoint_registry = EndpointRegistry::from_config(&cfg, &[]);
        let publisher = Publisher::noop();

        let (_, span_buffer) = SpanBufferLayer::new(&SpanBufferConfig::default());
        let tracing_service = Arc::new(TracingService::new(cfg.tracing.clone(), span_buffer));
        let tracing_client_context = Arc::new(
            crate::tracing_service::client_context::gather_for_bug_report(
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
        let a2a_hub = crate::a2a::A2aClientHub::new_shared();
        let a2a_tracker = crate::a2a::RemoteTaskTracker::load(
            layout.a2a_outbound_json(),
            Arc::clone(&a2a_hub),
            Arc::clone(&agent_messenger),
            layout.agent_inbox_dir(),
        )
        .await;
        let checkpoints = Arc::new(
            crate::checkpoints::CheckpointEngine::new(
                "test-agent",
                layout.root().to_path_buf(),
                &crate::config::paths::TeamPaths::new(dir.to_path_buf().join("team")),
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
            config_reload_tracker: crate::tools::SharedConfigReloadTracker::new_shared(),
            hub_dir: dir.join("hub"),
            lifecycle: crate::tools::LifecycleAccess::new(
                crate::background::team::DirectoryHandle::unbound(),
                "test",
            ),
        }
    }

    /// Build a session's tool registry, as the fork path does for a session
    /// at `address` in `category`. `conversation_target` mirrors what a
    /// conversation-triggered session carries (`None` for every other
    /// trigger) — it gates `a2a_task_update`.
    pub(crate) fn session_registry_for(
        &self,
        address: &str,
        category: &str,
        conversation_target: Option<ConversationTarget>,
    ) -> ToolRegistry {
        let session_tools = crate::tools::SessionTools {
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
            model_catalog: crate::config::ModelCatalog::default().into_shared(),
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

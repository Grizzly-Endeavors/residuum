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

pub struct SessionTools {
    pub path_policy: SharedPathPolicy,
    pub tools_path: SharedToolsPath,
    pub agent_keys: SharedAgentKeys,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub publisher: residuum_bus::Publisher,
    pub endpoint_registry: EndpointRegistry,
    pub session_registry: Arc<SessionRegistry>,
    pub action_store: Arc<Mutex<ActionStore>>,
    pub action_notify: Arc<Notify>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub tracing_service: Arc<residuum_tracing_service::TracingService>,
    pub tracing_client_context: Arc<residuum_tracing_service::ClientContext>,
    pub build: residuum_core::build_info::BuildInfo,
    pub web_search_backend: Option<residuum_config::StandaloneBackendConfig>,
    pub model_catalog: SharedModelCatalog,
    pub a2a_hub: Arc<A2aClientHub>,
    pub a2a_tracker: Arc<RemoteTaskTracker>,
    pub lifecycle: super::LifecycleAccess,
    pub messenger: Arc<AgentMessenger>,
    pub layout: WorkspaceLayout,
    pub config_dir: PathBuf,
    pub hub_dir: PathBuf,
    pub tz: chrono_tz::Tz,
    pub depth_cap: u32,
    pub auto_mode: Option<SharedAutoMode>,
    pub mailboxes: Option<Arc<dyn residuum_workspace::inbox::mail::MailboxDirectory>>,
}

impl SessionToolFactory for SessionTools {
    fn build(&self, scope: SessionToolScope) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.set_agent_keys(Arc::clone(&self.agent_keys));
        registration::attach_result_store(&mut registry, &self.hub_dir, &self.layout);
        let injections = RegistryInjections {
            tools_path: Some(Arc::clone(&self.tools_path)),
            checkpoints: Some(Arc::clone(&self.checkpoints)),
            publisher: Some(self.publisher.clone()),
            mailboxes: self.mailboxes.clone(),
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

fn register_session_workspace_tools(
    tools: &mut ToolRegistry,
    injections: &RegistryInjections,
    shared: &SessionTools,
    scope: &SessionToolScope,
) {
    let diagnostics_paths = residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
        config_dir: shared.config_dir.clone(),
        workspace_dir: shared.layout.root().to_path_buf(),
        hub_dir: shared.hub_dir.clone(),
    };
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
    registration::register_skill_tools(tools, injections, Arc::clone(&scope.skill_state));
    registration::register_memory_tools(
        tools,
        Arc::clone(&shared.hybrid_searcher),
        shared.layout.episodes_dir(),
        shared.layout.sessions_dir(),
    );
    registration::register_inbox_tools(tools, injections, &shared.layout, shared.tz);
    registration::register_workspace_checkpoint_tools(
        tools,
        Arc::clone(&shared.checkpoints),
        Arc::clone(&shared.path_policy),
    );
}

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

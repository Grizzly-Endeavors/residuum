use std::sync::Arc;

use tokio::sync::mpsc;

use residuum_agent::Agent;
use residuum_background::SessionRuntime;
use residuum_background::SpawnContext;
use residuum_background::registry::SessionRegistry;
use residuum_background::store::SessionStore;
use residuum_bus::control::{ServerCommand, StopRequest};
use residuum_bus::{
    BusHandle, EndpointName, EndpointRegistry, MessageEvent, Publisher, Subscriber,
};
use residuum_config::{Config, HubConfig, WorkspaceLayout};
use residuum_inference::SharedHttpClient;
use residuum_mcp::SharedMcpRegistry;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_memory::search::HybridSearcher;
use residuum_proactivity::actions::store::ActionStore;
use residuum_proactivity::pulse::scheduler::PulseScheduler;
use residuum_skills::SharedSkillState;
use residuum_tracing_service::TracingService;

pub(crate) struct GatewayCore {
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    pub retry_tx: mpsc::Sender<RetryTurnRequest>,
    pub config_dir: std::path::PathBuf,
    pub hub_dir: std::path::PathBuf,
    pub bus_handle: BusHandle,
    pub publisher: Publisher,
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
    pub pulse_fires: Arc<crate::pulse_fires::PulseFires>,
}

pub(crate) struct RetryTurnRequest {
    pub id: String,
    pub result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
}

pub(crate) struct CoreReceivers {
    pub reload: residuum_bus::control::ReloadReceiver,
    pub command: mpsc::Receiver<ServerCommand>,
    pub stop: mpsc::Receiver<StopRequest>,
    pub retry: mpsc::Receiver<RetryTurnRequest>,
}

impl GatewayCore {
    pub fn new(
        config_dir: std::path::PathBuf,
        hub_dir: std::path::PathBuf,
    ) -> (Self, CoreReceivers) {
        let (reload_tx, reload_rx) =
            tokio::sync::mpsc::unbounded_channel::<residuum_bus::control::ReloadSignal>();
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let (stop_tx, stop_rx) = mpsc::channel::<StopRequest>(8);
        let (retry_tx, retry_rx) = mpsc::channel::<RetryTurnRequest>(8);
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();

        let core = Self {
            reload_tx,
            command_tx,
            stop_tx,
            retry_tx,
            config_dir,
            hub_dir,
            bus_handle,
            publisher,
            queued: Arc::default(),
            pulse_fires: Arc::default(),
        };
        let receivers = CoreReceivers {
            reload: reload_rx,
            command: command_rx,
            stop: stop_rx,
            retry: retry_rx,
        };
        (core, receivers)
    }
}

#[derive(Clone)]
pub(crate) struct GatewayState {
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    pub retry_tx: mpsc::Sender<RetryTurnRequest>,
    pub agent_inbox_dir: std::path::PathBuf,
    pub uploads: residuum_hub::uploads::UploadStore,
    pub tz: chrono_tz::Tz,
    pub publisher: Publisher,
    pub bus_handle: BusHandle,
    pub file_registry: residuum_hub::file_server::FileRegistry,
    pub webhooks: residuum_interfaces::webhook::WebhookTable,
    pub session_registry: Arc<SessionRegistry>,
    pub session_store: Arc<SessionStore>,
    pub agent_messenger: Arc<residuum_background::messaging::AgentMessenger>,
    pub skill_state: SharedSkillState,
    pub workspace_watch_health:
        tokio::sync::watch::Receiver<residuum_workspace::watch::WatchHealth>,
    pub team_feed: Arc<residuum_hub::services::TeamChangeFeed>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub turn_journal: crate::turn_journal::TurnJournal,
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
    pub layout: WorkspaceLayout,
    pub activity: Arc<residuum_hub::activity::ActivityTracker>,
    pub pulse_fires: Arc<crate::pulse_fires::PulseFires>,
}

pub(crate) struct AgentRuntime {
    pub name: String,
    pub services: residuum_hub::services::HubServices,
    pub build: residuum_core::build_info::BuildInfo,
    pub cfg: Config,
    pub hub_cfg: HubConfig,
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    pub tool_injections: residuum_tools::registration::RegistryInjections,
    pub observer: Arc<Observer>,
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub subconscious: Arc<residuum_agent::subconscious::Subconscious>,
    pub learning_state: Arc<std::sync::Mutex<residuum_agent::subconscious::LearningState>>,
    pub memory_worker: Arc<crate::post_turn::MemoryWorker>,
    pub post_turn_subconscious: Arc<crate::post_turn::SubconsciousWorker>,
    pub post_turn_result_rx: mpsc::UnboundedReceiver<crate::post_turn::PostTurnResult>,
    pub deferred_inbound: std::collections::VecDeque<residuum_bus::MessageEvent>,
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    pub agent_messenger: Arc<residuum_background::messaging::AgentMessenger>,
    pub conversation_router: Arc<residuum_background::ConversationRouter>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub tools_path: residuum_config::SharedToolsPath,
    pub agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    pub skill_state: SharedSkillState,
    pub pulse_enabled: bool,
    pub notify_handles: Vec<tokio::task::JoinHandle<()>>,
    pub channel_configs: Vec<residuum_config::ExternalChannelConfig>,
    pub webhooks: residuum_interfaces::webhook::WebhookTable,
    pub bus_infra_handles: Vec<tokio::task::JoinHandle<()>>,
    pub http_client: SharedHttpClient,
    pub spawn_context: Arc<SpawnContext>,
    pub spawn_context_tx: tokio::sync::watch::Sender<Arc<SpawnContext>>,
    pub model_call_resources_tx:
        tokio::sync::watch::Sender<Arc<crate::web::model::ModelCallResources>>,
    pub bus_handle: BusHandle,
    pub publisher: Publisher,
    pub agent_subscriber: Subscriber<MessageEvent>,
    pub endpoint_registry: EndpointRegistry,
    pub error_subscriber: Subscriber<residuum_bus::ErrorEvent>,
    pub pulse_result_subscriber: Subscriber<residuum_bus::AgentResultEvent>,
    pub pulse_fire_subscriber: Subscriber<residuum_bus::PulseFireEvent>,
    pub last_output_endpoint: Option<EndpointName>,
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<EndpointName>>,
    pub reload_rx: residuum_bus::control::ReloadReceiver,
    pub command_rx: mpsc::Receiver<ServerCommand>,
    pub stop_rx: mpsc::Receiver<StopRequest>,
    pub retry_rx: mpsc::Receiver<RetryTurnRequest>,
    pub agent_stop_rx: mpsc::Receiver<()>,
    pub pulse_scheduler: PulseScheduler,
    pub pulse_fires: Arc<crate::pulse_fires::PulseFires>,
    pub config_dir: std::path::PathBuf,
    pub hub_dir: std::path::PathBuf,
    pub last_user_message_instant: Option<tokio::time::Instant>,
    pub chat_adapters: super::chat_adapters::ChatAdapters,
    pub a2a: Option<residuum_a2a::AgentA2a>,
    pub a2a_router_tx: tokio::sync::watch::Sender<Option<axum::Router>>,
    pub reload_done_tx: tokio::sync::watch::Sender<u64>,
    pub a2a_hub: Arc<residuum_a2a::A2aClientHub>,
    pub a2a_tracker: Arc<residuum_a2a::RemoteTaskTracker>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub config_reload_tracker: residuum_tools::SharedConfigReloadTracker,
    pub watcher_handle: Option<tokio::task::JoinHandle<()>>,
    pub root_config_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    pub change_feed_handle: Option<tokio::task::JoinHandle<()>>,
    pub workbench_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    pub path_policy: residuum_tools::SharedPathPolicy,
    pub lifecycle: residuum_tools::LifecycleAccess,
    pub auto_mode: residuum_agent::auto_mode::SharedAutoMode,
    pub tracing_service: Arc<TracingService>,
    pub activity: Arc<residuum_hub::activity::ActivityTracker>,
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        self.a2a_tracker.shutdown();
        self.pulse_fires.stop_all();
        for handle in self
            .notify_handles
            .drain(..)
            .chain(self.bus_infra_handles.drain(..))
            .chain(self.watcher_handle.take())
            .chain(self.root_config_watcher_handle.take())
            .chain(self.change_feed_handle.take())
            .chain(self.workbench_watcher_handle.take())
        {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::control::ReloadSignal;

    #[tokio::test]
    async fn core_channels_survive_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let (core, mut receivers) =
            GatewayCore::new(dir.path().to_path_buf(), dir.path().to_path_buf());

        let system_topic = || {
            residuum_bus::topics::Notification(residuum_bus::NotifyName::from(
                residuum_bus::SYSTEM_CHANNEL,
            ))
        };

        let mut subscriber: residuum_bus::Subscriber<residuum_bus::NoticeEvent> =
            core.bus_handle.subscribe(system_topic()).await.unwrap();

        let result = core
            .publisher
            .publish(system_topic(), residuum_bus::NoticeEvent::new("test"))
            .await;
        assert!(result.is_ok(), "bus publish should succeed before reload");

        let received = subscriber.recv().await.unwrap();
        assert!(
            received.is_some(),
            "notice should be delivered to subscriber"
        );
        assert_eq!(
            received.unwrap().message,
            "test",
            "received message should match published content"
        );

        core.reload_tx.send(ReloadSignal::Agent).unwrap();

        let signal = receivers.reload.recv().await.unwrap();
        assert_eq!(
            signal,
            ReloadSignal::Agent,
            "reload signal should propagate to receiver"
        );

        let result_after = core
            .publisher
            .publish(
                system_topic(),
                residuum_bus::NoticeEvent::new("after reload"),
            )
            .await;
        assert!(
            result_after.is_ok(),
            "bus publish should still work after reload signal"
        );

        let received_after = subscriber.recv().await.unwrap();
        assert!(
            received_after.is_some(),
            "notice should be delivered after reload signal"
        );
    }
}

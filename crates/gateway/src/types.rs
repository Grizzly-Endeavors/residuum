//! Core types for the gateway module.

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

/// One agent's long-lived communication channels and bus.
///
/// Created when the agent starts and persists across configuration reloads.
/// The senders are cloned into adapters, the web server, and event loop state.
pub(crate) struct GatewayCore {
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    /// The agent's own `config/` directory (`~/.residuum/<agent>/config`).
    pub config_dir: std::path::PathBuf,
    /// The hub's directory (`~/.residuum/hub`).
    pub hub_dir: std::path::PathBuf,
    pub bus_handle: BusHandle,
    pub publisher: Publisher,
    /// People's messages waiting for the main agent to read them, which the
    /// web UI can withdraw or have sent now.
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
}

/// Receiver halves consumed by the event loop.
pub(crate) struct CoreReceivers {
    pub reload: residuum_bus::control::ReloadReceiver,
    pub command: mpsc::Receiver<ServerCommand>,
    pub stop: mpsc::Receiver<StopRequest>,
}

impl GatewayCore {
    /// Create a new gateway core with fresh channels.
    pub fn new(
        config_dir: std::path::PathBuf,
        hub_dir: std::path::PathBuf,
    ) -> (Self, CoreReceivers) {
        let (reload_tx, reload_rx) =
            tokio::sync::mpsc::unbounded_channel::<residuum_bus::control::ReloadSignal>();
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let (stop_tx, stop_rx) = mpsc::channel::<StopRequest>(8);
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();

        let core = Self {
            reload_tx,
            command_tx,
            stop_tx,
            config_dir,
            hub_dir,
            bus_handle,
            publisher,
            queued: Arc::default(),
        };
        let receivers = CoreReceivers {
            reload: reload_rx,
            command: command_rx,
            stop: stop_rx,
        };
        (core, receivers)
    }
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
pub(crate) struct GatewayState {
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    pub agent_inbox_dir: std::path::PathBuf,
    pub tz: chrono_tz::Tz,
    pub publisher: Publisher,
    pub bus_handle: BusHandle,
    pub file_registry: residuum_hub::file_server::FileRegistry,
    /// Named webhooks served at `/webhook/{name}`; swapped in place on config reload.
    pub webhooks: residuum_interfaces::webhook::WebhookTable,
    /// Live agent sessions, for the sessions listing and Activity's stop
    /// command.
    pub session_registry: Arc<SessionRegistry>,
    /// Durable record of every session run, for the listing and transcripts.
    pub session_store: Arc<SessionStore>,
    /// Delivers the owner's messages (from a session panel) and artifacts' messages to
    /// sessions.
    pub agent_messenger: Arc<residuum_background::messaging::AgentMessenger>,
    /// The skill index, for checking the skill an artifact's session names.
    pub skill_state: SharedSkillState,
    /// Whether the workspace change feed is running, so a connection that
    /// starts watching can be told when live updates are off.
    pub workspace_watch_health:
        tokio::sync::watch::Receiver<residuum_workspace::watch::WatchHealth>,
    /// The hub's one change feed over the team directory, which a connection
    /// watching `team/...` paths reads.
    pub team_feed: Arc<residuum_hub::services::TeamChangeFeed>,
    /// Pending one-off scheduled actions, for the Schedule place's listing
    /// and cancel button.
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    /// The main conversation's turn in flight, which every connection
    /// follows the conversation through and can ask to be shown.
    pub turn_journal: crate::turn_journal::TurnJournal,
    /// People's messages waiting for the main agent, for `withdraw_queued`
    /// and `send_queued_now`.
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
    /// Workspace layout, for the Schedule place's reads of HEARTBEAT.yml,
    /// `pulse_state.json`, and its in-place edits to HEARTBEAT.yml.
    pub layout: WorkspaceLayout,
    /// Main-conversation activity for the rail and Home; a WebSocket
    /// connection registers itself here so unread counts reset while a client
    /// is watching.
    pub activity: Arc<residuum_hub::activity::ActivityTracker>,
}

/// All state one agent needs to run: its subsystems, its channels, and the
/// hub services it was started with.
pub(crate) struct AgentRuntime {
    /// The agent's name: its directory name and identity everywhere.
    pub name: String,
    /// Shared services the hub passed in.
    pub services: residuum_hub::services::HubServices,
    /// The running build, read by the bug-report and feedback client contexts.
    pub build: residuum_core::build_info::BuildInfo,
    // Current running config (for diffing on reload)
    pub cfg: Config,
    /// Current running hub config, reloaded independently of `cfg` — see
    /// `gateway::reload::handle_hub_reload`.
    pub hub_cfg: HubConfig,
    // Subsystems (from initialization)
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    /// What main's tool registry injected into its tools at registration,
    /// so a reload that re-registers one of them (the timezone tools)
    /// injects the same handles.
    pub tool_injections: residuum_tools::registration::RegistryInjections,
    /// `Arc`-shared with the post-turn background worker (see
    /// `crate::post_turn`) so an automatic observe cycle's LLM call
    /// can run off the event loop while a config reload still safely swaps
    /// the whole instance in place — `Observer`'s own provider/config are
    /// interior-mutable for exactly this.
    pub observer: Arc<Observer>,
    /// The single serialized writer for global memory: episode id
    /// allocation, the observation log, the search index, embedding, and
    /// the reflector trigger. Shared with every session run's completion
    /// pipeline through `spawn_context`.
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub subconscious: Arc<residuum_agent::subconscious::Subconscious>,
    /// In-memory learning-loop state (cooldown + fallback turn counter). Resets
    /// on restart. `Arc<Mutex<_>>` so the post-turn background worker (see
    /// `crate::post_turn`) and the main loop's own turn-count
    /// fallback can both reach it without racing.
    pub learning_state: Arc<std::sync::Mutex<residuum_agent::subconscious::LearningState>>,
    /// Background worker for the memory cycle: automatic observation
    /// (including the idle transition's) and the `/observe` and `/reflect`
    /// commands — see `crate::post_turn`.
    pub memory_worker: Arc<crate::post_turn::MemoryWorker>,
    /// Background worker for the end-of-turn subconscious evaluation.
    pub post_turn_subconscious: Arc<crate::post_turn::SubconsciousWorker>,
    /// Results from both post-turn workers above, applied on the main loop
    /// (see `run_loop`'s own select branch) — the `Agent`-touching tail
    /// neither worker can run for itself.
    pub post_turn_result_rx: mpsc::UnboundedReceiver<crate::post_turn::PostTurnResult>,
    /// Messages that reached the main conversation after a turn's last
    /// checkpoint drain (see `process_leftover_interrupts`). The event loop
    /// runs them as turns before it waits on anything else.
    pub deferred_inbound: std::collections::VecDeque<residuum_bus::MessageEvent>,
    /// People's messages waiting for the agent to read them: on a turn's
    /// interrupt channel or in `deferred_inbound`. Asked before either hands
    /// one on, so a withdrawn message is dropped and one to send now is held
    /// for the next turn.
    pub queued: Arc<crate::queued_messages::QueuedMessages>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    /// Routes `message_agent` deliveries by address. Constant for the
    /// agent's lifetime — cloned into `spawn_context` on every reload.
    pub agent_messenger: Arc<residuum_background::messaging::AgentMessenger>,
    /// Routes admitted inbound conversation messages that aren't the owner's
    /// own DM to their conversation's session. Constant for the agent's
    /// lifetime, like `agent_messenger`.
    pub conversation_router: Arc<residuum_background::ConversationRouter>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    /// Shared, reloadable effective `PATH` for spawned children (exec + MCP stdio).
    pub tools_path: residuum_config::SharedToolsPath,
    /// The hub's shared agent key store (exec, MCP, trace redaction, sessions).
    pub agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    pub skill_state: SharedSkillState,
    pub pulse_enabled: bool,
    pub notify_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Notification channels from `channels.toml`, kept to rebuild the endpoint registry on reload.
    pub channel_configs: Vec<residuum_config::ExternalChannelConfig>,
    /// Shared with the agent's webhook route; replaced on config reload.
    pub webhooks: residuum_interfaces::webhook::WebhookTable,
    /// Bus infrastructure handles (bridge, result router, registry) — not restarted on reload.
    pub bus_infra_handles: Vec<tokio::task::JoinHandle<()>>,
    pub http_client: SharedHttpClient,
    pub spawn_context: Arc<SpawnContext>,
    /// Pushes a fresh `ModelCallResources` to the model-call HTTP endpoint on
    /// every config reload, alongside `spawn_context`, so `POST
    /// /api/agents/{name}/model/complete` resolves providers from the current config
    /// without the HTTP router being rebuilt.
    pub model_call_resources_tx:
        tokio::sync::watch::Sender<Arc<crate::web::model::ModelCallResources>>,
    // Runtime channels + handles
    /// Bus handle for creating publishers/subscribers.
    pub bus_handle: BusHandle,
    /// Publisher for sending events onto the bus.
    pub publisher: Publisher,
    /// Typed subscriber for receiving inbound user messages from the bus.
    pub agent_subscriber: Subscriber<MessageEvent>,
    /// Endpoint registry for looking up configured endpoints.
    pub endpoint_registry: EndpointRegistry,
    /// Typed subscriber for error events from the system notification channel.
    pub error_subscriber: Subscriber<residuum_bus::ErrorEvent>,
    /// A second, independent subscription to the `Background` topic (the bus
    /// fans results out per-subscriber, so this doesn't interfere with
    /// `background::listener`'s own subscription) used only to record each
    /// completed pulse's delivered output into `pulse_scheduler` for
    /// `context_from` — see
    /// `crate::event_loop::pulse::handle_pulse_result_event`. Kept
    /// separate from `pulse_scheduler` mutation elsewhere: results arrive
    /// asynchronously, while every other `pulse_scheduler` access happens
    /// synchronously inside the event loop's `select!`, so a second
    /// subscriber (rather than sharing the scheduler behind a lock) is what
    /// lets this stay single-owned.
    pub pulse_result_subscriber: Subscriber<residuum_bus::AgentResultEvent>,
    /// Endpoint that last sent a message (for background turn response routing).
    pub last_output_endpoint: Option<EndpointName>,
    /// Sender for clearing the output endpoint override on user message.
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<EndpointName>>,
    pub reload_rx: residuum_bus::control::ReloadReceiver,
    pub command_rx: mpsc::Receiver<ServerCommand>,
    /// Stop requests for the currently running turn. Watched by the outer
    /// event loop when idle (responds "nothing running") and by the active
    /// turn's own select loop while a turn is in progress.
    pub stop_rx: mpsc::Receiver<StopRequest>,
    /// The hub asking this agent to stop (stop, restart, or hub shutdown).
    /// Watched by the outer loop and by the active turn, which it interrupts
    /// the way a user stop does.
    pub agent_stop_rx: mpsc::Receiver<()>,
    pub pulse_scheduler: PulseScheduler,
    /// Stops every pulse task script still running when the agent stops:
    /// each one's process tree is killed and its fire delivers nothing (see
    /// `residuum_proactivity::pulse::script::run_pulse_scripts`).
    pub pulse_scripts_stop: tokio_util::sync::CancellationToken,
    /// Path to the agent's own `config/` directory (for backup/rollback during reload).
    pub config_dir: std::path::PathBuf,
    /// Path to the hub's directory (`~/.residuum/hub`).
    pub hub_dir: std::path::PathBuf,
    /// When the last user message was received (for idle deadline recalculation on reload).
    pub last_user_message_instant: Option<tokio::time::Instant>,
    /// Discord, Telegram, and Teams, addressed by name for reload and shutdown.
    pub chat_adapters: super::chat_adapters::ChatAdapters,
    /// The agent's A2A server state: its card and the router the hub's A2A
    /// listener serves for it. `None` when A2A is disabled.
    pub a2a: Option<residuum_a2a::AgentA2a>,
    /// Where the hub's A2A listener finds this agent's current A2A router.
    pub a2a_router_tx: tokio::sync::watch::Sender<Option<axum::Router>>,
    /// Counts finished config reloads, for the hub to wait on.
    pub reload_done_tx: tokio::sync::watch::Sender<u64>,
    /// Remote A2A agents this agent's client can reach, loaded from
    /// `config/a2a.json` and reloaded on every workspace config change.
    pub a2a_hub: Arc<residuum_a2a::A2aClientHub>,
    /// Outbound A2A tasks this agent started on other agents.
    pub a2a_tracker: Arc<residuum_a2a::RemoteTaskTracker>,
    /// This agent's workspace and config checkpoint repositories, plus the
    /// team and hub-config ones every agent shares.
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    /// Tracks the agent's own config-file writes so the reload each one
    /// triggers can report back into its transcript instead of only
    /// reaching the user's interfaces.
    pub config_reload_tracker: residuum_tools::SharedConfigReloadTracker,
    pub watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// Polls `config.toml`/`providers.toml` for changes made outside the web
    /// config API (the agent's own `write_file`/`edit_file`, or a manual
    /// edit) and signals a root reload.
    pub root_config_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// The workspace change feed (one recursive watcher over the workspace).
    pub change_feed_handle: Option<tokio::task::JoinHandle<()>>,
    /// Derives artifact reloads from the change feed.
    pub workbench_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// Cloned core senders for rebuilding adapters on reload.
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    /// Shared path policy for updating blocked paths on reload.
    pub path_policy: residuum_tools::SharedPathPolicy,
    /// How this agent's tools reach the hub, for the session tool factory
    /// each config reload rebuilds.
    pub lifecycle: residuum_tools::LifecycleAccess,
    /// Shared Auto Mode, updated with the agent's `[auto_mode]` on reload.
    pub auto_mode: residuum_agent::auto_mode::SharedAutoMode,
    /// Shared tracing service for observability API.
    pub tracing_service: Arc<TracingService>,
    /// Main-conversation activity for the rail and Home.
    pub activity: Arc<residuum_hub::activity::ActivityTracker>,
}

impl Drop for AgentRuntime {
    /// A runtime dropped without [`graceful_shutdown`](super::event_loop) —
    /// its event loop panicked — must not leave its tasks running against a
    /// bus nobody reads. The chat adapters stop themselves (see
    /// [`ChatAdapters`](super::chat_adapters::ChatAdapters)); this aborts the
    /// rest.
    fn drop(&mut self) {
        self.a2a_tracker.shutdown();
        self.pulse_scripts_stop.cancel();
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

        // Subscribe before publishing so we can verify delivery
        let mut subscriber: residuum_bus::Subscriber<residuum_bus::NoticeEvent> =
            core.bus_handle.subscribe(system_topic()).await.unwrap();

        let result = core
            .publisher
            .publish(system_topic(), residuum_bus::NoticeEvent::new("test"))
            .await;
        assert!(result.is_ok(), "bus publish should succeed before reload");

        // Verify notice was delivered
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

        // Fire a reload signal
        core.reload_tx.send(ReloadSignal::Agent).unwrap();

        // Verify the reload signal propagated
        let signal = receivers.reload.recv().await.unwrap();
        assert_eq!(
            signal,
            ReloadSignal::Agent,
            "reload signal should propagate to receiver"
        );

        // Channels still work after the reload signal
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

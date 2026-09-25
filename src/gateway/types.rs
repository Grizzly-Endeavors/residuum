//! Core types for the gateway module.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::SessionRuntime;
use crate::background::registry::SessionRegistry;
use crate::background::spawn_context::SpawnContext;
use crate::background::store::SessionStore;
use crate::bus::{BusHandle, EndpointName, EndpointRegistry, MessageEvent, Publisher, Subscriber};
use crate::config::Config;
use crate::inference::SharedHttpClient;
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::memory::search::HybridSearcher;
use crate::pulse::scheduler::PulseScheduler;
use crate::skills::SharedSkillState;
use crate::tracing_service::TracingService;
use crate::tunnel::TunnelStatus;
use crate::update::SharedUpdateStatus;
use crate::workspace::layout::WorkspaceLayout;

/// Describes what kind of configuration reload was requested.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ReloadSignal {
    /// No reload pending.
    #[default]
    None,
    /// Full root config reload (config.toml changed).
    Root,
    /// Workspace-level reload (`mcp.json`, `channels.toml`, or
    /// `agent-card.json` changed).
    Workspace,
}

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
    /// Restart requested (binary updated, re-exec needed).
    Restart,
}

/// Which shutdown trigger interrupted a running turn.
///
/// A turn blocks the event loop's own `select!` for its whole duration, so
/// the trigger is observed and reacted to (stopping the turn) from inside
/// the turn's own select loop instead — see `run_agent_turn_with_interrupts`
/// in `gateway/event_loop/turns.rs`. That already consumes the underlying
/// signal, so this is bubbled back up to the event loop instead of it
/// re-observing the same signal a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownReason {
    /// SIGTERM (Unix) or the platform termination signal.
    Sigterm,
    /// Shutdown requested via the HTTP `/api/shutdown` endpoint.
    GatewayShutdown,
    /// Restart requested (binary updated, re-exec needed).
    Restart,
}

/// Platform-aware termination signal.
///
/// On Unix, wraps a SIGTERM listener. On Windows (and other platforms), `recv()`
/// pends forever — graceful shutdown is handled via the HTTP `/api/shutdown` endpoint
/// or the cross-platform Ctrl+C handler instead.
pub struct TermSignal {
    #[cfg(unix)]
    inner: tokio::signal::unix::Signal,
}

impl TermSignal {
    /// Register the SIGTERM listener.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the signal handler cannot be registered.
    #[cfg(unix)]
    pub fn new() -> std::io::Result<Self> {
        let inner = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        Ok(Self { inner })
    }

    /// Create the termination signal. Nothing to register on this platform.
    #[cfg(not(unix))]
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// Wait for the termination signal. On non-Unix platforms this never resolves.
    #[cfg(unix)]
    pub async fn recv(&mut self) {
        self.inner.recv().await;
    }

    /// Wait for the termination signal. On non-Unix platforms this never resolves.
    #[cfg(not(unix))]
    pub async fn recv(&mut self) {
        std::future::pending::<()>().await;
    }
}

/// A named command dispatched from any client channel to the server event loop.
pub struct ServerCommand {
    /// Command name (e.g. "observe", "reflect", "context").
    pub name: String,
    /// Optional argument text.
    pub args: Option<String>,
    /// Optional oneshot sender for routing a response back to the sender only.
    ///
    /// `Ok` carries a successful command's reply text (e.g. "context"); the
    /// WS layer intentionally ignores it since the outcome is already
    /// reflected via the normal bus broadcast. `Err` carries a rejection
    /// reason (e.g. an unknown command name) and is routed to the sender as
    /// an error, never broadcast to other clients.
    pub reply_tx: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
}

/// A request to stop the currently running main-agent turn.
///
/// Delivered outside the `ServerCommand`/`command_tx` pipeline because that
/// pipeline is only drained *between* turns (the event loop blocks on
/// `handle_inbound_message` for the whole turn) — a stop needs to reach a
/// turn while it is running, so it travels its own channel that the active
/// turn's select loop watches directly.
pub struct StopRequest {
    /// Correlation id of the turn to stop, when the sender knows it (e.g. a
    /// WebSocket client stopping the turn it's watching). `None` means "stop
    /// whichever turn is currently running" — used by chat commands, which
    /// don't track turn ids and only ever have at most one turn to stop.
    pub reply_to: Option<String>,
    /// Reports whether a running turn actually matched and was signalled to
    /// stop, so the caller can tell a successful stop from "nothing was
    /// running" without guessing from a timeout.
    pub result_tx: Option<tokio::sync::oneshot::Sender<bool>>,
}

/// Long-lived core that owns shared communication channels.
///
/// Created once at startup and persists across configuration reloads.
/// The senders are cloned into adapters, the web server, and event loop state.
pub(crate) struct GatewayCore {
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    /// Dedicated shutdown signal for the HTTP server (not tied to reload).
    pub http_shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub config_dir: std::path::PathBuf,
    pub bus_handle: BusHandle,
    pub publisher: Publisher,
}

/// Receiver halves consumed by the event loop.
pub(crate) struct CoreReceivers {
    pub reload: tokio::sync::watch::Receiver<ReloadSignal>,
    pub command: mpsc::Receiver<ServerCommand>,
    pub stop: mpsc::Receiver<StopRequest>,
}

impl GatewayCore {
    /// Create a new gateway core with fresh channels.
    pub fn new(config_dir: std::path::PathBuf) -> (Self, CoreReceivers) {
        let (reload_tx, reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let (stop_tx, stop_rx) = mpsc::channel::<StopRequest>(8);
        let http_shutdown_tx = tokio::sync::watch::channel::<bool>(false).0;
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();

        let core = Self {
            reload_tx,
            command_tx,
            stop_tx,
            http_shutdown_tx,
            config_dir,
            bus_handle,
            publisher,
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
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    pub agent_inbox_dir: std::path::PathBuf,
    pub tz: chrono_tz::Tz,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    pub publisher: Publisher,
    pub bus_handle: BusHandle,
    pub file_registry: crate::gateway::file_server::FileRegistry,
    /// Named webhooks served at `/webhook/{name}`; swapped in place on config reload.
    pub webhooks: crate::interfaces::webhook::WebhookTable,
    /// Live agent sessions, for the sessions listing and the sidebar's stop
    /// command.
    pub session_registry: Arc<SessionRegistry>,
    /// Durable record of every session run, for the listing and transcripts.
    pub session_store: Arc<SessionStore>,
    /// Delivers the sidebar's and artifacts' messages to sessions.
    pub agent_messenger: Arc<crate::background::messaging::AgentMessenger>,
    /// The skill index, for checking the skill an artifact's session names.
    pub skill_state: SharedSkillState,
    /// Whether the workspace change feed is running, so a connection that
    /// starts watching can be told when live updates are off.
    pub workspace_watch_health: tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
    /// Pending one-off scheduled actions, for the Scheduled view's listing
    /// and cancel button.
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    /// Workspace layout, for the Scheduled view's reads of HEARTBEAT.yml,
    /// `pulse_state.json`, and its in-place edits to HEARTBEAT.yml.
    pub layout: WorkspaceLayout,
}

/// All state needed by the main event loop.
pub(crate) struct GatewayRuntime {
    // Current running config (for diffing on reload)
    pub cfg: Config,
    // Subsystems (from initialization)
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    /// `Arc`-shared with the post-turn background worker (see
    /// `crate::gateway::post_turn`) so an automatic observe cycle's LLM call
    /// can run off the event loop while a config reload still safely swaps
    /// the whole instance in place — `Observer`'s own provider/config are
    /// interior-mutable for exactly this.
    pub observer: Arc<Observer>,
    /// The single serialized writer for global memory: episode id
    /// allocation, the observation log, the search index, embedding, and
    /// the reflector trigger. Shared with every session run's completion
    /// pipeline through `spawn_context`.
    pub merge_writer: Arc<MemoryMergeWriter>,
    pub subconscious: Arc<crate::subconscious::Subconscious>,
    /// In-memory learning-loop state (cooldown + fallback turn counter). Resets
    /// on restart. `Arc<Mutex<_>>` so the post-turn background worker (see
    /// `crate::gateway::post_turn`) and the main loop's own turn-count
    /// fallback can both reach it without racing.
    pub learning_state: Arc<std::sync::Mutex<crate::subconscious::LearningState>>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub session_runtime: Arc<SessionRuntime>,
    pub session_registry: Arc<SessionRegistry>,
    /// Durable record of every session run, shared with the HTTP server's
    /// sessions endpoints (rebuilt on a gateway rebind).
    pub session_store: Arc<SessionStore>,
    /// Routes `message_agent` deliveries by address. Constant for the
    /// process lifetime — cloned into `spawn_context` on every reload.
    pub agent_messenger: Arc<crate::background::messaging::AgentMessenger>,
    /// Routes admitted inbound conversation messages that aren't the owner's
    /// own DM to their conversation's session. Constant for the process
    /// lifetime, like `agent_messenger`.
    pub conversation_router: Arc<crate::background::ConversationRouter>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    /// Shared, reloadable effective `PATH` for spawned children (exec + MCP stdio).
    pub tools_path: crate::tools::SharedToolsPath,
    /// Shared agent key store (exec, MCP, trace redaction, sessions).
    pub agent_keys: crate::agent_keys::SharedAgentKeys,
    pub skill_state: SharedSkillState,
    pub pulse_enabled: bool,
    pub notify_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Notification channels from `channels.toml`, kept to rebuild the endpoint registry on reload.
    pub channel_configs: Vec<crate::notify::types::ExternalChannelConfig>,
    /// Shared with the HTTP server's webhook route; replaced on config reload.
    pub webhooks: crate::interfaces::webhook::WebhookTable,
    /// Bus infrastructure handles (bridge, result router, registry) — not restarted on reload.
    pub bus_infra_handles: Vec<tokio::task::JoinHandle<()>>,
    pub http_client: SharedHttpClient,
    pub spawn_context: Arc<SpawnContext>,
    /// Pushes a fresh `ModelCallResources` to the model-call HTTP endpoint on
    /// every config reload, alongside `spawn_context`, so `POST
    /// /api/model/complete` resolves providers from the current config
    /// without the HTTP router being rebuilt.
    pub model_call_resources_tx:
        tokio::sync::watch::Sender<Arc<crate::gateway::web::model::ModelCallResources>>,
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
    pub error_subscriber: Subscriber<crate::bus::ErrorEvent>,
    /// Endpoint that last sent a message (for background turn response routing).
    pub last_output_endpoint: Option<EndpointName>,
    /// Sender for clearing the output endpoint override on user message.
    pub output_topic_override_tx: tokio::sync::watch::Sender<Option<EndpointName>>,
    pub reload_rx: tokio::sync::watch::Receiver<ReloadSignal>,
    pub command_rx: mpsc::Receiver<ServerCommand>,
    /// Stop requests for the currently running turn. Watched by the outer
    /// event loop when idle (responds "nothing running") and by the active
    /// turn's own select loop while a turn is in progress.
    pub stop_rx: mpsc::Receiver<StopRequest>,
    /// Kept alive so the HTTP server task isn't dropped; shut down via `shutdown_tx`.
    pub server_handle: tokio::task::JoinHandle<()>,
    pub pulse_scheduler: PulseScheduler,
    /// Platform termination signal (SIGTERM on Unix, never-resolving on Windows).
    pub sigterm: TermSignal,
    /// Dedicated shutdown signal for the HTTP server.
    pub http_shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Path to the config directory (for backup/rollback during reload).
    pub config_dir: std::path::PathBuf,
    /// When the last user message was received (for idle deadline recalculation on reload).
    pub last_user_message_instant: Option<tokio::time::Instant>,
    // Cloud config for tunnel respawn
    pub cloud_config: Option<crate::config::CloudConfig>,
    // Adapter lifecycle handles
    pub tunnel_handle: Option<tokio::task::JoinHandle<()>>,
    pub tunnel_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub tunnel_status_tx: Arc<tokio::sync::watch::Sender<TunnelStatus>>,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    pub discord_handle: Option<tokio::task::JoinHandle<()>>,
    pub telegram_handle: Option<tokio::task::JoinHandle<()>>,
    pub discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub teams_handle: Option<tokio::task::JoinHandle<()>>,
    pub teams_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub a2a_handle: Option<tokio::task::JoinHandle<()>>,
    pub a2a_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// The live agent card, so a workspace-file reload can update it without
    /// restarting the listener. `None` when A2A is disabled.
    pub a2a_card_state: Option<crate::a2a::SharedCardState>,
    /// Remote A2A agents this instance's client can reach, loaded from
    /// `config/a2a.json` and reloaded on every workspace config change.
    pub a2a_hub: Arc<crate::a2a::A2aClientHub>,
    /// Outbound A2A tasks this instance started on other agents.
    pub a2a_tracker: Arc<crate::a2a::RemoteTaskTracker>,
    /// Workspace and config checkpoint repositories.
    pub checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    /// This instance's current A2A public URL, read by the web settings API. `None` when A2A is disabled.
    pub a2a_public_url: Option<crate::a2a::SharedA2aPublicUrl>,
    pub watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// The workspace change feed (one recursive watcher over the workspace).
    pub change_feed_handle: Option<tokio::task::JoinHandle<()>>,
    /// Whether the change feed is running; handed to the HTTP server's state
    /// again on a gateway rebind.
    pub workspace_watch_health: tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
    /// Derives artifact reloads from the change feed.
    pub workbench_watcher_handle: Option<tokio::task::JoinHandle<()>>,
    /// Whether the workbench artifacts listener is running, and on which port.
    pub workbench_serving: crate::workbench::server::WorkbenchServing,
    /// Stops the workbench artifacts listener, when it is running.
    pub workbench_listener_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Cloned core senders for rebuilding adapters on reload.
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub stop_tx: mpsc::Sender<StopRequest>,
    /// File registry for serving attachments to WebSocket clients.
    pub file_registry: crate::gateway::file_server::FileRegistry,
    /// Shared path policy for updating blocked paths on reload.
    pub path_policy: crate::tools::SharedPathPolicy,
    /// Shared tracing service for observability API.
    pub tracing_service: Arc<TracingService>,
    /// Shared update status for periodic version checking.
    pub update_status: SharedUpdateStatus,
    /// Sender half for triggering restart (cloned into API state on rebind).
    pub restart_tx: mpsc::Sender<()>,
    /// Receives a signal to trigger a graceful restart (binary replaced).
    pub restart_rx: mpsc::Receiver<()>,
    /// Sender half for triggering graceful shutdown from the HTTP API.
    pub gateway_shutdown_tx: mpsc::Sender<()>,
    /// Receives a signal to trigger a graceful shutdown from the HTTP API.
    pub gateway_shutdown_rx: mpsc::Receiver<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_signal_default_is_none() {
        let signal = ReloadSignal::default();
        assert_eq!(signal, ReloadSignal::None);
    }

    #[tokio::test]
    async fn core_channels_survive_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let (core, mut receivers) = GatewayCore::new(dir.path().to_path_buf());

        let system_topic = || {
            crate::bus::topics::Notification(crate::bus::NotifyName::from(
                crate::bus::SYSTEM_CHANNEL,
            ))
        };

        // Subscribe before publishing so we can verify delivery
        let mut subscriber: crate::bus::Subscriber<crate::bus::NoticeEvent> =
            core.bus_handle.subscribe(system_topic()).await.unwrap();

        let result = core
            .publisher
            .publish(
                system_topic(),
                crate::bus::NoticeEvent {
                    message: "test".to_string(),
                },
            )
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
        core.reload_tx.send(ReloadSignal::Root).unwrap();

        // Verify the reload signal propagated
        receivers.reload.changed().await.unwrap();
        let signal = receivers.reload.borrow_and_update().clone();
        assert_eq!(
            signal,
            ReloadSignal::Root,
            "reload signal should propagate to receiver"
        );

        // Channels still work after the reload signal
        let result_after = core
            .publisher
            .publish(
                system_topic(),
                crate::bus::NoticeEvent {
                    message: "after reload".to_string(),
                },
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

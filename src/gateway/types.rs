//! Core types for the gateway module.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::SpawnContext;
use crate::bus::{BusHandle, EndpointName, EndpointRegistry, MessageEvent, Publisher, Subscriber};
use crate::config::Config;
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::memory::search::{HybridSearcher, MemoryIndex};
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, SharedHttpClient};
use crate::projects::activation::SharedProjectState;
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
    /// Workspace-level reload (mcp.json or channels.toml changed).
    Workspace,
}

/// Outcome of the gateway main loop.
pub enum GatewayExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
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
    /// Register the platform termination signal.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the signal handler cannot be registered (Unix only).
    #[cfg(unix)]
    pub fn new() -> std::io::Result<Self> {
        let inner = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        Ok(Self { inner })
    }

    /// Register the platform termination signal.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the signal handler cannot be registered (Unix only).
    #[cfg(not(unix))]
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {})
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
    /// Optional oneshot sender for commands that return a response (e.g. "context").
    pub reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
}

/// Long-lived core that owns shared communication channels.
///
/// Created once at startup and persists across configuration reloads.
/// The senders are cloned into adapters, the web server, and event loop state.
pub(crate) struct GatewayCore {
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
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
}

impl GatewayCore {
    /// Create a new gateway core with fresh channels.
    pub fn new(config_dir: std::path::PathBuf) -> (Self, CoreReceivers) {
        let (reload_tx, reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let http_shutdown_tx = tokio::sync::watch::channel::<bool>(false).0;
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();

        let core = Self {
            reload_tx,
            command_tx,
            http_shutdown_tx,
            config_dir,
            bus_handle,
            publisher,
        };
        let receivers = CoreReceivers {
            reload: reload_rx,
            command: command_rx,
        };
        (core, receivers)
    }
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
pub(crate) struct GatewayState {
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub inbox_dir: std::path::PathBuf,
    pub tz: chrono_tz::Tz,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    pub publisher: Publisher,
    pub bus_handle: BusHandle,
    pub file_registry: crate::gateway::file_server::FileRegistry,
}

/// All state needed by the main event loop.
pub(crate) struct GatewayRuntime {
    // Current running config (for diffing on reload)
    pub cfg: Config,
    // Subsystems (from initialization)
    pub layout: WorkspaceLayout,
    pub tz: chrono_tz::Tz,
    pub agent: Agent,
    pub observer: Observer,
    pub reflector: Reflector,
    pub search_index: Arc<MemoryIndex>,
    pub vector_store: Option<Arc<VectorStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub background_spawner: Arc<BackgroundTaskSpawner>,
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub project_state: SharedProjectState,
    pub skill_state: SharedSkillState,
    pub pulse_enabled: bool,
    pub notify_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Bus infrastructure handles (bridge, result router, registry) — not restarted on reload.
    pub bus_infra_handles: Vec<tokio::task::JoinHandle<()>>,
    pub http_client: SharedHttpClient,
    pub spawn_context: Arc<SpawnContext>,
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
    /// Cloned core senders for rebuilding adapters on reload.
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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

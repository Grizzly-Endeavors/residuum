//! Core types for the gateway module.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::SpawnContext;
use crate::background::types::BackgroundResult;
use crate::config::Config;
use crate::interfaces::types::{ReplyHandle, RoutedMessage};
use crate::mcp::SharedMcpRegistry;
use crate::memory::observer::Observer;
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, SharedHttpClient};
use crate::notify::router::NotificationRouter;
use crate::projects::activation::SharedProjectState;
use crate::pulse::scheduler::PulseScheduler;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::protocol::ServerMessage;

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
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    /// Dedicated shutdown signal for the HTTP server (not tied to reload).
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub config_dir: std::path::PathBuf,
}

/// Receiver halves consumed by the event loop.
pub(crate) struct CoreReceivers {
    pub inbound: mpsc::Receiver<RoutedMessage>,
    pub reload: tokio::sync::watch::Receiver<ReloadSignal>,
    pub command: mpsc::Receiver<ServerCommand>,
}

impl GatewayCore {
    /// Create a new gateway core with fresh channels.
    pub fn new(config_dir: std::path::PathBuf) -> (Self, CoreReceivers) {
        let (inbound_tx, inbound_rx) = mpsc::channel::<RoutedMessage>(32);
        let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ServerMessage>(256);
        let (reload_tx, reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (command_tx, command_rx) = mpsc::channel::<ServerCommand>(32);
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let core = Self {
            inbound_tx,
            broadcast_tx,
            reload_tx,
            command_tx,
            shutdown_tx,
            config_dir,
        };
        let receivers = CoreReceivers {
            inbound: inbound_rx,
            reload: reload_rx,
            command: command_rx,
        };
        (core, receivers)
    }
}

/// Shared state for the axum WebSocket server.
#[derive(Clone)]
pub(crate) struct GatewayState {
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    pub inbox_dir: std::path::PathBuf,
    pub tz: chrono_tz::Tz,
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
    pub action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub action_notify: Arc<tokio::sync::Notify>,
    pub mcp_registry: SharedMcpRegistry,
    pub project_state: SharedProjectState,
    pub skill_state: SharedSkillState,
    pub pulse_enabled: bool,
    pub notification_router: Arc<NotificationRouter>,
    pub http_client: SharedHttpClient,
    pub background_spawner: Arc<BackgroundTaskSpawner>,
    pub background_result_rx: mpsc::Receiver<BackgroundResult>,
    pub spawn_context: Arc<SpawnContext>,
    // Runtime channels + handles
    pub inbound_rx: mpsc::Receiver<RoutedMessage>,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
    pub reload_rx: tokio::sync::watch::Receiver<ReloadSignal>,
    pub command_rx: mpsc::Receiver<ServerCommand>,
    /// Kept alive so the HTTP server task isn't dropped; shut down via `shutdown_tx`.
    pub server_handle: tokio::task::JoinHandle<()>,
    pub pulse_scheduler: PulseScheduler,
    /// SIGTERM signal listener for daemon stop support.
    pub sigterm: tokio::signal::unix::Signal,
    /// Dedicated shutdown signal for the HTTP server.
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Path to the config directory (for backup/rollback during reload).
    pub config_dir: std::path::PathBuf,
    /// Most recent reply handle from a user message. Used by wake turns to
    /// deliver responses to the channel the user last interacted from.
    pub last_reply: Option<Arc<dyn ReplyHandle>>,
    /// Unsolicited send handles keyed by interface name. Populated on first
    /// message from each interface for use during idle channel switching.
    pub unsolicited_handles: HashMap<String, Arc<dyn ReplyHandle>>,
    /// When the last user message was received (for idle deadline recalculation on reload).
    pub last_user_message_instant: Option<tokio::time::Instant>,
    // Adapter lifecycle handles
    #[expect(
        dead_code,
        reason = "kept alive so tunnel task is not dropped on shutdown"
    )]
    pub tunnel_handle: Option<tokio::task::JoinHandle<()>>,
    pub discord_handle: Option<tokio::task::JoinHandle<()>>,
    pub telegram_handle: Option<tokio::task::JoinHandle<()>>,
    pub discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Cloned core senders for rebuilding adapters on reload.
    pub inbound_tx: mpsc::Sender<RoutedMessage>,
    pub reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub command_tx: mpsc::Sender<ServerCommand>,
    /// Shared path policy for updating blocked paths on reload.
    pub path_policy: crate::tools::SharedPathPolicy,
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
        let (core, receivers) = GatewayCore::new(dir.path().to_path_buf());

        // Send a message through inbound before reload
        assert!(
            core.inbound_tx
                .send(crate::interfaces::types::RoutedMessage {
                    message: crate::interfaces::types::InboundMessage {
                        id: "test-1".to_string(),
                        content: "hello".to_string(),
                        origin: crate::interfaces::types::MessageOrigin {
                            interface: "test".to_string(),
                            sender_name: "tester".to_string(),
                            sender_id: "t1".to_string(),
                        },
                        timestamp: chrono::Utc::now(),
                        images: vec![],
                    },
                    reply: std::sync::Arc::new(crate::interfaces::websocket::WsReplyHandle::new(
                        core.broadcast_tx.clone(),
                        "test-1".to_string(),
                    ),),
                })
                .await
                .is_ok(),
            "inbound send should succeed before reload"
        );

        // Fire a reload signal
        core.reload_tx.send(ReloadSignal::Root).unwrap();

        // Channels still work after the reload signal
        let _broadcast_rx = core.broadcast_tx.subscribe();
        assert!(
            core.broadcast_tx
                .send(crate::gateway::protocol::ServerMessage::Pong)
                .is_ok(),
            "broadcast should still work after reload signal"
        );

        // Inbound receiver still has the message
        drop(core);
        let mut inbound = receivers.inbound;
        let msg = inbound.recv().await.unwrap();
        assert_eq!(msg.message.content, "hello");
    }
}

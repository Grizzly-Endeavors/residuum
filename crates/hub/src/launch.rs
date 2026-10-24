//! How the hub starts an agent.
//!
//! The hub hands an [`AgentLauncher`] the [`AgentStartInputs`] of one agent
//! and gets back a [`StartedAgent`]: the [`AgentControl`] the hub keeps (the
//! agent's HTTP and A2A routers, its reload channel, its stop signal and the
//! [`AgentCleanup`] for a crashed run) and the agent's event loop, which the
//! host runs as its own task under a supervisor until it ends with an
//! [`AgentExit`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use residuum_bus::control::ReloadSender;
use residuum_config::HubConfig;
use residuum_core::util::FatalError;

use super::activity::ActivityTracker;
use super::services::HubServices;

/// Maximum time to wait for live sessions to stop and finish recording their
/// runs during graceful shutdown, before giving up and leaving the rest to
/// startup recovery.
pub const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// The span every log line and task of the agent `name` runs inside, so each
/// carries an `agent` field. `residuum logs --agent <name>` filters on it.
pub fn agent_span(name: &str) -> tracing::Span {
    tracing::info_span!("agent", agent = %name)
}

/// What the hub gives an [`AgentLauncher`] to start one agent.
pub struct AgentStartInputs {
    /// The agent's name.
    pub name: String,
    /// The agent's directory (its workspace root).
    pub agent_dir: std::path::PathBuf,
    /// Services shared by every agent.
    pub services: HubServices,
    /// The hub config the agent resolves its own config against.
    pub hub_cfg: HubConfig,
    /// Where the agent reports main-conversation activity.
    pub activity: Arc<ActivityTracker>,
}

/// How the hub hands a message to an agent's main conversation. Its fields
/// stay private: only the hub's delivery path uses them.
#[derive(Clone)]
pub struct MainDelivery {
    /// The agent's bus publisher, for handing the agent a message.
    publisher: residuum_bus::Publisher,
    /// The agent's messenger, which delivers a creating agent's first
    /// message through the same hop-bounded path as any teammate message.
    messenger: Arc<residuum_background::messaging::AgentMessenger>,
}

impl MainDelivery {
    /// Delivery to the agent through its bus `publisher` and its `messenger`.
    #[must_use]
    pub fn new(
        publisher: residuum_bus::Publisher,
        messenger: Arc<residuum_background::messaging::AgentMessenger>,
    ) -> Self {
        Self {
            publisher,
            messenger,
        }
    }

    /// Deliver `content` to the agent's main conversation as a message from
    /// `from`: the owner's own message for the user, or a teammate message
    /// from the creating agent carrying `hop_count`, so a chain of agents
    /// creating agents meets the same hop limit as any other message chain.
    /// An idle main starts a turn on it.
    ///
    /// # Errors
    /// Returns a plain-language reason when the message could not be
    /// delivered: the agent's bus is closed, or the hop limit was reached.
    pub(crate) async fn deliver_to_main(
        &self,
        from: &residuum_background::team::Actor,
        content: String,
        tz: chrono_tz::Tz,
        hop_count: u32,
    ) -> Result<(), String> {
        let creator = match from {
            residuum_background::team::Actor::User => {
                let event = residuum_bus::MessageEvent {
                    id: format!("hub-{}", uuid::Uuid::new_v4()),
                    content,
                    origin: residuum_bus::origin::MessageOrigin {
                        endpoint: "ws".to_string(),
                        sender: None,
                        conversation: None,
                        agent_sender: None,
                    },
                    timestamp: residuum_core::time::now_local(tz),
                    images: Vec::new(),
                    attachments: Vec::new(),
                    context: None,
                };
                return self
                    .publisher
                    .publish(residuum_bus::topics::UserMessage, event)
                    .await
                    .map_err(|e| format!("the agent's message channel is closed: {e}"));
            }
            residuum_background::team::Actor::Agent(creator) => creator,
        };
        self.messenger
            .send(
                residuum_bus::session::MAIN_ADDRESS,
                residuum_bus::AgentMessageEvent::new(
                    residuum_bus::SessionAddress::from(format!(
                        "{}{creator}",
                        residuum_bus::session::TEAMMATE_SENDER_PREFIX
                    )),
                    residuum_bus::session::TEAMMATE_SENDER_CATEGORY,
                    content,
                    hop_count,
                ),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The hub's handle on an agent a launcher has just started.
#[derive(Clone)]
pub struct AgentControl {
    /// The agent's HTTP routes, rooted at `/`.
    pub router: axum::Router,
    /// The agent's current A2A router; `None` while A2A is disabled for it.
    /// Changes when a reload rebuilds the agent's A2A state.
    pub a2a_router: tokio::sync::watch::Receiver<Option<axum::Router>>,
    /// Delivers config reload signals to the agent's event loop.
    pub reload_tx: ReloadSender,
    /// Asks the agent's event loop to stop.
    pub stop_tx: tokio::sync::mpsc::Sender<()>,
    /// Counts config reloads the agent has finished, so the hub can wait for
    /// a reload it triggered and notice changes made behind its back.
    pub reload_done: tokio::sync::watch::Receiver<u64>,
    /// What the hub cleans up when the event loop dies without shutting the
    /// agent down.
    pub cleanup: AgentCleanup,
    /// The agent's live sessions, for the hub's bug reports and the hub's
    /// per-agent watcher.
    pub session_registry: Arc<residuum_background::registry::SessionRegistry>,
    /// Subscribes to the agent's own event bus: how the hub's per-agent
    /// watcher hears about sessions, outbound tasks, user inbox additions and
    /// file changes.
    pub bus: residuum_bus::BusHandle,
    /// Whether the change feed over the agent's directory is running.
    pub workspace_watch_health:
        tokio::sync::watch::Receiver<residuum_workspace::watch::WatchHealth>,
    /// Hands the agent a message for its main conversation.
    pub main_delivery: MainDelivery,
}

/// The parts of an agent that outlive a crashed event loop and must be
/// stopped explicitly: its live sessions and the MCP servers it spawned.
#[derive(Clone)]
pub struct AgentCleanup {
    sessions: Arc<residuum_background::SessionRuntime>,
    mcp_registry: residuum_mcp::SharedMcpRegistry,
}

impl AgentCleanup {
    /// The cleanup of an agent whose live sessions are `sessions` and whose
    /// MCP servers are in `mcp_registry`.
    #[must_use]
    pub fn new(
        sessions: Arc<residuum_background::SessionRuntime>,
        mcp_registry: residuum_mcp::SharedMcpRegistry,
    ) -> Self {
        Self {
            sessions,
            mcp_registry,
        }
    }

    /// Stop the agent's live sessions, recording them as interrupted the way
    /// a normal stop does, and disconnect its MCP servers.
    pub(crate) async fn run(&self) {
        self.sessions.shutdown(SESSION_SHUTDOWN_TIMEOUT).await;
        self.mcp_registry.write().await.disconnect_all().await;
    }
}

/// How an agent's event loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExit {
    /// The hub asked the agent to stop and it shut down cleanly.
    Stopped,
    /// The loop's own inbound message channel closed underneath it.
    BusClosed,
}

/// A started agent, ready to run.
pub struct StartedAgent {
    /// The hub's handle on the agent.
    pub control: AgentControl,
    /// The agent's event loop, which processes all of its events until the
    /// hub stops it. It does nothing until the host runs it.
    pub event_loop: Pin<Box<dyn Future<Output = AgentExit> + Send>>,
}

/// Starts agents for the hub.
#[async_trait]
pub trait AgentLauncher: Send + Sync {
    /// Initialize the agent `inputs` describes on the hub's shared services
    /// and return it ready to run.
    ///
    /// # Errors
    ///
    /// Returns `FatalError` when the agent can't start: its config can't be
    /// loaded and no fallback works, or an essential subsystem can't start.
    async fn start(&self, inputs: AgentStartInputs) -> Result<StartedAgent, FatalError>;
}

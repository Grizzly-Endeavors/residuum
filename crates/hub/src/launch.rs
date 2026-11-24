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

pub const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

pub fn agent_span(name: &str) -> tracing::Span {
    tracing::info_span!("agent", agent = %name)
}

pub struct AgentStartInputs {
    pub name: String,
    pub agent_dir: std::path::PathBuf,
    pub services: HubServices,
    pub hub_cfg: HubConfig,
    pub activity: Arc<ActivityTracker>,
}

#[derive(Clone)]
pub struct MainDelivery {
    publisher: residuum_bus::Publisher,
    messenger: Arc<residuum_background::messaging::AgentMessenger>,
}

impl MainDelivery {
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

#[derive(Clone)]
pub struct AgentControl {
    pub router: axum::Router,
    pub a2a_router: tokio::sync::watch::Receiver<Option<axum::Router>>,
    pub reload_tx: ReloadSender,
    pub stop_tx: tokio::sync::mpsc::Sender<()>,
    pub reload_done: tokio::sync::watch::Receiver<u64>,
    pub cleanup: AgentCleanup,
    pub session_registry: Arc<residuum_background::registry::SessionRegistry>,
    pub bus: residuum_bus::BusHandle,
    pub workspace_watch_health:
        tokio::sync::watch::Receiver<residuum_workspace::watch::WatchHealth>,
    pub main_delivery: MainDelivery,
}

#[derive(Clone)]
pub struct AgentCleanup {
    sessions: Arc<residuum_background::SessionRuntime>,
    mcp_registry: residuum_mcp::SharedMcpRegistry,
}

impl AgentCleanup {
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

    pub(crate) async fn run(&self) {
        self.sessions.shutdown(SESSION_SHUTDOWN_TIMEOUT).await;
        self.mcp_registry.write().await.disconnect_all().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExit {
    Stopped,
    BusClosed,
}

pub struct StartedAgent {
    pub control: AgentControl,
    pub event_loop: Pin<Box<dyn Future<Output = AgentExit> + Send>>,
}

#[async_trait]
pub trait AgentLauncher: Send + Sync {
    async fn start(&self, inputs: AgentStartInputs) -> Result<StartedAgent, FatalError>;
}

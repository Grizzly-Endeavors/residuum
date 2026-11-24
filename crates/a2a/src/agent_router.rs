use std::sync::Arc;

use axum::Router;

use residuum_background::messaging::AgentMessenger;
use residuum_background::registry::SessionRegistry;
use residuum_bus::BusHandle;
use residuum_config::A2aConfig;
use residuum_config::WorkspaceLayout;
use residuum_net::tunnel::TunnelStatus;
use residuum_skills::SharedSkillState;

use super::card::{CardRuntime, CardState, SharedCardState};
use super::executor::SessionExecutor;
use super::handler::{ResiduumA2aHandler, resume_in_progress_tasks};
use super::listener::agent_handler_router;
use super::public_url::relay_a2a_base;
use super::task_store::{DelegatingTaskStore, FileTaskStore};

pub struct AgentA2aState {
    pub name: String,
    pub a2a: A2aConfig,
    pub bind: String,
    pub layout: WorkspaceLayout,
    pub timezone: chrono_tz::Tz,
    pub agent_messenger: Arc<AgentMessenger>,
    pub session_registry: Arc<SessionRegistry>,
    pub bus_handle: BusHandle,
    pub skill_state: SharedSkillState,
    pub sessions_ready: tokio::sync::watch::Receiver<bool>,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct AgentA2a {
    pub router: Router,
    pub card_state: SharedCardState,
    pub public_url: String,
    pub(crate) tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    _card_url_watch: AbortOnDrop,
}

impl AgentA2a {
    #[must_use]
    pub fn relay_base(&self) -> Option<String> {
        relay_a2a_base(&self.tunnel_status_rx.borrow())
    }
}

fn watch_card_url(
    card_state: SharedCardState,
    card_path: std::path::PathBuf,
    watch: CardUrlWatch,
) -> AbortOnDrop {
    let CardUrlWatch {
        a2a,
        bind,
        agent,
        mut tunnel_status_rx,
    } = watch;
    let mut applied = relay_a2a_base(&tunnel_status_rx.borrow_and_update());
    AbortOnDrop(residuum_core::util::spawn_in_span(async move {
        while tunnel_status_rx.changed().await.is_ok() {
            let base = relay_a2a_base(&tunnel_status_rx.borrow_and_update());
            if base == applied {
                continue;
            }
            let runtime = CardRuntime::from_config(&a2a, &bind, &agent, base.as_deref());
            match card_state.reload(&card_path, &runtime) {
                Ok(()) => {
                    tracing::info!(
                        agent = %agent,
                        url = %runtime.interfaces_base_url,
                        "agent card address updated after a tunnel change"
                    );
                    applied = base;
                }
                Err(e) => {
                    tracing::warn!(
                        agent = %agent,
                        error = %e,
                        "couldn't update the agent card's address after a tunnel change; still serving the previous card"
                    );
                }
            }
        }
    }))
}

struct CardUrlWatch {
    a2a: A2aConfig,
    bind: String,
    agent: String,
    tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

pub async fn agent_a2a_router(state: AgentA2aState) -> anyhow::Result<AgentA2a> {
    let relay_base = relay_a2a_base(&state.tunnel_status_rx.borrow());
    let card_runtime =
        CardRuntime::from_config(&state.a2a, &state.bind, &state.name, relay_base.as_deref());
    let public_url = card_runtime.interfaces_base_url.clone();
    let card_state = CardState::load_or_default(&state.layout.agent_card_json(), &card_runtime);
    let card_url_watch = watch_card_url(
        Arc::clone(&card_state),
        state.layout.agent_card_json(),
        CardUrlWatch {
            a2a: state.a2a.clone(),
            bind: state.bind.clone(),
            agent: state.name.clone(),
            tunnel_status_rx: state.tunnel_status_rx.clone(),
        },
    );

    let task_store = FileTaskStore::load(&state.layout.a2a_tasks_dir()).await?;

    let executor = SessionExecutor::new(
        state.agent_messenger,
        state.session_registry,
        state.bus_handle,
        state.skill_state,
        Arc::clone(&card_state),
        state.layout.agent_inbox_dir(),
        state.timezone,
    );
    let inner = a2a_server::DefaultRequestHandler::new(
        executor,
        DelegatingTaskStore(Arc::clone(&task_store)),
    )
    .with_capabilities(a2a::AgentCapabilities {
        streaming: Some(true),
        push_notifications: Some(false),
        extensions: None,
        extended_agent_card: None,
    });
    let handler = Arc::new(ResiduumA2aHandler::new(inner, Arc::clone(&task_store)));

    let resume_handler = Arc::clone(&handler);
    let resume_store = Arc::clone(&task_store);
    let mut sessions_ready = state.sessions_ready;
    let agent = state.name;
    residuum_core::util::spawn_in_span(async move {
        if sessions_ready.wait_for(|ready| *ready).await.is_err() {
            tracing::error!(
                agent = %agent,
                "session spawner never became ready; a2a tasks left in progress were not resumed"
            );
            return;
        }
        resume_in_progress_tasks(resume_handler, resume_store).await;
    });

    Ok(AgentA2a {
        router: agent_handler_router(handler, Arc::clone(&card_state)),
        card_state,
        public_url,
        tunnel_status_rx: state.tunnel_status_rx,
        _card_url_watch: card_url_watch,
    })
}

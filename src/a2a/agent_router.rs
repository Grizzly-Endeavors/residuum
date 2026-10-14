//! Per-agent A2A server assembly: the function the agent host calls once per
//! running agent to get that agent's A2A routes. See
//! `docs/systems-usage/a2a.md`.

use std::sync::Arc;

use axum::Router;

use crate::background::messaging::AgentMessenger;
use crate::background::registry::SessionRegistry;
use crate::bus::BusHandle;
use crate::config::A2aConfig;
use crate::skills::SharedSkillState;
use crate::tunnel::TunnelStatus;
use crate::workspace::layout::WorkspaceLayout;

use super::card::{CardRuntime, CardState, SharedCardState};
use super::executor::SessionExecutor;
use super::handler::{ResiduumA2aHandler, resume_in_progress_tasks};
use super::listener::agent_handler_router;
use super::public_url::relay_a2a_base;
use super::task_store::{DelegatingTaskStore, FileTaskStore};

/// Everything one agent's A2A server needs from that agent's runtime.
pub struct AgentA2aState {
    /// The agent's name: its card's URL segment (`/agents/<name>`).
    pub name: String,
    /// The `[a2a]` settings as this agent sees them: the hub's `port` and
    /// `public_url`, and this agent's own `visibility`.
    pub a2a: A2aConfig,
    /// The gateway's bind address, for the local-fallback card URL.
    pub bind: String,
    /// The agent's workspace layout: `agent-card.json`, the persistent task
    /// store directory, and the inbox for inbound attachments.
    pub layout: WorkspaceLayout,
    /// The agent's timezone.
    pub timezone: chrono_tz::Tz,
    /// The agent's messenger, for delivering inbound tasks to its sessions.
    pub agent_messenger: Arc<AgentMessenger>,
    /// The agent's live sessions.
    pub session_registry: Arc<SessionRegistry>,
    /// The agent's bus.
    pub bus_handle: BusHandle,
    /// The agent's skills, for mapping a task's skill metadata.
    pub skill_state: SharedSkillState,
    /// Turns `true` once the agent's session spawn listener is subscribed.
    /// Restart continuations wait on it: a session spawn published before
    /// then has no listener and would be lost.
    pub sessions_ready: tokio::sync::watch::Receiver<bool>,
    /// The relay tunnel's status. The card advertises the agent's relay
    /// address while the tunnel is connected, and follows the tunnel as it
    /// connects and disconnects.
    pub(crate) tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

/// Aborts the task it holds when dropped, so a task tied to an [`AgentA2a`]
/// ends with it.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One agent's A2A server: its routes and the handles the host keeps.
pub struct AgentA2a {
    /// The agent's card, JSON-RPC, and REST routes, rooted at `/` and
    /// unauthenticated. The hub's A2A listener authenticates each request
    /// against the agent's visibility, strips the `/agents/<name>` prefix,
    /// and dispatches here.
    pub router: Router,
    /// The live card, so the host can reload it when `agent-card.json` or
    /// the `[a2a]` settings change.
    pub card_state: SharedCardState,
    /// The URL the agent's card advertises for its JSON-RPC endpoint when the
    /// server was built.
    pub public_url: String,
    /// The relay tunnel's status, for rebuilding the card on a workspace
    /// reload with the current relay address.
    pub(crate) tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    /// Keeps the card's relay address current as the tunnel connects and
    /// disconnects; ends when this value is dropped.
    _card_url_watch: AbortOnDrop,
}

impl AgentA2a {
    /// The instance's A2A base (`{instance origin}/a2a`) to build the card under
    /// right now, when the tunnel is connected.
    #[must_use]
    pub fn relay_base(&self) -> Option<String> {
        relay_a2a_base(&self.tunnel_status_rx.borrow())
    }
}

/// Reload `card_state` under the agent's relay address every time the tunnel
/// connects, disconnects, or changes address. A card file that no longer
/// loads keeps the last good card, address included, until it is fixed; the
/// reload that follows a fix picks up the current address.
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
    AbortOnDrop(crate::util::spawn_in_span(async move {
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

/// What [`watch_card_url`] needs to rebuild a card.
struct CardUrlWatch {
    a2a: A2aConfig,
    bind: String,
    agent: String,
    tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

/// Build one agent's A2A server from its runtime state.
///
/// Loads the agent's card (a broken `agent-card.json` serves a minimal
/// fallback card), opens its persistent task store, wires the session
/// executor and handler, and starts the sweep that resumes tasks left in
/// progress by a previous run once `sessions_ready` turns true.
///
/// The router omits authentication, the caller-key store, and the tunnel
/// nonce; those are hub-level and live on the hub listener.
///
/// # Errors
/// Returns an error if the persistent task store's directory can't be
/// created or read.
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
    crate::util::spawn_in_span(async move {
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

//! The running agent's web routes: its status, A2A status and card, inbox,
//! sessions, scheduled runs, memory and model. The routes the hub serves
//! (hub-level, repair and file routes) live in `residuum_hub::web`.

use axum::routing::get;

pub(crate) mod a2a;
pub(crate) mod artifact_identity;
pub(crate) mod inbox;
pub(crate) mod memory;
pub(crate) mod model;
pub mod scheduled;
pub(crate) mod sessions;
mod status;

pub(crate) use status::AgentStatusState;

/// The running agent's `status` route: the one per-agent data route that
/// describes the live process, so it needs a running agent.
pub(crate) fn agent_status_api_router(state: AgentStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(status::api_status))
        .with_state(state)
}

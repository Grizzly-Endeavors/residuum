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

pub(crate) fn agent_status_api_router(state: AgentStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(status::api_status))
        .with_state(state)
}

//! The hub's HTTP surface: one router for everything the backend serves.
//!
//! [`hub_router`] takes the [`AgentDirectory`] and the hub-level handles in
//! [`HubHttpState`] and returns the whole app, laid out as
//! `docs/systems-usage/hub-http.md` places it:
//!
//! - `/api/hub/...`: agent lifecycle and status ([`lifecycle`]), the hub
//!   WebSocket ([`ws`], with its artifact events in [`artifact_events`] and
//!   its session relay in [`session_relay`]), every agent's user inbox
//!   ([`inbox`]), the team event
//!   log ([`events`]), the team overview ([`overview`]), Web Push devices
//!   ([`push`]), and the routes that
//!   exist once per process: hub config, secrets, keys, cloud, update,
//!   shutdown, tracing, and the hub and team checkpoint repositories
//!   ([`process`]).
//! - `/api/team/...`: the team's file API and workbench.
//! - `/api/agents/{name}/...` and `/webhook/{agent}/{name}`: resolved against
//!   the directory on every request and handed to the agent's own routers
//!   ([`dispatch`]).
//! - `/cloud/callback`, and the embedded web app for every other path.
//!
//! The cross-site guard covers the whole app. The remote-control guard covers
//! hub shutdown and cloud disconnect. Requests the artifacts listener forwards
//! here are refused on the routes in [`artifacts_origin`].

mod artifact_events;
mod artifacts_origin;
mod dispatch;
mod error;
mod events;
mod inbox;
mod lifecycle;
mod overview;
mod process;
mod push;
mod session_relay;
mod state;
#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes parsed JSON for clarity"
)]
mod tests;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;

pub use crate::gateway::web::{
    ConfigApiState, WorkspaceScope, agent_repair_api_router as agent_repair_router,
};
pub use state::HubHttpState;

use crate::gateway::web;
use crate::hub::AgentDirectory;

/// Build the hub's whole HTTP app over `directory`, with the process-wide
/// handles in `hub`. Serve it on the gateway address.
pub fn hub_router(directory: Arc<dyn AgentDirectory>, hub: HubHttpState) -> Router {
    let lifecycle_state = lifecycle::LifecycleState {
        directory: Arc::clone(&directory),
        hub_dir: hub.hub_dir.clone(),
        tunnel_status_rx: hub.tunnel_status_rx.clone(),
        started_at: hub.started_at,
    };
    let ws_state = ws::HubWsState {
        directory: Arc::clone(&directory),
        team_bus: hub.team_bus.clone(),
        team_watch_health: hub.team_watch_health.clone(),
        team_events: Arc::clone(&hub.team_events),
        overview: Arc::clone(&hub.overview),
        agent_changes: Arc::clone(&hub.agent_changes),
        presence: Arc::clone(hub.push.presence()),
    };

    let app = Router::new()
        .merge(lifecycle::routes(lifecycle_state))
        .merge(ws::routes(ws_state))
        .merge(inbox::routes(inbox::InboxState {
            directory: Arc::clone(&directory),
            overview: Arc::clone(&hub.overview),
        }))
        .merge(push::routes(push::PushApiState {
            push: Arc::clone(&hub.push),
            directory: Arc::clone(&directory),
        }))
        .merge(events::routes(Arc::clone(&hub.team_events)))
        .merge(overview::routes(Arc::clone(&hub.overview)))
        .merge(process::hub_config_routes(&hub))
        .merge(process::cloud_routes(&hub))
        .merge(process::update_routes(&hub))
        .merge(process::tracing_routes(&hub))
        .merge(process::checkpoint_routes(&hub))
        .merge(process::team_routes(&hub))
        .merge(dispatch::routes(directory))
        .route("/api/sessions", post(sessions_need_an_agent))
        .fallback_service(web::static_assets())
        .layer(axum::middleware::from_fn(
            artifacts_origin::refuse_blocked_artifact_calls,
        ))
        .layer(axum::middleware::from_fn(
            crate::gateway::cross_site::reject_cross_site_requests,
        ));
    // The routers above hold their own clones of the handles they need.
    drop(hub);
    app
}

/// `POST /api/sessions`: sessions belong to an agent, so an artifact starts
/// one at `/api/agents/{name}/sessions`. A request that names no agent gets
/// this explanation instead of a bare "not found".
async fn sessions_need_an_agent() -> Response {
    error::json_error(
        StatusCode::BAD_REQUEST,
        "A session runs on one agent, so a start request must name it. Start it with \
         POST /api/agents/<name>/sessions, or pass { agent } to residuum.sessions.start.",
    )
}

mod artifact_events;
mod artifacts_origin;
mod dispatch;
mod error;
mod events;
mod lifecycle;
mod mail;
mod overview;
mod pairing;
mod process;
mod push;
mod remote_access;
mod session_relay;
mod state;
#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes parsed JSON for clarity"
)]
mod tests;
mod uploads;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;

pub use state::HubHttpState;

use crate::AgentDirectory;
use crate::web;

pub fn hub_router(directory: Arc<dyn AgentDirectory>, hub: HubHttpState) -> Router {
    let assets = hub.assets;
    let lifecycle_state = lifecycle::LifecycleState {
        directory: Arc::clone(&directory),
        hub_dir: hub.hub_dir.clone(),
        tunnel_status_rx: hub.tunnel_status_rx.clone(),
        started_at: hub.started_at,
        build: hub.build,
    };
    let ws_state = ws::HubWsState {
        directory: Arc::clone(&directory),
        team_bus: hub.team_bus.clone(),
        team_watch_health: hub.team_watch_health.clone(),
        team_events: Arc::clone(&hub.team_events),
        overview: Arc::clone(&hub.overview),
        agent_changes: Arc::clone(&hub.agent_changes),
        presence: Arc::clone(hub.push.presence()),
        system_one: Arc::clone(&hub.system_one),
    };

    let app = Router::new()
        .merge(lifecycle::routes(lifecycle_state))
        .merge(ws::routes(ws_state))
        .merge(mail::routes(mail::MailState {
            directory: Arc::clone(&directory),
            overview: Arc::clone(&hub.overview),
            uploads: hub.uploads.clone(),
        }))
        .merge(uploads::routes(hub.uploads.clone()))
        .merge(push::routes(push::PushApiState {
            push: Arc::clone(&hub.push),
            directory: Arc::clone(&directory),
        }))
        .merge(events::routes(Arc::clone(&hub.team_events)))
        .merge(overview::routes(Arc::clone(&hub.overview)))
        .merge(pairing::routes(hub.pairing.clone()))
        .merge(remote_access::routes(hub.remote_access.clone()))
        .merge(process::hub_config_routes(&hub))
        .merge(process::cloud_routes(&hub))
        .merge(process::update_routes(&hub))
        .merge(process::tracing_routes(&hub))
        .merge(process::checkpoint_routes(&hub))
        .merge(process::team_routes(&hub))
        .merge(dispatch::routes(directory))
        .route("/api/sessions", post(sessions_need_an_agent))
        .fallback_service(web::static_assets(assets))
        .layer(axum::middleware::from_fn(
            artifacts_origin::refuse_blocked_artifact_calls,
        ))
        .layer(axum::middleware::from_fn(
            residuum_net::cross_site::reject_cross_site_requests,
        ))
        .layer(axum::middleware::from_fn_with_state(
            residuum_net::pairing::GateState {
                pairing: hub.pairing.clone(),
                surface: residuum_net::pairing::Surface::Ui,
            },
            residuum_net::pairing::device_gate,
        ));
    drop(hub);
    app
}

async fn sessions_need_an_agent() -> Response {
    error::json_error(
        StatusCode::BAD_REQUEST,
        "A session runs on one agent, so a start request must name it. Start it with \
         POST /api/agents/<name>/sessions, or pass { agent } to residuum.sessions.start.",
    )
}

//! `/api/hub/agents...` and `/api/hub/status`: agent lifecycle and hub status.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, patch, post};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::error::{json_error, lifecycle_error_response};
use crate::gateway::web::cloud::CloudStatusResponse;
use crate::hub::{
    Actor, AgentDirectory, AgentPatch, AgentState, AgentSummary, CreateAgentRequest, LifecycleError,
};

/// What the lifecycle and status routes read.
#[derive(Clone)]
pub(super) struct LifecycleState {
    pub directory: Arc<dyn AgentDirectory>,
    pub hub_dir: std::path::PathBuf,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<crate::tunnel::TunnelStatus>,
    pub started_at: std::time::Instant,
}

/// The agent list, create, per-agent lifecycle, stop-all, and status routes.
pub(super) fn routes(state: LifecycleState) -> Router {
    Router::new()
        .route("/api/hub/agents", get(list_agents).post(create_agent))
        .route(
            "/api/hub/agents/{name}",
            patch(patch_agent).delete(delete_agent),
        )
        .route("/api/hub/agents/{name}/start", post(start_agent))
        .route("/api/hub/agents/{name}/stop", post(stop_agent))
        .route("/api/hub/agents/{name}/restart", post(restart_agent))
        .route("/api/hub/stop-all", post(stop_all))
        .route("/api/hub/status", get(hub_status))
        .with_state(state)
}

/// Parse a JSON request body, or say in plain words why it can't be used.
fn parse_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, String> {
    serde_json::from_slice(body)
        .map_err(|e| format!("the request body isn't valid for this route: {e}"))
}

fn summary_response(result: Result<AgentSummary, LifecycleError>) -> Response {
    match result {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => lifecycle_error_response(&e),
    }
}

/// `GET /api/hub/agents` — every agent, sorted by name.
async fn list_agents(State(state): State<LifecycleState>) -> Json<serde_json::Value> {
    let mut agents = state.directory.list();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Json(json!({ "agents": agents }))
}

/// `POST /api/hub/agents` — create and start an agent.
async fn create_agent(State(state): State<LifecycleState>, body: Bytes) -> Response {
    let request: CreateAgentRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    match state.directory.create(request, Actor::User).await {
        Ok(summary) => (StatusCode::CREATED, Json(summary)).into_response(),
        Err(e) => lifecycle_error_response(&e),
    }
}

/// `DELETE /api/hub/agents/{name}` — checkpoint, stop, and remove an agent.
async fn delete_agent(State(state): State<LifecycleState>, Path(name): Path<String>) -> Response {
    match state.directory.delete(&name, Actor::User).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => lifecycle_error_response(&e),
    }
}

/// `POST /api/hub/agents/{name}/start`.
async fn start_agent(State(state): State<LifecycleState>, Path(name): Path<String>) -> Response {
    summary_response(state.directory.start(&name).await)
}

/// `POST /api/hub/agents/{name}/stop`.
async fn stop_agent(State(state): State<LifecycleState>, Path(name): Path<String>) -> Response {
    summary_response(state.directory.stop(&name).await)
}

/// `POST /api/hub/agents/{name}/restart`.
async fn restart_agent(State(state): State<LifecycleState>, Path(name): Path<String>) -> Response {
    summary_response(state.directory.restart(&name).await)
}

/// `PATCH /api/hub/agents/{name}` — change autostart and/or A2A visibility.
async fn patch_agent(
    State(state): State<LifecycleState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let patch: AgentPatch = match parse_body(&body) {
        Ok(patch) => patch,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    if patch.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "the request must set at least one of autostart or a2a_visibility",
        );
    }
    summary_response(state.directory.patch(&name, patch).await)
}

/// One agent that `stop-all` could not stop.
#[derive(Serialize)]
struct StopFailure {
    name: String,
    error: String,
}

/// `POST /api/hub/stop-all` — stop every running or starting agent.
///
/// Answers `200` when every agent stopped and `500` when some did not; the
/// body lists which agents stopped and which failed, and why.
async fn stop_all(State(state): State<LifecycleState>) -> Response {
    let mut stopped = Vec::new();
    let mut failed = Vec::new();
    for agent in state.directory.list() {
        if !matches!(agent.state, AgentState::Running | AgentState::Starting) {
            continue;
        }
        match state.directory.stop(&agent.name).await {
            Ok(summary) => stopped.push(summary),
            Err(e) => {
                tracing::error!(agent = %agent.name, error = %e, "failed to stop an agent during stop-all");
                failed.push(StopFailure {
                    name: agent.name,
                    error: e.to_string(),
                });
            }
        }
    }
    let status = if failed.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (
        status,
        Json(json!({ "stopped": stopped, "failed": failed })),
    )
        .into_response()
}

/// `GET /api/hub/status` — version, uptime, tunnel, and agent counts.
async fn hub_status(
    State(state): State<LifecycleState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let (mut starting, mut running, mut stopped, mut failed) = (0_u32, 0_u32, 0_u32, 0_u32);
    for agent in state.directory.list() {
        match agent.state {
            AgentState::Starting => starting += 1,
            AgentState::Running => running += 1,
            AgentState::Stopped => stopped += 1,
            AgentState::Failed => failed += 1,
        }
    }
    let tunnel = CloudStatusResponse::current(&state.hub_dir, &state.tunnel_status_rx, &headers);
    Json(json!({
        "version": crate::update::CURRENT_VERSION,
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "tunnel": tunnel,
        "agents": {
            "starting": starting,
            "running": running,
            "stopped": stopped,
            "failed": failed,
        },
    }))
}

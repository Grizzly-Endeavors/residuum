//! The hub's HTTP router over the agent directory.
//!
//! [`build_hub_app`] is the whole of the hub's HTTP wiring, kept in this one
//! function so the full contract router can replace it. It does two things:
//!
//! - `/api/agents/{name}/...` is resolved per request against the directory
//!   and served by that agent's own router with the prefix stripped: `404`
//!   for an unknown agent, `409` with the agent's state when it isn't running
//!   (except for the config, file, and checkpoint routes, which answer on a
//!   stopped or failed agent so it can be repaired).
//! - Every other path is served by the first running agent's router, which
//!   still carries the hub-level routes (secrets, cloud, update, tracing,
//!   shutdown) and the app shell, so a single-agent install keeps working
//!   against root paths. With no agent running, the app shell is still
//!   served and API paths answer `503`.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use tower::ServiceExt as _;

use super::directory::AgentDirectory;
use super::types::{AgentState, LifecycleError};
use crate::gateway::web::NO_ROUTE_HEADER;

#[derive(Clone)]
struct WiringState {
    directory: Arc<dyn AgentDirectory>,
}

/// The hub's HTTP router over `directory`.
pub(crate) fn build_hub_app(directory: Arc<dyn AgentDirectory>) -> Router {
    Router::new()
        .route("/api/agents/{name}", any(dispatch_to_agent))
        .route("/api/agents/{name}/{*rest}", any(dispatch_to_agent))
        .fallback(serve_first_running_agent)
        .with_state(WiringState { directory })
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

/// The request with its path replaced by `path`, query preserved.
fn with_path(mut request: Request<Body>, path: &str) -> Result<Request<Body>, axum::http::Error> {
    let path_and_query = match request.uri().query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let mut parts = request.uri().clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse()?);
    *request.uri_mut() = Uri::from_parts(parts)?;
    Ok(request)
}

async fn forward(router: Router, request: Request<Body>) -> Response {
    match router.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

async fn dispatch_to_agent(
    State(state): State<WiringState>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    request: Request<Body>,
) -> Response {
    let Some(name) = params.get("name") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let prefix = format!("/api/agents/{name}");
    let rest = request
        .uri()
        .path()
        .strip_prefix(&prefix)
        .filter(|rest| !rest.is_empty())
        .unwrap_or("/")
        .to_string();
    let request = match with_path(request, &rest) {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, "rejected a request with an unusable path");
            return json_error(StatusCode::BAD_REQUEST, "the request path is not valid");
        }
    };

    match state.directory.agent_router(name) {
        Ok(router) => forward(router, request).await,
        Err(LifecycleError::NotFound(_)) => {
            json_error(StatusCode::NOT_FOUND, &format!("no agent named '{name}'"))
        }
        Err(LifecycleError::NotRunning {
            state: agent_state, ..
        }) => answer_for_stopped_agent(&state, name, agent_state, request).await,
        Err(e) => {
            tracing::error!(agent = %name, error = %e, "failed to resolve an agent's routes");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

/// Serve the repair routes on an agent that isn't running, or answer `409`
/// with its state for anything else.
async fn answer_for_stopped_agent(
    state: &WiringState,
    name: &str,
    agent_state: AgentState,
    request: Request<Body>,
) -> Response {
    let not_running = || {
        (
            StatusCode::CONFLICT,
            axum::Json(serde_json::json!({
                "error": format!("{name} is {agent_state}"),
                "state": agent_state.to_string(),
            })),
        )
            .into_response()
    };
    let router = match state.directory.agent_repair_router(name) {
        Ok(router) => router,
        Err(LifecycleError::NotFound(_)) => {
            return json_error(StatusCode::NOT_FOUND, &format!("no agent named '{name}'"));
        }
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, "an agent's repair routes are unavailable");
            return not_running();
        }
    };
    let response = forward(router, request).await;
    if response.headers().contains_key(NO_ROUTE_HEADER) {
        not_running()
    } else {
        response
    }
}

/// Serve a root-level request from the first running agent, or the app shell
/// when none is running.
async fn serve_first_running_agent(
    State(state): State<WiringState>,
    request: Request<Body>,
) -> Response {
    let first_running = state
        .directory
        .list()
        .into_iter()
        .find(|agent| agent.state == AgentState::Running)
        .and_then(|agent| state.directory.agent_router(&agent.name).ok());
    if let Some(router) = first_running {
        return forward(router, request).await;
    }
    if request.uri().path().starts_with("/api/") || request.uri().path() == "/ws" {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "no agent is running");
    }
    crate::gateway::web::static_handler(request.uri().clone()).await
}

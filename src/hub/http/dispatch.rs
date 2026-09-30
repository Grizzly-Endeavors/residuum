//! Per-request routing into a hosted agent's own routers.
//!
//! Agents are created, stopped, and deleted while the hub runs, so
//! `/api/agents/{name}/...` and `/webhook/{agent}/{name}` are resolved
//! against the [`AgentDirectory`] on every request. The agent's router sees
//! the request with the hub's prefix removed, exactly as it has always seen
//! it, and is called with `oneshot` so the request (and with it a WebSocket
//! upgrade) passes through whole.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, Uri};
use axum::response::Response;
use tower::ServiceExt;
use tracing::Instrument;

use super::error::{json_error, lifecycle_error_response};
use crate::hub::{AgentDirectory, LifecycleError};

/// The routes that hand requests to an agent: everything under
/// `/api/agents/` and `/webhook/`.
///
/// They are mounted as services under fixed prefixes rather than as routes
/// with `{name}` captures. A capture would leave path parameters on the
/// request, and the agent's own router would then see them beside its own
/// and fail every `Path` extractor.
pub(super) fn routes(directory: Arc<dyn AgentDirectory>) -> Router {
    let agents = {
        let directory = Arc::clone(&directory);
        tower::service_fn(move |req: Request| {
            let directory = Arc::clone(&directory);
            async move { Ok::<_, Infallible>(agent_request(directory.as_ref(), req).await) }
        })
    };
    let webhooks = tower::service_fn(move |req: Request| {
        let directory = Arc::clone(&directory);
        async move { Ok::<_, Infallible>(webhook_request(directory.as_ref(), req).await) }
    });
    Router::new()
        .nest_service("/api/agents", agents)
        .nest_service("/webhook", webhooks)
}

/// Which of an agent's routers serves an inner path.
#[derive(Debug, PartialEq, Eq)]
enum AgentRouterKind {
    /// Config, providers, MCP, workspace-file, and checkpoint routes, which
    /// work on a stopped or failed agent so the user can repair it.
    Repair,
    /// Routes that only read and write the agent's files: chat history,
    /// usage, the user inbox, and the raw A2A client settings. A running
    /// agent serves them from its own router, at the cost it always had. Any
    /// other agent gets them from a router that opens no checkpoint
    /// repository, so they answer even when the repositories can't be opened.
    Files,
    /// Everything else, which needs a running agent.
    Running,
}

impl AgentRouterKind {
    /// The router for a path in the agent's own route table (`/ws`, or
    /// `/api/...`).
    ///
    /// The file routes are matched whole, not by their first segment,
    /// because most routes under the same first segment need the agent: of
    /// `a2a/...` only `a2a/agents/raw` is a file route, while `a2a/agents`,
    /// `a2a/status`, `a2a/card` and `a2a/outbound...` are live.
    fn for_inner_path(inner: &str) -> Self {
        let segments: Vec<&str> = inner.trim_start_matches('/').split('/').collect();
        match segments.as_slice() {
            [
                "api",
                "config" | "providers" | "mcp" | "workspace" | "checkpoints",
                ..,
            ] => Self::Repair,
            ["api", route @ ..] if is_file_data_route(route) => Self::Files,
            _ => Self::Running,
        }
    }
}

/// Whether the route below `/api`, split into its segments, is one of the
/// file-only data routes.
fn is_file_data_route(route: &[&str]) -> bool {
    match route {
        ["chat", "history"] | ["usage"] | ["a2a", "agents", "raw"] => true,
        ["inbox", rest @ ..] => matches!(
            rest,
            [] | ["archive"] | [_, "read" | "archive" | "restore"] | [_, "attachments", _]
        ),
        _ => false,
    }
}

/// The path in the agent's own route table for the part of the hub URL after
/// `/api/agents/{name}`. `/ws` stays `/ws`; every other route lives under
/// `/api` in the agent's table.
fn agent_inner_path(after_name: &str) -> String {
    if after_name == "/ws" {
        "/ws".to_string()
    } else {
        format!("/api{after_name}")
    }
}

/// `/api/agents/{name}` and everything below it. The request's path has
/// already lost its `/api/agents` prefix: it is `/{name}{rest}`.
async fn agent_request(directory: &dyn AgentDirectory, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let Some((name, after_name)) = split_agent_path(&path) else {
        return json_error(StatusCode::NOT_FOUND, "not found");
    };
    let inner = agent_inner_path(after_name);
    let router = match AgentRouterKind::for_inner_path(&inner) {
        AgentRouterKind::Repair => directory.agent_repair_router(name),
        AgentRouterKind::Files => match directory.agent_router(name) {
            Err(LifecycleError::NotRunning { .. }) => directory.agent_file_router(name),
            running_or_unknown => running_or_unknown,
        },
        AgentRouterKind::Running => directory.agent_router(name),
    };
    forward(name, router, req, &inner).await
}

/// `/webhook/{agent}/{name}`: the agent's own `/webhook/{name}` route. The
/// request's path has already lost its `/webhook` prefix: it is
/// `/{agent}/{name}`.
async fn webhook_request(directory: &dyn AgentDirectory, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let Some((agent, webhook)) = path.strip_prefix('/').and_then(|rest| rest.split_once('/'))
    else {
        return json_error(StatusCode::NOT_FOUND, "not found");
    };
    let inner = format!("/webhook/{webhook}");
    forward(agent, directory.agent_router(agent), req, &inner).await
}

/// Split `/{name}{rest}` into the raw name and `rest` (empty, or starting
/// with `/`).
fn split_agent_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix('/')?;
    if after.is_empty() {
        return None;
    }
    Some(after.find('/').map_or((after, ""), |i| after.split_at(i)))
}

/// Hand `req` to the agent's router with its path replaced by `inner_path`,
/// or answer with the reason the agent can't take it.
///
/// The request runs inside `agent`'s span whichever of its routers serves it,
/// so handler logs and the tasks handlers spawn with `spawn_in_span` carry the
/// `agent` field even for the repair and webhook routes.
async fn forward(
    agent: &str,
    router: Result<Router, LifecycleError>,
    mut req: Request,
    inner_path: &str,
) -> Response {
    let router = match router {
        Ok(router) => router,
        Err(e) => return lifecycle_error_response(&e),
    };
    let path_and_query = req.uri().query().map_or_else(
        || inner_path.to_string(),
        |query| format!("{inner_path}?{query}"),
    );
    let uri = match Uri::builder().path_and_query(path_and_query).build() {
        Ok(uri) => uri,
        Err(e) => {
            tracing::warn!(error = %e, path = %inner_path, "couldn't build the agent-relative request URI");
            return json_error(StatusCode::BAD_REQUEST, "the request path isn't valid");
        }
    };
    *req.uri_mut() = uri;
    match router
        .oneshot(req)
        .instrument(crate::gateway::event_loop::agent_span(agent))
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_paths_split_into_name_and_remainder() {
        assert_eq!(
            split_agent_path("/scout/config/raw"),
            Some(("scout", "/config/raw"))
        );
        assert_eq!(split_agent_path("/scout"), Some(("scout", "")));
        assert_eq!(split_agent_path("/scout/ws"), Some(("scout", "/ws")));
        assert_eq!(split_agent_path("/"), None);
        assert_eq!(split_agent_path(""), None);
    }

    #[test]
    fn the_websocket_keeps_its_path_and_everything_else_moves_under_api() {
        assert_eq!(agent_inner_path("/ws"), "/ws");
        assert_eq!(agent_inner_path("/status"), "/api/status");
        assert_eq!(agent_inner_path("/files/workspace"), "/api/files/workspace");
        assert_eq!(agent_inner_path("/a2a/agents/raw"), "/api/a2a/agents/raw");
    }

    #[test]
    fn repair_routes_are_config_providers_mcp_workspace_and_checkpoints() {
        for repair in [
            "/api/config/raw",
            "/api/providers/patch",
            "/api/providers/models",
            "/api/mcp/raw",
            "/api/workspace/file",
            "/api/checkpoints",
            "/api/checkpoints/abc/diff",
        ] {
            assert_eq!(
                AgentRouterKind::for_inner_path(repair),
                AgentRouterKind::Repair,
                "{repair}"
            );
        }
    }

    #[test]
    fn file_routes_are_history_usage_the_user_inbox_and_the_raw_a2a_settings() {
        for files in [
            "/api/chat/history",
            "/api/usage",
            "/api/a2a/agents/raw",
            "/api/inbox",
            "/api/inbox/archive",
            "/api/inbox/2026-09-30-note/read",
            "/api/inbox/2026-09-30-note/archive",
            "/api/inbox/2026-09-30-note/restore",
            "/api/inbox/2026-09-30-note/attachments/0",
            "/api/inbox/archive/read",
        ] {
            assert_eq!(
                AgentRouterKind::for_inner_path(files),
                AgentRouterKind::Files,
                "{files}"
            );
        }
    }

    #[test]
    fn routes_that_need_the_live_agent_stay_running_routes() {
        for running in [
            "/ws",
            "/api/status",
            "/api/sessions",
            "/api/configuration",
            "/api/workspaces",
            "/api/files/workspace",
            "/api/agent-inbox",
            "/api/chat",
            "/api/chat/history/extra",
            "/api/usage/totals",
            "/api/a2a/agents",
            "/api/a2a/agents/raw/extra",
            "/api/a2a/status",
            "/api/a2a/card",
            "/api/a2a/outbound",
            "/api/inbox/item",
            "/api/inbox/item/unknown",
            "/api/inbox/item/attachments",
            "/api/inbox/item/attachments/0/extra",
        ] {
            assert_eq!(
                AgentRouterKind::for_inner_path(running),
                AgentRouterKind::Running,
                "{running}"
            );
        }
    }
}

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::Response;
use tower::ServiceExt;
use tracing::Instrument;

use super::error::{json_error, lifecycle_error_response};
use crate::AgentDirectory;
use residuum_background::team::LifecycleError;
use residuum_config::paths::validate_agent_name;
use residuum_net::browser_landing::{self, AppNotice, Fallback, Landing};

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

#[derive(Debug, PartialEq, Eq)]
enum AgentRouterKind {
    Repair,
    Files,
    UsageRecords,
    Running,
}

impl AgentRouterKind {
    fn for_inner_path(inner: &str) -> Self {
        let segments: Vec<&str> = inner.trim_start_matches('/').split('/').collect();
        match segments.as_slice() {
            [
                "api",
                "config" | "providers" | "mcp" | "workspace" | "checkpoints" | "teams-setup",
                ..,
            ] => Self::Repair,
            ["api", "usage", "records" | "summary"] => Self::UsageRecords,
            ["api", route @ ..] if is_file_data_route(route) => Self::Files,
            _ => Self::Running,
        }
    }
}

fn is_file_data_route(route: &[&str]) -> bool {
    match route {
        ["chat", "history"] | ["usage" | "context"] | ["a2a", "agents", "raw"] | ["files", _] => {
            true
        }
        ["inbox", rest @ ..] => {
            matches!(rest, [_, "attachments", _] | ["agent", _, "attachments", _])
        }
        _ => false,
    }
}

fn agent_inner_path(after_name: &str) -> String {
    if after_name == "/ws" {
        "/ws".to_string()
    } else {
        format!("/api{after_name}")
    }
}

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
        AgentRouterKind::UsageRecords => directory.agent_file_router(name),
        AgentRouterKind::Running => directory.agent_router(name),
    };
    forward(name, router, req, &inner).await
}

async fn webhook_request(directory: &dyn AgentDirectory, req: Request) -> Response {
    let path = req.uri().path().to_string();
    if browser_landing::is_top_level_navigation(req.method(), req.headers()) {
        let agent = path.trim_start_matches('/').split('/').next();
        return webhook_address_opened(agent);
    }
    let Some((agent, webhook)) = path
        .strip_prefix('/')
        .and_then(|rest| rest.split_once('/'))
        .filter(|(agent, webhook)| !agent.is_empty() && !webhook.is_empty())
    else {
        return json_error(
            StatusCode::NOT_FOUND,
            "A webhook address names its agent and webhook: /webhook/{agent}/{name}.",
        );
    };
    if req.method() != Method::POST {
        let mut response = json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Webhooks receive POST requests from other services.",
        );
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return response;
    }
    let inner = format!("/webhook/{webhook}");
    forward(agent, directory.agent_router(agent), req, &inner).await
}

fn webhook_address_opened(agent: Option<&str>) -> Response {
    let notice = ("notice", AppNotice::WebhookAddress.code());
    let target = match agent.filter(|name| validate_agent_name(name).is_ok()) {
        Some(agent) => {
            let settings = format!("{agent}/connections");
            browser_landing::app_path(
                &format!("/agent/{agent}"),
                &[("settings", &settings), notice],
            )
        }
        None => browser_landing::app_path("/home", &[notice]),
    };
    browser_landing::app_redirect(
        &target,
        Fallback {
            title: "Webhook address",
            message: "This is a webhook address: other services send POST requests to it to give an agent work. There's nothing to open here in a browser.",
        },
    )
}

fn split_agent_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix('/')?;
    if after.is_empty() {
        return None;
    }
    Some(after.find('/').map_or((after, ""), |i| after.split_at(i)))
}

async fn forward(
    agent: &str,
    router: Result<Router, LifecycleError>,
    mut req: Request,
    inner_path: &str,
) -> Response {
    let router = match router {
        Ok(router) => router,
        Err(e) if browser_landing::is_top_level_navigation(req.method(), req.headers()) => {
            return agent_unavailable_to_browser(agent, &e);
        }
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
        .instrument(crate::launch::agent_span(agent))
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

fn agent_unavailable_to_browser(agent: &str, error: &LifecycleError) -> Response {
    let agent_page = validate_agent_name(agent)
        .is_ok()
        .then(|| format!("/agent/{agent}"));
    match (error, agent_page) {
        (LifecycleError::NotRunning { .. }, Some(page)) => browser_landing::app_redirect(
            &browser_landing::app_path(&page, &[("notice", AppNotice::AgentStopped.code())]),
            Fallback {
                title: "Agent stopped",
                message: "This link needs its agent running. Start the agent in Residuum, then open the link again.",
            },
        ),
        (LifecycleError::NotFound(_), page) => browser_landing::app_redirect(
            page.as_deref().unwrap_or("/home"),
            Fallback {
                title: "No such agent",
                message: "There's no agent by that name in Residuum. It may have been renamed or deleted.",
            },
        ),
        (LifecycleError::ShuttingDown(message), _) => browser_landing::fallback_page(
            StatusCode::SERVICE_UNAVAILABLE,
            &Landing::new("Residuum is shutting down", message.clone()),
        ),
        (other, _) => {
            if matches!(other, LifecycleError::Failed(_)) {
                tracing::error!(error = %other, agent, "hub request failed");
            } else {
                tracing::debug!(error = %other, agent, "a browser opened a link the agent can't answer");
            }
            browser_landing::fallback_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                &Landing::new(
                    "Couldn't open this link",
                    "Residuum couldn't open this link. Try again, and if it keeps happening, Residuum's logs have the details.",
                ),
            )
        }
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
            "/api/teams-setup/prereqs",
            "/api/teams-setup/job",
            "/api/teams-setup/job/redirect",
            "/api/teams-setup/job/cancel",
            "/api/teams-setup/job/retry",
            "/api/teams-setup/job/install-app",
            "/api/teams-setup/job/package",
            "/api/teams-setup/cleanup",
        ] {
            assert_eq!(
                AgentRouterKind::for_inner_path(repair),
                AgentRouterKind::Repair,
                "{repair}"
            );
        }
    }

    #[test]
    fn file_routes_are_history_usage_the_context_breakdown_inbox_attachments_the_raw_a2a_settings_and_file_links()
     {
        for files in [
            "/api/chat/history",
            "/api/usage",
            "/api/context",
            "/api/a2a/agents/raw",
            "/api/files/workspace",
            "/api/files/abc123",
            "/api/inbox/2026-09-30-note/attachments/0",
            "/api/inbox/agent/2026-09-30-note/attachments/0",
        ] {
            assert_eq!(
                AgentRouterKind::for_inner_path(files),
                AgentRouterKind::Files,
                "{files}"
            );
        }
    }

    #[test]
    fn usage_records_and_summaries_are_served_from_the_file_router() {
        for usage in ["/api/usage/records", "/api/usage/summary"] {
            assert_eq!(
                AgentRouterKind::for_inner_path(usage),
                AgentRouterKind::UsageRecords,
                "{usage}"
            );
        }
        assert_eq!(
            AgentRouterKind::for_inner_path("/api/usage"),
            AgentRouterKind::Files
        );
    }

    #[test]
    fn routes_that_need_the_live_agent_stay_running_routes() {
        for running in [
            "/ws",
            "/api/status",
            "/api/sessions",
            "/api/configuration",
            "/api/workspaces",
            "/api/files",
            "/api/files/abc123/extra",
            "/api/agent-inbox",
            "/api/chat",
            "/api/chat/history/extra",
            "/api/usage/totals",
            "/api/context/extra",
            "/api/a2a/agents",
            "/api/a2a/agents/raw/extra",
            "/api/a2a/status",
            "/api/a2a/card",
            "/api/a2a/outbound",
            "/api/inbox",
            "/api/inbox/archive",
            "/api/inbox/item",
            "/api/inbox/item/read",
            "/api/inbox/item/unknown",
            "/api/inbox/agent/item/attachments",
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

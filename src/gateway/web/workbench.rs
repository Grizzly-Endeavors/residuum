//! Workbench API: list, serve, and delete the agent's workbench tools.
//!
//! Tool pages are served with a CSP `sandbox` header, so a page runs in an
//! opaque origin whether the web UI frames it or someone opens its URL
//! directly: it cannot read gateway responses, and the cross-site guard
//! rejects anything state-changing it sends. Tools reach the gateway only
//! through the web UI's bridge (`web/src/lib/workbench-bridge.ts`), which
//! decides what they may call.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Serialize;

use crate::gateway::protocol::WorkbenchToolSummary;
use crate::workbench::{self, ToolDeleteError, ToolPageError};

/// Sandbox flags for tool pages. Must match the `sandbox` attribute on the
/// web UI's tool frame; the browser applies the intersection of both.
const TOOL_PAGE_CSP: &str =
    "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";

#[derive(Clone)]
pub(crate) struct WorkbenchApiState {
    /// `<workspace>/workbench`.
    pub dir: PathBuf,
}

/// Response from `DELETE /api/workbench/tools/{name}`.
#[derive(Debug, Serialize)]
struct DeleteToolResponse {
    /// File names removed: the page and any `<name>.*` data files.
    removed: Vec<String>,
}

pub(crate) fn workbench_api_router(state: WorkbenchApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/workbench/tools", get(api_workbench_tools))
        .route(
            "/api/workbench/tools/{name}",
            get(api_workbench_tool_page).delete(api_workbench_tool_delete),
        )
        .with_state(state)
}

/// `GET /api/workbench/tools` — every tool, most recently modified first.
async fn api_workbench_tools(
    State(state): State<WorkbenchApiState>,
) -> Result<Json<Vec<WorkbenchToolSummary>>, (StatusCode, String)> {
    workbench::list_tools(&state.dir).await.map(Json).map_err(|e| {
        tracing::error!(dir = %state.dir.display(), error = %e, "failed to list workbench tools");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Couldn't read the workbench folder. Check that the workspace is readable and try again."
                .to_string(),
        )
    })
}

/// `GET /api/workbench/tools/{name}` — the tool's page, SDK injected, sandboxed.
async fn api_workbench_tool_page(
    State(state): State<WorkbenchApiState>,
    Path(name): Path<String>,
) -> Response {
    match workbench::read_tool_page(&state.dir, &name).await {
        Ok(page) => {
            let mut resp = page.into_response();
            let headers = resp.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(TOOL_PAGE_CSP),
            );
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            resp
        }
        Err(e) => {
            let status = match &e {
                ToolPageError::InvalidName(_) => StatusCode::BAD_REQUEST,
                ToolPageError::NotFound(_) => StatusCode::NOT_FOUND,
                ToolPageError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
                ToolPageError::Io { .. } => {
                    tracing::error!(tool = %name, error = %e, "failed to serve workbench tool");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            (status, error_page(&e.to_string())).into_response()
        }
    }
}

/// A minimal page for errors, since the response lands in the tool frame.
fn error_page(message: &str) -> (header::HeaderMap, String) {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'; style-src 'unsafe-inline'"),
    );
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    (
        headers,
        format!(
            "<!doctype html><meta charset=utf-8><body style=\"font:14px sans-serif;color:#a8a29e;background:#12100e;padding:2rem\"><p>{escaped}</p></body>"
        ),
    )
}

/// `DELETE /api/workbench/tools/{name}` — remove the tool and its data files.
async fn api_workbench_tool_delete(
    State(state): State<WorkbenchApiState>,
    Path(name): Path<String>,
) -> Result<Json<DeleteToolResponse>, (StatusCode, String)> {
    match workbench::delete_tool(&state.dir, &name).await {
        Ok(removed) => {
            tracing::info!(tool = %name, files = ?removed, "deleted workbench tool");
            Ok(Json(DeleteToolResponse { removed }))
        }
        Err(e @ ToolDeleteError::InvalidName(_)) => Err((StatusCode::BAD_REQUEST, e.to_string())),
        Err(ToolDeleteError::NotFound(_)) => Err((
            StatusCode::NOT_FOUND,
            "That tool no longer exists. It may already have been deleted.".to_string(),
        )),
        Err(e @ ToolDeleteError::Io { .. }) => {
            tracing::error!(tool = %name, error = %e, "failed to delete workbench tool");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't delete the tool. Some of its files may remain in the workbench folder; check the workspace permissions and try again.".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    fn app(dir: &std::path::Path) -> axum::Router {
        workbench_api_router(WorkbenchApiState {
            dir: dir.to_path_buf(),
        })
    }

    async fn body_text(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn tool_page_is_sandboxed_and_carries_the_sdk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("chart.html"),
            "<html><head><title>Chart</title></head></html>",
        )
        .unwrap();
        let resp = app(dir.path())
            .oneshot(
                Request::get("/api/workbench/tools/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            TOOL_PAGE_CSP
        );
        assert!(
            !TOOL_PAGE_CSP.contains("allow-same-origin"),
            "tools must never share the gateway's origin"
        );
        assert!(body_text(resp).await.contains("window.residuum"));
    }

    #[tokio::test]
    async fn missing_and_invalid_tools_get_sandboxed_error_pages() {
        let dir = tempfile::tempdir().unwrap();
        for (path, status) in [
            ("/api/workbench/tools/nope", StatusCode::NOT_FOUND),
            ("/api/workbench/tools/Bad.Name", StatusCode::BAD_REQUEST),
        ] {
            let resp = app(dir.path())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), status, "{path}");
            assert!(
                resp.headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("sandbox"),
            );
        }
    }

    #[tokio::test]
    async fn list_then_delete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.html"), "<title>My Chart</title>").unwrap();
        std::fs::write(dir.path().join("chart.state.json"), "{}").unwrap();

        let listing = app(dir.path())
            .oneshot(
                Request::get("/api/workbench/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let tools: Vec<WorkbenchToolSummary> =
            serde_json::from_str(&body_text(listing).await).unwrap();
        let [tool] = tools.as_slice() else {
            panic!("expected exactly one tool, got {tools:?}");
        };
        assert_eq!(tool.name, "chart");
        assert_eq!(tool.title, "My Chart");

        let deleted = app(dir.path())
            .oneshot(
                Request::delete("/api/workbench/tools/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_text(deleted).await).unwrap();
        assert_eq!(
            body.get("removed"),
            Some(&serde_json::json!(["chart.html", "chart.state.json"]))
        );

        let deleted_again = app(dir.path())
            .oneshot(
                Request::delete("/api/workbench/tools/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted_again.status(), StatusCode::NOT_FOUND);
    }
}

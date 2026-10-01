//! What a page on the artifacts origin can't ask the hub to do.
//!
//! The artifacts listener forwards `/api` to this router with an
//! [`ArtifactsOrigin`] marker on the request (see `workbench::forward`). An
//! artifact page can call everything the web UI can, except the routes listed
//! in [`BLOCKED_ROUTES`]: they end or reconfigure the whole process, and a page
//! that an agent wrote has no business doing that. The refusal is a `403` with
//! a plain-language `{ "error" }`, so the page can show why.
//!
//! The marker is a request extension, so a client can't send it, and a request
//! that reaches the router any other way never has it.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use super::error::json_error;
use crate::workbench::forward::ArtifactsOrigin;

/// The hub routes a page on the artifacts origin is refused, whatever the
/// method: shutdown, stop-all, update check, apply and restart, and setup
/// completion. Routes match on their exact path, which the router does too.
const BLOCKED_ROUTES: [&str; 6] = [
    "/api/hub/shutdown",
    "/api/hub/stop-all",
    "/api/hub/update/check",
    "/api/hub/update/apply",
    "/api/hub/update/restart",
    "/api/hub/config/complete-setup",
];

const REFUSAL_MESSAGE: &str = "Pages opened from the workbench can't shut down, stop, update, or \
set up Residuum. Do that from the Residuum app.";

/// Refuse a request that arrived through the artifacts origin to a route in
/// [`BLOCKED_ROUTES`]. Every other request passes untouched.
pub(super) async fn refuse_blocked_artifact_calls(req: Request, next: Next) -> Response {
    if req.extensions().get::<ArtifactsOrigin>().is_some() && is_blocked(req.uri().path()) {
        tracing::warn!(
            method = %req.method(),
            path = %req.uri().path(),
            "refused a workbench page's request to a route artifacts can't use"
        );
        return json_error(StatusCode::FORBIDDEN, REFUSAL_MESSAGE);
    }
    next.run(req).await
}

fn is_blocked(path: &str) -> bool {
    BLOCKED_ROUTES.contains(&path)
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::routing::any;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn only_the_listed_routes_are_blocked() {
        for path in BLOCKED_ROUTES {
            assert!(is_blocked(path), "{path}");
        }
        for path in [
            "/api/hub/update/status",
            "/api/hub/agents",
            "/api/hub/agents/scout/stop",
            "/api/hub/cloud/disconnect",
            "/api/hub/config/raw",
            "/api/agents/scout/status",
            "/api/hub/shutdown/now",
        ] {
            assert!(!is_blocked(path), "{path}");
        }
    }

    #[tokio::test]
    async fn a_marked_request_is_refused_on_each_blocked_route_and_an_unmarked_one_is_not() {
        let mut app = Router::new().route("/api/hub/agents", any(|| async { "reached" }));
        for path in BLOCKED_ROUTES {
            app = app.route(path, any(|| async { "reached" }));
        }
        let app = app.layer(axum::middleware::from_fn(refuse_blocked_artifact_calls));

        for path in BLOCKED_ROUTES {
            let unmarked = Request::post(path).body(Body::empty()).unwrap();
            let allowed = app.clone().oneshot(unmarked).await.unwrap();
            assert_eq!(allowed.status(), StatusCode::OK, "{path} unmarked");

            let mut marked = Request::post(path).body(Body::empty()).unwrap();
            marked.extensions_mut().insert(ArtifactsOrigin);
            let refused = app.clone().oneshot(marked).await.unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{path} marked");
        }

        let mut other_route = Request::get("/api/hub/agents").body(Body::empty()).unwrap();
        other_route.extensions_mut().insert(ArtifactsOrigin);
        let passed = app.oneshot(other_route).await.unwrap();
        assert_eq!(passed.status(), StatusCode::OK);
    }
}

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use super::error::json_error;
use residuum_net::workbench::ArtifactsOrigin;

const BLOCKED_ROUTES: [&str; 8] = [
    "/api/hub/shutdown",
    "/api/hub/stop-all",
    "/api/hub/update/check",
    "/api/hub/update/apply",
    "/api/hub/update/restart",
    "/api/hub/config/complete-setup",
    "/api/hub/cloud/connect",
    "/api/hub/cloud/disconnect",
];

const BLOCKED_PREFIXES: [&str; 2] = ["/api/hub/devices", "/api/hub/remote-access"];

const REFUSAL_MESSAGE: &str = "Pages opened from the workbench can't shut down, stop, update, set up, \
or manage who can reach Residuum. Do that from the Residuum app.";

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
        || BLOCKED_PREFIXES.iter().any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
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
            "/api/hub/cloud/status",
            "/api/hub/cloud/connectx",
            "/api/hub/config/raw",
            "/api/agents/scout/status",
            "/api/hub/shutdown/now",
            "/api/hub/pairing/handoff",
            "/api/hub/devicesx",
        ] {
            assert!(!is_blocked(path), "{path}");
        }
    }

    #[test]
    fn the_device_and_remote_access_routes_are_blocked_at_and_below_their_prefix() {
        for path in [
            "/api/hub/devices",
            "/api/hub/devices/abc",
            "/api/hub/devices/pending/abc/approve",
            "/api/hub/devices/recovery-codes",
            "/api/hub/remote-access/pair-link",
        ] {
            assert!(is_blocked(path), "{path}");
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

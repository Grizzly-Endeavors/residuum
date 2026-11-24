use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::pairing::remote::remote_context;

const REFUSAL_MESSAGE: &str = "Shutting down or disconnecting can't be done remotely, because \
nothing could bring Residuum back. Do it on the machine running Residuum.";

pub async fn reject_remote_shutdown_and_disconnect(req: Request, next: Next) -> Response {
    if remote_context(req.extensions()).is_some() {
        tracing::warn!(
            path = %req.uri().path(),
            "refused a shutdown or cloud-disconnect request that arrived through the tunnel"
        );
        return (StatusCode::FORBIDDEN, REFUSAL_MESSAGE).into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::remote::RemoteTransport;
    use axum::Router;
    use axum::routing::post;
    use tower::ServiceExt;

    fn test_router() -> Router {
        Router::new()
            .route("/api/hub/shutdown", post(|| async { "ok" }))
            .route("/api/hub/cloud/disconnect", post(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn(
                reject_remote_shutdown_and_disconnect,
            ))
            .route("/api/update/restart", post(|| async { "ok" }))
    }

    fn post_to(path: &str, remote: bool) -> Request {
        let mut req = Request::post(path).body(axum::body::Body::empty()).unwrap();
        if remote {
            req.extensions_mut().insert(RemoteTransport {
                peer_ip: Some("203.0.113.9".to_string()),
                origin: Some("https://bear.agent-residuum.com".to_string()),
            });
        }
        req
    }

    #[tokio::test]
    async fn a_local_shutdown_or_disconnect_is_allowed() {
        for path in ["/api/hub/shutdown", "/api/hub/cloud/disconnect"] {
            let resp = test_router().oneshot(post_to(path, false)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn a_remote_shutdown_or_disconnect_is_refused() {
        for path in ["/api/hub/shutdown", "/api/hub/cloud/disconnect"] {
            let resp = test_router().oneshot(post_to(path, true)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
        }
    }

    #[tokio::test]
    async fn headers_a_client_sends_do_not_make_a_local_request_remote() {
        let mut req = post_to("/api/hub/shutdown", false);
        for name in ["x-residuum-tunnel", "x-real-ip", "x-forwarded-for"] {
            req.headers_mut()
                .insert(name, axum::http::HeaderValue::from_static("203.0.113.9"));
        }
        let resp = test_router().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn restart_route_is_unaffected_by_the_guard() {
        let resp = test_router()
            .oneshot(post_to("/api/update/restart", true))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the guard must be scoped to shutdown/disconnect only"
        );
    }
}

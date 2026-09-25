//! Guard against shutting down or disconnecting the gateway over the tunnel.
//!
//! Observed failure this exists to prevent: a remote shutdown or cloud
//! disconnect executed through the tunnel leaves nothing that can bring the
//! gateway back, since both actions cut off the only channel a remote caller
//! has to reach it. Restart and update stay reachable remotely — only these
//! two irreversible-from-a-distance actions are refused.
//!
//! Tunnel-forwarded requests carry [`crate::tunnel::TUNNEL_NONCE_HEADER`] set
//! to this process's own nonce (see `tunnel::forward_http::forward`), the
//! same mechanism the A2A listener uses to attest sibling requests. A local
//! request never carries a value that matches: the nonce is generated fresh
//! per process and never leaves it except over the loopback hop the tunnel
//! forwarder itself makes, so nothing a client sends — over the tunnel or
//! directly to a local port — can forge a match.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::tunnel::{TUNNEL_NONCE_HEADER, tunnel_nonce};

const REFUSAL_MESSAGE: &str = "Shutting down or disconnecting can't be done remotely, because \
nothing could bring Residuum back. Do it on the machine running Residuum.";

/// Whether `req` arrived through this instance's own tunnel connection.
fn is_tunnel_forwarded(req: &Request) -> bool {
    req.headers()
        .get(TUNNEL_NONCE_HEADER)
        .is_some_and(|v| v.as_bytes() == tunnel_nonce().as_bytes())
}

/// Refuse a tunnel-forwarded request to a route this guard is mounted on.
///
/// Apply with `.route_layer(...)` to exactly `/api/shutdown` and
/// `/api/cloud/disconnect` — never with `.layer(...)`, which would apply it
/// to the whole router instead of just those two routes.
pub(crate) async fn reject_remote_shutdown_and_disconnect(req: Request, next: Next) -> Response {
    if is_tunnel_forwarded(&req) {
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
    use axum::Router;
    use axum::routing::post;
    use tower::ServiceExt;

    fn test_router() -> Router {
        Router::new()
            .route("/api/shutdown", post(|| async { "ok" }))
            .route_layer(axum::middleware::from_fn(
                reject_remote_shutdown_and_disconnect,
            ))
            .route("/api/update/restart", post(|| async { "ok" }))
    }

    #[tokio::test]
    async fn local_shutdown_request_is_allowed() {
        let req = Request::post("/api/shutdown").body(String::new()).unwrap();
        let resp = test_router().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tunnel_forwarded_shutdown_request_is_refused() {
        let req = Request::post("/api/shutdown")
            .header(TUNNEL_NONCE_HEADER, tunnel_nonce())
            .body(String::new())
            .unwrap();
        let resp = test_router().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn client_supplied_marker_with_wrong_value_is_ignored() {
        // A client can send this header itself, but only the process's own
        // nonce (which it can never know) counts as tunnel-forwarded.
        let req = Request::post("/api/shutdown")
            .header(TUNNEL_NONCE_HEADER, "not-the-real-nonce")
            .body(String::new())
            .unwrap();
        let resp = test_router().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn restart_route_is_unaffected_by_the_guard() {
        let req = Request::post("/api/update/restart")
            .header(TUNNEL_NONCE_HEADER, tunnel_nonce())
            .body(String::new())
            .unwrap();
        let resp = test_router().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the guard must be scoped to shutdown/disconnect only"
        );
    }
}

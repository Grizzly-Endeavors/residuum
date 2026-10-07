//! Guard against shutting down the hub or disconnecting it from the cloud over
//! the tunnel.
//!
//! Observed failure this exists to prevent: a remote shutdown or cloud
//! disconnect executed through the tunnel leaves nothing that can bring the
//! gateway back, since both actions cut off the only channel a remote caller
//! has to reach it. Every other action stays reachable remotely, including
//! restart, update, and stopping agents (one or all), because the hub keeps
//! running after those and any agent can be started again through the tunnel.
//!
//! A request is remote when a transport marked it (see `pairing::remote`).
//! The legacy tunnel adds [`crate::tunnel::TUNNEL_NONCE_HEADER`] set to this
//! process's own nonce (see `tunnel::forward_http::forward`), and the secure
//! tunnel's engine adds a request extension no client can send. A local
//! request carries neither: the nonce is generated fresh per process and never
//! leaves it except over the loopback hop the tunnel forwarder itself makes, so
//! nothing a client sends, over a tunnel or directly to a local port, can forge
//! a match.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::pairing::remote::remote_context;

const REFUSAL_MESSAGE: &str = "Shutting down or disconnecting can't be done remotely, because \
nothing could bring Residuum back. Do it on the machine running Residuum.";

/// Refuse a tunnel-forwarded request to a route this guard is mounted on.
///
/// Apply with `.route_layer(...)` to exactly `/api/hub/shutdown` and
/// `/api/hub/cloud/disconnect` — never with
/// `.layer(...)`, which would apply it to the whole router instead of just
/// those routes.
pub(crate) async fn reject_remote_shutdown_and_disconnect(req: Request, next: Next) -> Response {
    if remote_context(req.headers(), req.extensions()).is_some() {
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
    use crate::tunnel::{TUNNEL_NONCE_HEADER, tunnel_nonce};
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

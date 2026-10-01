//! API forwarding on the artifacts origin.
//!
//! An artifact page runs on the artifacts listener's own origin, so its calls
//! to Residuum arrive at that listener, not at the gateway. The listener
//! answers `/api` and `/api/...`, WebSocket upgrades included, by handing the
//! request in-process to the same hub router the gateway serves. Nothing else
//! is forwarded: the router's embedded web app, webhooks and relay callback
//! are not reachable through the artifacts origin.
//!
//! The listener starts before the hub router exists (the router needs the
//! listener's port), so it holds a [`HubApi`], a handle the runtime binds once
//! the router is built.
//!
//! Every forwarded request carries [`ArtifactsOrigin`], which the hub's routes
//! read to refuse the calls an artifact can't make and to keep an artifact's
//! agent sockets out of the activity count.

use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::any;
use serde_json::json;
use tower::ServiceExt;

/// Marks a request as arriving through the artifacts origin.
///
/// It is a request extension, not a header: only the listener's forwarder
/// inserts it, in-process, so nothing a client sends can set or forge it. The
/// tunnel's loopback hop to the gateway is an ordinary HTTP request and never
/// carries it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArtifactsOrigin;

/// The hub router, bound after the artifacts listener has started.
#[derive(Clone, Default)]
pub(crate) struct HubApi {
    router: Arc<OnceLock<Router>>,
}

impl HubApi {
    /// An unbound handle. Forwarded requests answer `503` until [`Self::bind`].
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Bind the hub router. The first call wins; binding again is a bug in the
    /// caller, logged and otherwise ignored.
    pub(crate) fn bind(&self, router: Router) {
        if self.router.set(router).is_err() {
            tracing::error!(
                "the hub router was bound to the artifacts listener twice; keeping the first"
            );
        }
    }

    /// Hand `req` to the hub router as a request arriving through the
    /// artifacts origin.
    async fn dispatch(&self, mut req: Request) -> Response {
        let Some(router) = self.router.get() else {
            tracing::warn!(
                path = %req.uri().path(),
                "a workbench page called the API before the hub router was ready"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "Residuum is still starting. Try again in a moment." })),
            )
                .into_response();
        };
        // The listener's router recorded the `/api/{*rest}` route that matched.
        // The hub router routes the request afresh, and axum refuses to route
        // into a mounted service (the agent routes) with a stale match.
        req.extensions_mut().remove::<MatchedPath>();
        req.extensions_mut().insert(ArtifactsOrigin);
        match router.clone().oneshot(req).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    }
}

/// The routes that forward to the hub router: `/api` and everything below it.
///
/// A folder artifact can't be called `api` (see `is_valid_artifact_name`), so
/// these never shadow an artifact's page.
pub(super) fn routes(api: HubApi) -> Router {
    Router::new()
        .route("/api", any(forward))
        .route("/api/", any(forward))
        .route("/api/{*rest}", any(forward))
        .with_state(api)
}

async fn forward(State(api): State<HubApi>, req: Request) -> Response {
    api.dispatch(req).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::routing::get;

    use super::*;

    /// A hub stand-in that reports whether the request carried the marker.
    fn stand_in() -> Router {
        Router::new().route(
            "/api/probe",
            get(|req: Request| async move {
                if req.extensions().get::<ArtifactsOrigin>().is_some() {
                    "marked"
                } else {
                    "unmarked"
                }
            }),
        )
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn an_unbound_handle_answers_503_with_a_plain_message() {
        let api = HubApi::new();
        let response = routes(api)
            .oneshot(Request::get("/api/probe").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(body_text(response).await.contains("still starting"));
    }

    #[tokio::test]
    async fn a_bound_handle_dispatches_with_the_marker() {
        let api = HubApi::new();
        let forwarder = routes(api.clone());
        api.bind(stand_in());
        let response = forwarder
            .oneshot(Request::get("/api/probe").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "marked");
    }

    #[tokio::test]
    async fn a_mounted_service_of_the_hub_router_serves_a_forwarded_request() {
        let agents = tower::service_fn(|_req: Request| async {
            Ok::<_, std::convert::Infallible>("agent".into_response())
        });
        let hub = Router::new().nest_service("/api/agents", agents);
        let api = HubApi::new();
        api.bind(hub);
        let response = routes(api)
            .oneshot(
                Request::get("/api/agents/scout/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_text(response).await, "agent");
    }

    #[tokio::test]
    async fn the_first_binding_is_kept() {
        let api = HubApi::new();
        api.bind(stand_in());
        api.bind(Router::new());
        let response = routes(api)
            .oneshot(Request::get("/api/probe").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn only_api_paths_are_forwarded() {
        let api = HubApi::new();
        api.bind(stand_in());
        for path in ["/", "/webhook/scout/x", "/cloud/callback", "/assets/app.js"] {
            let response = routes(api.clone())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}

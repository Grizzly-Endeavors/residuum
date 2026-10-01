//! API forwarding on the artifacts origin.
//!
//! An artifact page runs on the artifacts listener's own origin, so its calls
//! to Residuum arrive at that listener, not at the gateway. The listener
//! answers `/api` and `/api/...`, WebSocket upgrades included, by handing the
//! request in-process to the same hub router the gateway serves. Nothing else
//! is forwarded: the router's embedded web app, webhooks and relay callback
//! are not reachable through the artifacts origin.
//!
//! The hand-off happens before any routing of the listener's own, so the hub
//! router gets the request exactly as the listener received it. A route such
//! as `/api/{*rest}` would leave `rest` on the request as a path parameter, and
//! the hub router's own match appends to those, so every `Path` extractor in
//! the hub would count one parameter too many. It would also leave the matched
//! route behind, which axum's debug assertions reject when the request then
//! enters a mounted service such as the agent routes.
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
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
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
        req.extensions_mut().insert(ArtifactsOrigin);
        match router.clone().oneshot(req).await {
            Ok(response) => response,
            Err(never) => match never {},
        }
    }
}

/// Put the API in front of `artifacts`: `/api` and everything below it goes to
/// the hub router, every other path to `artifacts`.
///
/// A folder artifact can't be called `api` (see `is_valid_artifact_name`), so
/// the API never shadows an artifact's page.
///
/// The split is a middleware over a router that has no routes of its own, not
/// a route, so nothing matches the request before the hub router does (see the
/// module docs for what a match would leave behind).
pub(super) fn forwarding(artifacts: Router, api: HubApi) -> Router {
    Router::new()
        .fallback_service(artifacts)
        .layer(axum::middleware::from_fn_with_state(api, split_api))
}

async fn split_api(State(api): State<HubApi>, req: Request, next: Next) -> Response {
    if is_api_path(req.uri().path()) {
        api.dispatch(req).await
    } else {
        next.run(req).await
    }
}

/// Whether `path` is `/api` or below it. `/apix` and `/api-tools` are not.
fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

#[cfg(test)]
mod tests {
    use axum::Extension;
    use axum::body::Body;
    use axum::extract::Path;
    use axum::routing::get;

    use super::*;

    /// An extension the forwarder knows nothing about, standing in for the
    /// ones axum, hyper and the server put on a request (`OnUpgrade`,
    /// `ConnectInfo`).
    #[derive(Debug, Clone, Copy)]
    struct Carried(u8);

    /// A hub stand-in: `/api/probe` reports whether the request carried the
    /// marker, `/api/carried` what `Carried` held, and the routes with path
    /// parameters answer with what their `Path` extractors saw.
    fn stand_in() -> Router {
        Router::new()
            .route(
                "/api/probe",
                get(|req: Request| async move {
                    if req.extensions().get::<ArtifactsOrigin>().is_some() {
                        "marked"
                    } else {
                        "unmarked"
                    }
                }),
            )
            .route(
                "/api/carried",
                get(|carried: Option<Extension<Carried>>| async move {
                    carried.map_or_else(|| "none".to_string(), |Extension(c)| c.0.to_string())
                }),
            )
            .route(
                "/api/x/{id}",
                get(|Path(id): Path<String>| async move { id }),
            )
            .route(
                "/api/y/{first}/{second}/z",
                get(|Path((first, second)): Path<(String, String)>| async move {
                    format!("{first}+{second}")
                }),
            )
    }

    /// The artifacts side: routes shaped like the real ones, which would
    /// match `/api/...` if the API weren't split off first, and which answer
    /// "artifact" to tell them from the hub's answers.
    fn artifacts_stand_in() -> Router {
        Router::new()
            .route("/{name}/{*rest}", get(|| async { "artifact" }))
            .fallback(|| async { "artifact" })
    }

    fn listener(api: &HubApi) -> Router {
        forwarding(artifacts_stand_in(), api.clone())
    }

    async fn get_body(app: Router, path: &str) -> (StatusCode, String) {
        let response = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn an_unbound_handle_answers_503_with_a_plain_message() {
        let api = HubApi::new();
        let (status, body) = get_body(listener(&api), "/api/probe").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("still starting"), "{body}");
    }

    #[tokio::test]
    async fn a_bound_handle_dispatches_with_the_marker() {
        let api = HubApi::new();
        let forwarder = listener(&api);
        api.bind(stand_in());
        let (status, body) = get_body(forwarder, "/api/probe").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "marked");
    }

    #[tokio::test]
    async fn path_parameters_of_hub_routes_reach_their_extractors() {
        let api = HubApi::new();
        api.bind(stand_in());
        for (path, expected) in [("/api/x/42", "42"), ("/api/y/ab/cd/z", "ab+cd")] {
            let (status, body) = get_body(listener(&api), path).await;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
            assert_eq!(body, expected, "{path}");
        }
    }

    #[tokio::test]
    async fn a_request_reaches_the_hub_router_with_the_extensions_it_arrived_with() {
        let api = HubApi::new();
        api.bind(stand_in());
        let mut request = Request::get("/api/carried").body(Body::empty()).unwrap();
        request.extensions_mut().insert(Carried(7));
        let response = listener(&api).oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes, "7");
    }

    #[tokio::test]
    async fn a_mounted_service_of_the_hub_router_serves_a_forwarded_request() {
        let agents = tower::service_fn(|_req: Request| async {
            Ok::<_, std::convert::Infallible>("agent".into_response())
        });
        let hub = Router::new().nest_service("/api/agents", agents);
        let api = HubApi::new();
        api.bind(hub);
        let (_, body) = get_body(listener(&api), "/api/agents/scout/status").await;
        assert_eq!(body, "agent");
    }

    #[tokio::test]
    async fn the_first_binding_is_kept() {
        let api = HubApi::new();
        api.bind(stand_in());
        api.bind(Router::new());
        let (status, _) = get_body(listener(&api), "/api/probe").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn only_api_paths_go_to_the_hub_router() {
        let api = HubApi::new();
        api.bind(stand_in());
        for path in [
            "/",
            "/webhook/scout/x",
            "/cloud/callback",
            "/assets/app.js",
            "/apix/probe",
            "/api-tools/probe",
            "/%61pi/probe",
        ] {
            let (_, body) = get_body(listener(&api), path).await;
            assert_eq!(body, "artifact", "{path} is the artifacts side's");
        }
        for path in ["/api", "/api/", "/api/probe", "/api/no/such/route"] {
            let (_, body) = get_body(listener(&api), path).await;
            assert_ne!(body, "artifact", "{path} is the hub's");
        }
    }
}

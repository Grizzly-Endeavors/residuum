use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;
use tower::ServiceExt;

use super::server::UiAddress;
use crate::browser_landing::{self, AppRedirect, Landing};

#[derive(Debug, Clone, Copy)]
pub struct ArtifactsOrigin;

#[derive(Clone, Default)]
pub struct HubApi {
    router: Arc<OnceLock<Router>>,
}

impl HubApi {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&self, router: Router) {
        if self.router.set(router).is_err() {
            tracing::error!(
                "the hub router was bound to the artifacts listener twice; keeping the first"
            );
        }
    }

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

pub(super) fn forwarding(artifacts: Router, api: HubApi, ui: UiAddress) -> Router {
    Router::new()
        .fallback_service(artifacts)
        .layer(axum::middleware::from_fn_with_state((api, ui), split_api))
}

async fn split_api(
    State((api, ui)): State<(HubApi, UiAddress)>,
    req: Request,
    next: Next,
) -> Response {
    if !is_api_path(req.uri().path()) {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let origin = ui.origin_for(&parts);
    let response = api.dispatch(Request::from_parts(parts, body)).await;
    match response.extensions().get::<AppRedirect>().copied() {
        Some(redirect) => to_app_origin(response, redirect, origin.as_deref()),
        None => response,
    }
}

fn to_app_origin(mut response: Response, redirect: AppRedirect, origin: Option<&str>) -> Response {
    let target = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .filter(|target| target.starts_with('/') && !target.starts_with("//"))
        .map(str::to_owned);
    if let (Some(origin), Some(target)) = (origin, target)
        && let Ok(location) = HeaderValue::from_str(&format!("{origin}{target}"))
    {
        response.headers_mut().insert(header::LOCATION, location);
        return response;
    }
    browser_landing::fallback_page(StatusCode::NOT_FOUND, &Landing::from(redirect.fallback))
}

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

    #[derive(Debug, Clone, Copy)]
    struct Carried(u8);

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

    fn artifacts_stand_in() -> Router {
        Router::new()
            .route("/{name}/{*rest}", get(|| async { "artifact" }))
            .fallback(|| async { "artifact" })
    }

    fn listener(api: &HubApi) -> Router {
        forwarding(artifacts_stand_in(), api.clone(), ui_on_port(7700))
    }

    fn ui_on_port(port: u16) -> UiAddress {
        let no_hub = std::env::temp_dir().join("residuum-forward-tests-no-hub");
        UiAddress::new(crate::pairing::DevicePairing::open(&no_hub), port)
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

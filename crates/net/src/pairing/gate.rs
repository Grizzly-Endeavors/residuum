use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde_json::json;

use super::remote::{is_navigation, passes_cross_site_rule, remote_context};
use super::{DevicePairing, Surface};
use crate::browser_landing::{self, Landing};
use crate::workbench::forward::ArtifactsOrigin;

pub const PAIRING_PAGE_PATH: &str = "/pair";

pub(crate) const HANDOFF_PAGE_PATH: &str = "/_handoff";

const STATE_PATH: &str = "/api/hub/pairing/state";
const PRE_AUTH_POSTS: [&str; 4] = [
    "/api/hub/pairing/requests",
    "/api/hub/pairing/requests/poll",
    "/api/hub/pairing/recovery",
    "/api/hub/pairing/redeem",
];
const HANDOFF_API_PATH: &str = "/api/hub/pairing/handoff";

const CROSS_SITE_MESSAGE: &str = "This request came from another website and was blocked. Open Residuum directly to make changes.";

pub(crate) const DEVICE_REQUIRED_CODE: &str = "device_required";

#[derive(Clone)]
pub struct GateState {
    pub pairing: DevicePairing,
    pub surface: Surface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedDevice {
    pub(crate) id: String,
}

impl AuthenticatedDevice {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

pub async fn device_gate(State(gate): State<GateState>, mut req: Request, next: Next) -> Response {
    let Some(remote) = remote_context(req.extensions()) else {
        return next.run(req).await;
    };
    if gate.surface == Surface::Ui && req.extensions().get::<ArtifactsOrigin>().is_some() {
        return next.run(req).await;
    }

    let own_origin = remote
        .origin
        .or_else(|| gate.pairing.origin_of(gate.surface));
    if !passes_cross_site_rule(req.method(), req.headers(), own_origin.as_deref()) {
        tracing::warn!(
            method = %req.method(),
            path = %req.uri().path(),
            sec_fetch_site = ?req.headers().get("sec-fetch-site"),
            origin = ?req.headers().get(header::ORIGIN),
            "rejected a cross-site request that arrived through Residuum Cloud"
        );
        return (StatusCode::FORBIDDEN, CROSS_SITE_MESSAGE).into_response();
    }

    if is_pre_auth(gate.surface, req.method(), req.uri().path()) {
        return next.run(req).await;
    }

    let cookie_header = super::cookie_header(req.headers());
    let Some((hit, secret)) = gate
        .pairing
        .authenticate(gate.surface, cookie_header.as_deref())
        .await
    else {
        return unauthorized(&gate, &req);
    };

    req.extensions_mut()
        .insert(AuthenticatedDevice { id: hit.device_id });
    let mut response = next.run(req).await;
    if hit.refresh
        && response.status() != StatusCode::SWITCHING_PROTOCOLS
        && !response.headers().contains_key(header::SET_COOKIE)
        && let Ok(value) = HeaderValue::from_str(&gate.pairing.set_cookie_for(&secret))
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

fn is_pre_auth(surface: Surface, method: &Method, path: &str) -> bool {
    let is_read = matches!(*method, Method::GET | Method::HEAD);
    match surface {
        Surface::Ui => {
            (is_read && is_pairing_page_asset(path))
                || (*method == Method::GET && path == STATE_PATH)
                || (*method == Method::POST && PRE_AUTH_POSTS.contains(&path))
        }
        Surface::Workbench => {
            (is_read && path == HANDOFF_PAGE_PATH)
                || (*method == Method::POST && path == HANDOFF_API_PATH)
        }
    }
}

fn is_pairing_page_asset(path: &str) -> bool {
    if path.contains("..") {
        return false;
    }
    path == PAIRING_PAGE_PATH
        || path.starts_with("/assets/")
        || path.starts_with("/icons/")
        || path == "/favicon.svg"
        || path == "/manifest.webmanifest"
}

fn unauthorized(gate: &GateState, req: &Request) -> Response {
    if is_navigation(req.method(), req.headers()) {
        return match gate.surface {
            Surface::Ui => Redirect::to(PAIRING_PAGE_PATH).into_response(),
            Surface::Workbench => workbench_navigation_redirect(gate, req),
        };
    }
    tracing::debug!(
        method = %req.method(),
        path = %req.uri().path(),
        "refused a request through Residuum Cloud from a browser that isn't paired"
    );
    let error = match gate.pairing.origin_of(Surface::Ui) {
        Some(ui_origin) => format!(
            "This browser isn't paired with Residuum yet. Pair it at {ui_origin}{PAIRING_PAGE_PATH}."
        ),
        None => "This browser isn't paired with Residuum yet. Open Residuum's address in this browser to pair it.".to_string(),
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": error, "code": DEVICE_REQUIRED_CODE })),
    )
        .into_response()
}

fn workbench_navigation_redirect(gate: &GateState, req: &Request) -> Response {
    let ui_origin = gate.pairing.origin_of(Surface::Ui);
    let artifact = req
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .filter(|name| !name.is_empty() && crate::workbench::is_valid_artifact_name(name));
    let target = match artifact {
        Some(name) => format!("/team/workbench/{name}"),
        None => "/team/workbench".to_string(),
    };
    if let Some(ui_origin) = &ui_origin
        && browser_landing::is_top_level_navigation(req.method(), req.headers())
    {
        return browser_landing::redirect_to_app_origin(ui_origin, &target);
    }
    let link = ui_origin.map(|origin| format!("{origin}{target}"));
    browser_landing::fallback_page(
        StatusCode::UNAUTHORIZED,
        &Landing::new(
            "Open this from Residuum",
            "This artifact opens from the Workbench in Residuum, once this browser is paired with it.",
        )
        .with_app_link(link.as_deref(), "Open the Workbench"),
    )
}

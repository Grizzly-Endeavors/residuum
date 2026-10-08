//! The device gate: in front of the main and workbench routers, it refuses
//! remotely delivered requests that don't carry a paired device's credential.
//!
//! What it does for a request:
//!
//! 1. A local request (see [`super::remote`]) passes untouched.
//! 2. A remote request that changes state or opens a socket must pass the
//!    cross-site rule, before anything else is looked at.
//! 3. The pre-auth routes (the pairing page, its assets and the pairing API)
//!    pass.
//! 4. Anything else needs a valid device credential for the surface. Without
//!    one, a browser navigation is redirected to the pairing page and
//!    everything else gets `401`.
//!
//! The A2A and Teams listeners are separate servers and have no gate.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde_json::json;

use super::remote::{is_navigation, passes_cross_site_rule, remote_context};
use super::{DevicePairing, Surface};
use crate::workbench::forward::ArtifactsOrigin;

/// The pairing page's path on the UI host.
pub const PAIRING_PAGE_PATH: &str = "/pair";

/// The page on the workbench host that carries a handoff token to the host.
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

/// The machine-readable reason a `401` carries, which the web app turns into
/// a move to the pairing page.
pub const DEVICE_REQUIRED_CODE: &str = "device_required";

/// What the gate needs: the pairing state and which host it is guarding.
#[derive(Clone)]
pub(crate) struct GateState {
    pub(crate) pairing: DevicePairing,
    pub(crate) surface: Surface,
}

/// The device a request authenticated as. A request extension, set only by the
/// gate, so nothing a client sends can fake it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthenticatedDevice {
    pub(crate) id: String,
}

/// Refuse remote requests that carry no valid device credential. See the
/// module docs for the order of checks.
pub(crate) async fn device_gate(
    State(gate): State<GateState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(remote) = remote_context(req.extensions()) else {
        return next.run(req).await;
    };
    // The workbench listener already gated this request, on its own host's
    // credential, before handing the API call to the hub router in-process.
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

    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
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

/// Whether an unauthenticated remote request may reach its route.
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

/// The pairing page and the files it loads: the app shell's hashed assets,
/// icons, favicon and manifest. None of these hold anything but the app's own
/// code.
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
            Surface::Workbench => workbench_navigation_redirect(gate, req.uri().path()),
        };
    }
    tracing::debug!(
        method = %req.method(),
        path = %req.uri().path(),
        "refused a request through Residuum Cloud from a browser that isn't paired"
    );
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "This browser isn't paired with Residuum yet. Open Residuum's pairing page to pair it.",
            "code": DEVICE_REQUIRED_CODE,
        })),
    )
        .into_response()
}

/// An artifact opened directly on the workbench host goes to the same artifact
/// in the Residuum app, which opens it through the handoff once the browser is
/// paired. Without a known app address there is nowhere to send it.
fn workbench_navigation_redirect(gate: &GateState, path: &str) -> Response {
    let Some(ui_origin) = gate.pairing.origin_of(Surface::Ui) else {
        return (
            StatusCode::UNAUTHORIZED,
            "Open this artifact from the Workbench in Residuum.",
        )
            .into_response();
    };
    let artifact = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .filter(|name| !name.is_empty() && crate::workbench::is_valid_artifact_name(name));
    let target = match artifact {
        Some(name) => format!("{ui_origin}/team/workbench/{name}"),
        None => format!("{ui_origin}/team/workbench"),
    };
    Redirect::to(&target).into_response()
}

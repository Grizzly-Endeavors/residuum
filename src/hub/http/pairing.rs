//! Device pairing routes: how a browser becomes a paired device, and how the
//! paired devices are listed and revoked.
//!
//! The pairing routes (`/api/hub/pairing/...`) are the only API a remote
//! browser can reach before it is paired; the device gate lets exactly those
//! through (see `pairing::gate`). The management routes
//! (`/api/hub/devices/...`) need a paired device or a local request, like the
//! rest of the API. Minting the first device's pairing link is local only.
//!
//! The device cookie is set by these handlers. Responses that carry a secret
//! are `no-store`.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};

use super::error::json_error;
use crate::pairing::remote::{RemoteContext, remote_context};
use crate::pairing::types::{
    CreatePairingRequestBody, DeviceListResponse, HandoffBody, PairLinkResponse, PairedResponse,
    PairingPollResponse, PairingRequestCreated, PairingRequestStatus, PairingStateResponse,
    PollPairingBody, RecoveryCodeBody, RecoveryCodesResponse, RedeemTokenBody,
    WorkbenchHandoffResponse,
};
use crate::pairing::{
    AuthenticatedDevice, DevicePairing, Issued, PAIRING_PAGE_PATH, PairingError, PollOutcome,
    Surface, qr,
};
use crate::workbench::forward::ArtifactsOrigin;

const NO_STORE: (HeaderName, HeaderValue) =
    (header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

/// The routes, over the pairing state.
pub(super) fn routes(pairing: DevicePairing) -> Router {
    Router::new()
        .route("/api/hub/pairing/state", get(pairing_state))
        .route("/api/hub/pairing/requests", post(create_request))
        .route("/api/hub/pairing/requests/poll", post(poll_request))
        .route("/api/hub/pairing/redeem", post(redeem_token))
        .route("/api/hub/pairing/recovery", post(redeem_recovery))
        .route("/api/hub/pairing/handoff", post(redeem_handoff))
        .route("/api/hub/devices", get(list_devices))
        .route("/api/hub/devices/{id}", delete(revoke_device))
        .route(
            "/api/hub/devices/pending/{id}/approve",
            post(approve_pending),
        )
        .route("/api/hub/devices/pending/{id}/deny", post(deny_pending))
        .route(
            "/api/hub/devices/recovery-codes",
            post(regenerate_recovery_codes),
        )
        .route(
            "/api/hub/devices/workbench-handoff",
            post(workbench_handoff),
        )
        .route("/api/hub/remote-access/pair-link", post(pair_link))
        .with_state(pairing)
}

fn surface_of(parts: &Parts) -> Surface {
    if parts.extensions.get::<ArtifactsOrigin>().is_some() {
        Surface::Workbench
    } else {
        Surface::Ui
    }
}

fn remote_of(parts: &Parts) -> Option<RemoteContext> {
    remote_context(&parts.extensions)
}

fn cookie_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
}

/// The response for a pairing operation that didn't happen.
fn error_response(error: &PairingError) -> Response {
    let message = error.to_string();
    match error {
        PairingError::Rejected(_) | PairingError::Invalid(_) => {
            json_error(StatusCode::BAD_REQUEST, message)
        }
        PairingError::NotReady(_) => json_error(StatusCode::CONFLICT, message),
        PairingError::RateLimited(_) => {
            let mut response = json_error(StatusCode::TOO_MANY_REQUESTS, message);
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
            response
        }
        PairingError::Storage(_) | PairingError::Random => {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    }
}

/// `body` with the device cookie for `issued` set and caching off.
fn with_credential(pairing: &DevicePairing, issued: &Issued, body: Response) -> Response {
    let mut response = body;
    if let Ok(value) = HeaderValue::from_str(&pairing.set_cookie_for(&issued.secret)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response.headers_mut().insert(NO_STORE.0, NO_STORE.1);
    response
}

fn paired(pairing: &DevicePairing, issued: &Issued) -> Response {
    let body = Json(PairedResponse {
        device_name: issued.device_name.clone(),
    })
    .into_response();
    with_credential(pairing, issued, body)
}

fn no_store(json: impl IntoResponse) -> Response {
    let mut response = json.into_response();
    response.headers_mut().insert(NO_STORE.0, NO_STORE.1);
    response
}

// ── Pre-auth: the pairing page's API ─────────────────────────────────

/// `GET /api/hub/pairing/state`: whether this browser needs to pair.
async fn pairing_state(State(pairing): State<DevicePairing>, parts: Parts) -> Response {
    let remote = remote_of(&parts).is_some();
    let paired = !remote
        || pairing
            .authenticate(surface_of(&parts), cookie_header(&parts.headers))
            .await
            .is_some();
    no_store(Json(PairingStateResponse { remote, paired }))
}

/// `POST /api/hub/pairing/requests`: ask to be paired and get a code to show.
async fn create_request(
    State(pairing): State<DevicePairing>,
    parts: Parts,
    Json(body): Json<CreatePairingRequestBody>,
) -> Response {
    match pairing.create_request(remote_of(&parts).as_ref(), &body.device_name) {
        Ok(created) => no_store(Json(PairingRequestCreated {
            request_id: created.request_id,
            code: created.code,
            expires_in_secs: created.expires_in_secs,
        })),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/pairing/requests/poll`: has the request been answered?
async fn poll_request(
    State(pairing): State<DevicePairing>,
    Json(body): Json<PollPairingBody>,
) -> Response {
    let outcome = match pairing.poll_request(&body.request_id).await {
        Ok(outcome) => outcome,
        Err(e) => return error_response(&e),
    };
    let (status, issued) = match outcome {
        PollOutcome::Pending => (PairingRequestStatus::Pending, None),
        PollOutcome::Denied => (PairingRequestStatus::Denied, None),
        PollOutcome::Expired => (PairingRequestStatus::Expired, None),
        PollOutcome::Approved(issued) => (PairingRequestStatus::Approved, Some(issued)),
    };
    let answer = no_store(Json(PairingPollResponse { status }));
    match issued {
        Some(issued) => with_credential(&pairing, &issued, answer),
        None => answer,
    }
}

/// `POST /api/hub/pairing/redeem`: pair with a link's single-use token.
async fn redeem_token(
    State(pairing): State<DevicePairing>,
    parts: Parts,
    Json(body): Json<RedeemTokenBody>,
) -> Response {
    match pairing
        .redeem_token(remote_of(&parts).as_ref(), &body.token, &body.device_name)
        .await
    {
        Ok(issued) => paired(&pairing, &issued),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/pairing/recovery`: pair with a recovery code.
async fn redeem_recovery(
    State(pairing): State<DevicePairing>,
    parts: Parts,
    Json(body): Json<RecoveryCodeBody>,
) -> Response {
    match pairing
        .redeem_recovery(remote_of(&parts).as_ref(), &body.code, &body.device_name)
        .await
    {
        Ok(issued) => paired(&pairing, &issued),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/pairing/handoff`, on the workbench host: trade a token a
/// paired UI-host browser minted for this host's own credential.
async fn redeem_handoff(
    State(pairing): State<DevicePairing>,
    parts: Parts,
    Json(body): Json<HandoffBody>,
) -> Response {
    if surface_of(&parts) != Surface::Workbench {
        return json_error(
            StatusCode::NOT_FOUND,
            "Workbench handoffs are completed on the workbench address.",
        );
    }
    let held = pairing.cookie_in(cookie_header(&parts.headers));
    match pairing
        .redeem_handoff(remote_of(&parts).as_ref(), &body.token, held.as_deref())
        .await
    {
        Ok(issued) => paired(&pairing, &issued),
        Err(e) => error_response(&e),
    }
}

// ── Management: paired devices and the local UI ──────────────────────

/// `GET /api/hub/devices`: paired devices and pending requests.
async fn list_devices(State(pairing): State<DevicePairing>, parts: Parts) -> Response {
    let current = parts
        .extensions
        .get::<AuthenticatedDevice>()
        .map(|d| d.id.clone());
    no_store(Json(DeviceListResponse {
        devices: pairing.devices(current.as_deref()),
        pending: pairing.pending(),
        recovery_codes_remaining: u32::try_from(pairing.recovery_codes_remaining()).unwrap_or(0),
        ui_origin: pairing.identity().ui_origin,
        remote: remote_of(&parts).is_some(),
    }))
}

/// `DELETE /api/hub/devices/{id}`: stop trusting a device.
async fn revoke_device(State(pairing): State<DevicePairing>, Path(id): Path<String>) -> Response {
    match pairing.revoke(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(PairingError::Rejected(message)) => json_error(StatusCode::NOT_FOUND, message),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/devices/pending/{id}/approve`.
async fn approve_pending(State(pairing): State<DevicePairing>, Path(id): Path<String>) -> Response {
    answer_pending(&pairing, &id, true)
}

/// `POST /api/hub/devices/pending/{id}/deny`.
async fn deny_pending(State(pairing): State<DevicePairing>, Path(id): Path<String>) -> Response {
    answer_pending(&pairing, &id, false)
}

fn answer_pending(pairing: &DevicePairing, id: &str, approve: bool) -> Response {
    match pairing.decide(id, approve) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(PairingError::Rejected(message)) => json_error(StatusCode::NOT_FOUND, message),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/devices/recovery-codes`: replace the recovery codes.
async fn regenerate_recovery_codes(State(pairing): State<DevicePairing>) -> Response {
    match pairing.regenerate_recovery_codes().await {
        Ok(recovery_codes) => no_store(Json(RecoveryCodesResponse { recovery_codes })),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/devices/workbench-handoff`: mint the token a paired
/// UI-host browser carries to the workbench host.
async fn workbench_handoff(State(pairing): State<DevicePairing>, parts: Parts) -> Response {
    let device = parts
        .extensions
        .get::<AuthenticatedDevice>()
        .filter(|_| surface_of(&parts) == Surface::Ui);
    let Some(device) = device else {
        return json_error(
            StatusCode::CONFLICT,
            "This browser isn't a paired device. Opened on the machine Residuum runs on, the workbench needs no handoff.",
        );
    };
    match pairing.mint_handoff(&device.id) {
        Ok((token, expires_in_secs)) => no_store(Json(WorkbenchHandoffResponse {
            token,
            expires_in_secs,
        })),
        Err(e) => error_response(&e),
    }
}

/// `POST /api/hub/remote-access/pair-link`: mint the first device's link.
/// Refused for a remote request: the first device is paired from the machine
/// Residuum runs on, never over the relay.
async fn pair_link(State(pairing): State<DevicePairing>, parts: Parts) -> Response {
    if remote_of(&parts).is_some() {
        return json_error(
            StatusCode::FORBIDDEN,
            "A pairing link can only be made on the machine running Residuum. From a paired device, approve the other device's request instead.",
        );
    }
    let minted = match pairing.mint_pair_token().await {
        Ok(minted) => minted,
        Err(e) => return error_response(&e),
    };
    let link = format!(
        "{}{PAIRING_PAGE_PATH}#token={}",
        minted.ui_origin, minted.token
    );
    let qr_svg = match qr::svg(&link) {
        Ok(svg) => svg,
        Err(message) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    no_store(Json(PairLinkResponse {
        link,
        qr_svg,
        expires_in_secs: minted.expires_in_secs,
        recovery_codes: minted.new_recovery_codes,
    }))
}

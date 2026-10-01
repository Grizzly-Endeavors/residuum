//! `/api/hub/push/...`: the hub's Web Push key and the devices that receive
//! notifications.
//!
//! The routes register browsers for push and change or remove them, and send
//! a test notification. The service behind them is [`crate::hub::push`].
//! Failures are `{ "error": message }`: `400` for a request that can't be
//! used, `404` for an unknown device, and `500` when the key or devices file
//! can't be read or written.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, patch, post};

use super::error::json_error;
use super::lifecycle::parse_body;
use crate::hub::AgentDirectory;
use crate::hub::inbox;
use crate::hub::push::{
    PatchPushDeviceRequest, PushDeviceList, PushDeviceResponse, PushError, PushKeyResponse,
    PushService, PutPushDeviceRequest, validate_subscription,
};

/// What the push routes read.
#[derive(Clone)]
pub(super) struct PushApiState {
    pub push: Arc<PushService>,
    /// Read for the unread count a test notification puts on the app badge.
    pub directory: Arc<dyn AgentDirectory>,
}

/// The key, device list, registration, change, removal, and test routes.
pub(super) fn routes(state: PushApiState) -> Router {
    Router::new()
        .route("/api/hub/push/key", get(public_key))
        .route("/api/hub/push/devices", get(list_devices).put(put_device))
        .route(
            "/api/hub/push/devices/{id}",
            patch(patch_device).delete(delete_device),
        )
        .route("/api/hub/push/devices/{id}/test", post(test_device))
        .with_state(state)
}

fn push_error_response(error: &PushError) -> Response {
    let status = match error {
        PushError::BadRequest(_) => StatusCode::BAD_REQUEST,
        PushError::UnknownDevice(_) => StatusCode::NOT_FOUND,
        PushError::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json_error(status, error.to_string())
}

/// `GET /api/hub/push/key` — the public key a browser subscribes with.
async fn public_key(State(state): State<PushApiState>) -> Response {
    match state.push.public_key().await {
        Ok(public_key) => Json(PushKeyResponse { public_key }).into_response(),
        Err(e) => push_error_response(&e),
    }
}

/// `GET /api/hub/push/devices`.
async fn list_devices(State(state): State<PushApiState>) -> Response {
    match state.push.devices().await {
        Ok(devices) => Json(PushDeviceList { devices }).into_response(),
        Err(e) => push_error_response(&e),
    }
}

/// `PUT /api/hub/push/devices` — register a browser, or update the device
/// already registered for the same subscription.
async fn put_device(State(state): State<PushApiState>, body: Bytes) -> Response {
    let request: PutPushDeviceRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    if let Err(e) = validate_subscription(&request.subscription) {
        return push_error_response(&e);
    }
    match state.push.upsert_device(request).await {
        Ok(device) => Json(PushDeviceResponse { device }).into_response(),
        Err(e) => push_error_response(&e),
    }
}

/// `PATCH /api/hub/push/devices/{id}` — change a label and/or preferences.
async fn patch_device(
    State(state): State<PushApiState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let request: PatchPushDeviceRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    match state.push.patch_device(&id, request).await {
        Ok(device) => Json(PushDeviceResponse { device }).into_response(),
        Err(e) => push_error_response(&e),
    }
}

/// `DELETE /api/hub/push/devices/{id}`.
async fn delete_device(State(state): State<PushApiState>, Path(id): Path<String>) -> Response {
    match state.push.delete_device(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => push_error_response(&e),
    }
}

/// `POST /api/hub/push/devices/{id}/test` — send a test notification and
/// answer once the push service has accepted or refused it.
async fn test_device(State(state): State<PushApiState>, Path(id): Path<String>) -> Response {
    let badge = match inbox::unread(state.directory.as_ref()).await {
        Ok(unread) => unread.total,
        Err(e) => {
            // The count only sets the app badge, so a test still goes out.
            tracing::warn!(error = %e, "couldn't count unread inbox items for a test notification's badge");
            0
        }
    };
    match state.push.send_test(&id, badge).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => push_error_response(&e),
    }
}

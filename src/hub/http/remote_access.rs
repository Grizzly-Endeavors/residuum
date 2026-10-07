//! Remote access status and the actions on it: the recovery code, a retry,
//! and a pin reset.
//!
//! The recovery code and the reset are for the machine Residuum runs on: a
//! request that arrived through Residuum Cloud can read the status and ask for
//! a retry, but is refused the rest. Pages on the artifacts origin are refused
//! every route here (see `artifacts_origin`).

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::error::json_error;
use crate::pairing::remote::remote_context;
use crate::remote_access::manager::ResetError;
use crate::remote_access::slot::RemoteAccessSlot;

const LOCAL_ONLY: &str = "This can only be done on the machine running Residuum, because it handles the recovery code that protects your address.";

pub(super) fn routes(slot: RemoteAccessSlot) -> Router {
    Router::new()
        .route("/api/hub/remote-access/status", get(status))
        .route("/api/hub/remote-access/retry", post(retry))
        .route(
            "/api/hub/remote-access/recovery-code/saved",
            post(recovery_code_saved),
        )
        .route("/api/hub/remote-access/reset-pins", post(reset_pins))
        .with_state(slot)
}

fn is_remote(parts: &Parts) -> bool {
    remote_context(&parts.headers, &parts.extensions).is_some()
}

fn no_store(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

/// `GET /api/hub/remote-access/status`.
async fn status(State(slot): State<RemoteAccessSlot>, parts: Parts) -> Response {
    no_store(Json(slot.status(!is_remote(&parts))))
}

/// `POST /api/hub/remote-access/retry`: look again now.
async fn retry(State(slot): State<RemoteAccessSlot>) -> Response {
    slot.retry_now();
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /api/hub/remote-access/recovery-code/saved`: the person has saved
/// the recovery code; Residuum forgets it.
async fn recovery_code_saved(State(slot): State<RemoteAccessSlot>, parts: Parts) -> Response {
    if is_remote(&parts) {
        return json_error(StatusCode::FORBIDDEN, LOCAL_ONLY);
    }
    match slot.acknowledge_recovery_code().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "couldn't forget the saved recovery code");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Residuum couldn't update its record of the recovery code: {e:#}"),
            )
        }
    }
}

#[derive(Deserialize)]
struct ResetBody {
    recovery_code: String,
}

/// `POST /api/hub/remote-access/reset-pins`: take the address back with the
/// recovery code. The new recovery code is in the status afterwards.
async fn reset_pins(
    State(slot): State<RemoteAccessSlot>,
    parts: Parts,
    Json(body): Json<ResetBody>,
) -> Response {
    if is_remote(&parts) {
        return json_error(StatusCode::FORBIDDEN, LOCAL_ONLY);
    }
    match slot.reset_pins(&body.recovery_code).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e @ ResetError::Invalid(_)) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e @ (ResetError::NotConnected | ResetError::Failed(_))) => {
            json_error(StatusCode::CONFLICT, e.to_string())
        }
    }
}

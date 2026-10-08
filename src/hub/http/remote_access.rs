//! Remote access status and the actions on it: the recovery code, a retry,
//! a pin reset, joining and approving sibling instances, removing the pin of
//! an instance that no longer exists, and switching the active instance.
//!
//! The recovery code and the reset are for the machine Residuum runs on: a
//! request that arrived through Residuum Cloud can read the status and act on
//! siblings, pins and the switcher, but is refused those two. Pages on the artifacts origin are refused
//! every route here (see `artifacts_origin`).

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::error::json_error;
use crate::pairing::remote::remote_context;
use crate::remote_access::manager::{ActionError, ResetError};
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
        .route("/api/hub/remote-access/join", post(start_join))
        .route(
            "/api/hub/remote-access/joins/{id}/approve",
            post(approve_join),
        )
        .route("/api/hub/remote-access/joins/{id}/deny", post(deny_join))
        .route("/api/hub/remote-access/pins/remove", post(remove_pin))
        .route(
            "/api/hub/remote-access/instances/{slug}/activate",
            post(activate_instance),
        )
        .with_state(slot)
}

fn is_remote(parts: &Parts) -> bool {
    remote_context(&parts.extensions).is_some()
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

fn action_result(result: Result<(), ActionError>) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e @ ActionError::Invalid(_)) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e @ (ActionError::NotConnected | ActionError::Failed(_))) => {
            json_error(StatusCode::CONFLICT, e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct JoinBody {
    instance: String,
}

/// `POST /api/hub/remote-access/join`: ask the instance named in the body to
/// approve this one. Progress, including the code to compare, is in the status.
async fn start_join(State(slot): State<RemoteAccessSlot>, Json(body): Json<JoinBody>) -> Response {
    action_result(slot.start_join(&body.instance).await)
}

/// `POST /api/hub/remote-access/joins/{id}/approve`.
async fn approve_join(State(slot): State<RemoteAccessSlot>, Path(id): Path<String>) -> Response {
    action_result(slot.approve_join(&id).await)
}

/// `POST /api/hub/remote-access/joins/{id}/deny`.
async fn deny_join(State(slot): State<RemoteAccessSlot>, Path(id): Path<String>) -> Response {
    action_result(slot.deny_join(&id))
}

#[derive(Deserialize)]
struct RemovePinBody {
    account_uri: String,
}

/// `POST /api/hub/remote-access/pins/remove`: stop allowing the certificate
/// account of an instance that no longer exists.
async fn remove_pin(
    State(slot): State<RemoteAccessSlot>,
    Json(body): Json<RemovePinBody>,
) -> Response {
    action_result(slot.remove_pin(&body.account_uri).await)
}

/// `POST /api/hub/remote-access/instances/{slug}/activate`: send the user's
/// address to that instance.
async fn activate_instance(
    State(slot): State<RemoteAccessSlot>,
    Path(slug): Path<String>,
) -> Response {
    action_result(slot.activate_instance(&slug))
}

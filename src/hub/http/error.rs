//! The contract's JSON error bodies.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use crate::hub::LifecycleError;

/// A `{ "error": message }` response with `status`.
pub(super) fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

/// The response for a failed lifecycle or lookup call.
///
/// `NotFound` and `NoDeletedAgent` are `404`, `InvalidName` and `InvalidRequest` are `400`,
/// `AlreadyExists` is `409`, `NotRunning` is `409` with the agent's `state`
/// beside the message, and `Failed` is `500`.
pub(super) fn lifecycle_error_response(error: &LifecycleError) -> Response {
    let message = error.to_string();
    match error {
        LifecycleError::NotFound(_) | LifecycleError::NoDeletedAgent(_) => {
            json_error(StatusCode::NOT_FOUND, message)
        }
        LifecycleError::InvalidName(_) | LifecycleError::InvalidRequest(_) => {
            json_error(StatusCode::BAD_REQUEST, message)
        }
        LifecycleError::AlreadyExists(_) => json_error(StatusCode::CONFLICT, message),
        LifecycleError::NotRunning { state, .. } => (
            StatusCode::CONFLICT,
            Json(json!({ "error": message, "state": state })),
        )
            .into_response(),
        LifecycleError::Failed(_) => {
            tracing::error!(error = %message, "hub request failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    }
}

//! `POST /api/hub/uploads`: stages one file the client will send later.
//!
//! The body is the file's bytes and `filename` is a query parameter. The
//! store does the work and its rules (see [`crate::uploads`]); this module
//! maps its results to responses. Failures are `{ "error": message }`: `400`
//! for a missing or unusable filename or an empty body, `413` for a body
//! over the limit, and `500` when the store can't write.

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use serde::Deserialize;
use serde_json::json;

use super::error::json_error;
use crate::uploads::{MAX_UPLOAD_BYTES, StagedUpload, UploadError, UploadStore};

/// The upload route, served by the hub whatever the agents' state.
pub(super) fn routes(store: UploadStore) -> Router {
    Router::new()
        .route("/api/hub/uploads", post(create_upload))
        .route_layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(store)
}

/// The query of `POST /api/hub/uploads`.
#[derive(Deserialize)]
struct UploadParams {
    filename: Option<String>,
}

async fn create_upload(
    State(store): State<UploadStore>,
    query: Result<Query<UploadParams>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Ok(Query(params)) = query else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "The upload's filename query parameter could not be read.",
        );
    };
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(rejection) => return body_rejection_response(rejection),
    };
    let Some(filename) = params.filename else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Name the upload with a filename query parameter, such as ?filename=notes.txt.",
        );
    };
    match store.stage(&filename, &bytes).await {
        Ok(staged) => staged_response(&staged),
        Err(err) => upload_error_response(&err),
    }
}

fn staged_response(staged: &StagedUpload) -> Response {
    Json(json!({
        "id": staged.id,
        "filename": staged.filename,
        "size": staged.size,
    }))
    .into_response()
}

/// The response for a body the server could not read: over the limit is
/// `413`, anything else is a request the server could not read.
fn body_rejection_response(rejection: BytesRejection) -> Response {
    let response = rejection.into_response();
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return upload_too_large_response();
    }
    json_error(
        StatusCode::BAD_REQUEST,
        "The upload could not be read. Send the file's bytes as the request body.",
    )
}

fn upload_too_large_response() -> Response {
    json_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "The file is larger than the {} MB upload limit.",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ),
    )
}

fn upload_error_response(err: &UploadError) -> Response {
    match err {
        UploadError::Empty => json_error(StatusCode::BAD_REQUEST, "The upload is empty."),
        UploadError::TooLarge { .. } => upload_too_large_response(),
        UploadError::InvalidFilename { reason } => {
            json_error(StatusCode::BAD_REQUEST, format!("The filename {reason}."))
        }
        UploadError::InvalidId(_) | UploadError::NotFound(_) => {
            json_error(StatusCode::BAD_REQUEST, "That upload is not staged.")
        }
        UploadError::Io { .. } => {
            tracing::error!(error = %err, "failed to stage hub upload");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The upload could not be saved. Try again.",
            )
        }
    }
}

//! `/api/hub/rooms/*`: HTTP handlers for multi-agent meeting rooms (`#860`).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::error::json_error;
use crate::file_server::mark_agent_file;
use crate::uploads::{StagedUpload, UploadError, UploadStore};
use residuum_bus::attachment::detect_mime_type;
use residuum_rooms::{
    CreateRoomRequest, MarkRoomReadRequest, PatchRoomRequest, PostRoomMessageRequest,
    RoomAttachment, RoomService, RoomServiceError, RoomStatus, RoomStoreError, validate_room_id,
};

/// Shared state for `/api/hub/rooms/*` routes.
#[derive(Clone)]
pub(super) struct RoomsHttpState {
    pub rooms: Arc<RoomService>,
    pub uploads: UploadStore,
}

/// Mount all `/api/hub/rooms*` routes onto the hub router.
pub(super) fn routes(state: RoomsHttpState) -> Router {
    Router::new()
        .route("/api/hub/rooms", get(list_rooms).post(create_room))
        .route("/api/hub/rooms/candidates", get(list_candidates))
        .route(
            "/api/hub/rooms/{id}",
            get(get_room).patch(patch_room).delete(delete_room),
        )
        .route("/api/hub/rooms/{id}/read", post(mark_read))
        .route("/api/hub/rooms/{id}/messages", post(post_message))
        .route("/api/hub/rooms/{id}/continue", post(continue_window))
        .route("/api/hub/rooms/{id}/stop", post(stop_room))
        .route("/api/hub/rooms/{id}/archive", post(archive_room))
        .route("/api/hub/rooms/{id}/reopen", post(reopen_room))
        .route(
            "/api/hub/rooms/{id}/attachments/{att_id}",
            get(get_attachment),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct GetRoomQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before_seq: Option<u64>,
}

struct HttpRefusal(StatusCode, String);

impl IntoResponse for HttpRefusal {
    fn into_response(self) -> Response {
        json_error(self.0, self.1)
    }
}

fn parse_json_body<T>(res: Result<Json<T>, JsonRejection>) -> Result<T, HttpRefusal> {
    res.map(|Json(body)| body).map_err(|rejection| {
        HttpRefusal(
            StatusCode::BAD_REQUEST,
            format!("invalid request body: {}", rejection.body_text()),
        )
    })
}

fn room_error_response(err: &RoomServiceError) -> Response {
    let message = err.to_string();
    match err {
        RoomServiceError::Validation(_)
        | RoomServiceError::BadRequest(_)
        | RoomServiceError::Store(RoomStoreError::InvalidRoomId(_)) => {
            json_error(StatusCode::BAD_REQUEST, message)
        }
        RoomServiceError::NotFound(_) | RoomServiceError::Store(RoomStoreError::NotFound(_)) => {
            json_error(StatusCode::NOT_FOUND, message)
        }
        RoomServiceError::Conflict(_) => json_error(StatusCode::CONFLICT, message),
        RoomServiceError::AccessDenied(_) => json_error(StatusCode::FORBIDDEN, message),
        RoomServiceError::Store(_) | RoomServiceError::Internal(_) => {
            tracing::error!(error = %message, "meeting room request failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    }
}

/// `GET /api/hub/rooms`
async fn list_rooms(State(state): State<RoomsHttpState>) -> Response {
    Json(state.rooms.list_rooms().await).into_response()
}

/// `GET /api/hub/rooms/candidates`
async fn list_candidates(State(state): State<RoomsHttpState>) -> Response {
    Json(state.rooms.list_candidates().await).into_response()
}

/// `POST /api/hub/rooms`
async fn create_room(
    State(state): State<RoomsHttpState>,
    body: Result<Json<CreateRoomRequest>, JsonRejection>,
) -> Response {
    let req = match parse_json_body(body) {
        Ok(req) => req,
        Err(refusal) => return refusal.into_response(),
    };
    match state.rooms.create_room(req).await {
        Ok(detail) => (StatusCode::CREATED, Json(detail)).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `GET /api/hub/rooms/{id}`
async fn get_room(
    State(state): State<RoomsHttpState>,
    Path(id): Path<String>,
    query: Result<Query<GetRoomQuery>, QueryRejection>,
) -> Response {
    let Query(params) = match query {
        Ok(q) => q,
        Err(rejection) => return json_error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    let limit = match params.limit {
        Some(0) => return json_error(StatusCode::BAD_REQUEST, "limit must be at least 1"),
        Some(n) => n.min(200),
        None => 200,
    };
    match state.rooms.get_room(&id, limit, params.before_seq).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `PATCH /api/hub/rooms/{id}`
async fn patch_room(
    State(state): State<RoomsHttpState>,
    Path(id): Path<String>,
    body: Result<Json<PatchRoomRequest>, JsonRejection>,
) -> Response {
    let req = match parse_json_body(body) {
        Ok(req) => req,
        Err(refusal) => return refusal.into_response(),
    };
    match state.rooms.patch_room(&id, req).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `POST /api/hub/rooms/{id}/read`
async fn mark_read(
    State(state): State<RoomsHttpState>,
    Path(id): Path<String>,
    body: Result<Json<MarkRoomReadRequest>, JsonRejection>,
) -> Response {
    let req = match parse_json_body(body) {
        Ok(req) => req,
        Err(refusal) => return refusal.into_response(),
    };
    match state.rooms.mark_read(&id, req.seq).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => room_error_response(&err),
    }
}

async fn resolve_staged_uploads(
    uploads: &UploadStore,
    ids: &[String],
) -> Result<Vec<StagedUpload>, HttpRefusal> {
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        match uploads.resolve(id).await {
            Ok(staged) => resolved.push(staged),
            Err(UploadError::InvalidId(_) | UploadError::NotFound(_)) => {
                return Err(HttpRefusal(
                    StatusCode::BAD_REQUEST,
                    format!("upload '{id}' is not available; attach the file again"),
                ));
            }
            Err(err) => {
                tracing::error!(upload = %id, error = %err, "failed to read staged upload for room message");
                return Err(HttpRefusal(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to read an attached file".to_owned(),
                ));
            }
        }
    }
    Ok(resolved)
}

async fn copy_room_attachments(
    state: &RoomsHttpState,
    room_id: &str,
    staged: &[StagedUpload],
) -> Result<(Vec<RoomAttachment>, Vec<PathBuf>), HttpRefusal> {
    let base_dir = state
        .rooms
        .store()
        .rooms_root()
        .join(room_id)
        .join("attachments");
    let mut attachments = Vec::with_capacity(staged.len());
    let mut created_dirs = Vec::with_capacity(staged.len());

    for upload in staged {
        let dest_dir = base_dir.join(&upload.id);
        let dest_file = dest_dir.join(&upload.filename);
        if let Err(err) = tokio::fs::create_dir_all(&dest_dir).await {
            cleanup_dirs(&created_dirs).await;
            tracing::error!(path = %dest_dir.display(), error = %err, "failed to create room attachment directory");
            return Err(HttpRefusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save attached file".to_owned(),
            ));
        }
        created_dirs.push(dest_dir.clone());
        if let Err(err) = tokio::fs::copy(&upload.path, &dest_file).await {
            cleanup_dirs(&created_dirs).await;
            tracing::error!(src = %upload.path.display(), dst = %dest_file.display(), error = %err, "failed to copy staged upload into room");
            return Err(HttpRefusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save attached file".to_owned(),
            ));
        }
        attachments.push(RoomAttachment {
            id: upload.id.clone(),
            filename: upload.filename.clone(),
            mime_type: upload.content_type.clone(),
            size: upload.size,
            url: format!("/api/hub/rooms/{room_id}/attachments/{}", upload.id),
        });
    }

    Ok((attachments, created_dirs))
}

async fn cleanup_dirs(dirs: &[PathBuf]) {
    for dir in dirs {
        tokio::fs::remove_dir_all(dir).await.ok();
    }
}

async fn consume_uploads(uploads: &UploadStore, staged: &[StagedUpload]) {
    for upload in staged {
        if let Err(err) = uploads.remove(&upload.id).await {
            tracing::warn!(
                upload = %upload.id,
                error = %err,
                "failed to remove consumed staged upload after room message"
            );
        }
    }
}

/// `POST /api/hub/rooms/{id}/messages`
async fn post_message(
    State(state): State<RoomsHttpState>,
    Path(id): Path<String>,
    body: Result<Json<PostRoomMessageRequest>, JsonRejection>,
) -> Response {
    if let Err(err) = validate_room_id(&id) {
        return room_error_response(&err.into());
    }
    let req = match parse_json_body(body) {
        Ok(req) => req,
        Err(refusal) => return refusal.into_response(),
    };
    if req.content.trim().is_empty() && req.upload_ids.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "message content or attachments required",
        );
    }

    match state.rooms.store().get_record(&id).await {
        Ok(rec) if rec.status == RoomStatus::Archived => {
            return json_error(
                StatusCode::CONFLICT,
                "cannot post to an archived room; reopen it first",
            );
        }
        Ok(_) => {}
        Err(err) => return room_error_response(&err.into()),
    }

    let staged = match resolve_staged_uploads(&state.uploads, &req.upload_ids).await {
        Ok(s) => s,
        Err(refusal) => return refusal.into_response(),
    };
    let (attachments, created_dirs) = match copy_room_attachments(&state, &id, &staged).await {
        Ok(res) => res,
        Err(refusal) => return refusal.into_response(),
    };

    match state
        .rooms
        .post_owner_message(&id, req.content, attachments)
        .await
    {
        Ok(msg) => {
            consume_uploads(&state.uploads, &staged).await;
            (StatusCode::ACCEPTED, Json(msg)).into_response()
        }
        Err(err) => {
            cleanup_dirs(&created_dirs).await;
            room_error_response(&err)
        }
    }
}

/// `POST /api/hub/rooms/{id}/continue`
async fn continue_window(State(state): State<RoomsHttpState>, Path(id): Path<String>) -> Response {
    match state.rooms.continue_window(&id).await {
        Ok(detail) => (StatusCode::ACCEPTED, Json(detail)).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `POST /api/hub/rooms/{id}/stop`
async fn stop_room(State(state): State<RoomsHttpState>, Path(id): Path<String>) -> Response {
    match state.rooms.stop_room(&id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `POST /api/hub/rooms/{id}/archive`
async fn archive_room(State(state): State<RoomsHttpState>, Path(id): Path<String>) -> Response {
    match state.rooms.archive_room(&id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `POST /api/hub/rooms/{id}/reopen`
async fn reopen_room(State(state): State<RoomsHttpState>, Path(id): Path<String>) -> Response {
    match state.rooms.reopen_room(&id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(err) => room_error_response(&err),
    }
}

/// `DELETE /api/hub/rooms/{id}`
async fn delete_room(State(state): State<RoomsHttpState>, Path(id): Path<String>) -> Response {
    match state.rooms.delete_room(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => room_error_response(&err),
    }
}

fn is_valid_attachment_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `GET /api/hub/rooms/{id}/attachments/{att_id}`
async fn get_attachment(
    State(state): State<RoomsHttpState>,
    Path((id, att_id)): Path<(String, String)>,
) -> Response {
    if let Err(err) = validate_room_id(&id) {
        return room_error_response(&err.into());
    }
    if !is_valid_attachment_id(&att_id) {
        return json_error(StatusCode::BAD_REQUEST, "invalid attachment id");
    }
    if let Err(err) = state.rooms.store().get_record(&id).await {
        return room_error_response(&err.into());
    }
    let dir = state
        .rooms
        .store()
        .rooms_root()
        .join(&id)
        .join("attachments")
        .join(&att_id);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return json_error(StatusCode::NOT_FOUND, "attachment not found");
        }
        Err(err) => {
            tracing::error!(path = %dir.display(), error = %err, "failed to read room attachment directory");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read attachment",
            );
        }
    };
    let entry = match entries.next_entry().await {
        Ok(Some(entry)) => entry,
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "attachment not found"),
        Err(err) => {
            tracing::error!(path = %dir.display(), error = %err, "failed to read room attachment entry");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read attachment",
            );
        }
    };
    let file_path = entry.path();
    let filename = entry.file_name().to_string_lossy().into_owned();
    let bytes = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return json_error(StatusCode::NOT_FOUND, "attachment not found");
        }
        Err(err) => {
            tracing::error!(path = %file_path.display(), error = %err, "failed to read room attachment bytes");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read attachment",
            );
        }
    };
    let mime = detect_mime_type(&file_path);
    let mut headers = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, val);
    }
    let sanitized_name = filename.replace('"', "_");
    if let Ok(disp) = HeaderValue::from_str(&format!("inline; filename=\"{sanitized_name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, disp);
    }
    mark_agent_file(&mut headers, &mime);
    (StatusCode::OK, headers, Body::from(bytes)).into_response()
}

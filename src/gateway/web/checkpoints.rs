//! Checkpoint HTTP API: visibility (list/show/diff/file/stats) and
//! restore/undo, for the web UI. See `docs/systems-usage/checkpoints.md`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use crate::checkpoints::{
    CheckpointContext, CheckpointEngine, CheckpointError, CheckpointTrigger, RepoKind,
};

/// Shared state for the checkpoints API.
#[derive(Clone)]
pub(crate) struct CheckpointApiState {
    pub checkpoints: Arc<CheckpointEngine>,
}

/// Build the checkpoints API router.
pub(crate) fn checkpoints_api_router(state: CheckpointApiState) -> Router {
    use axum::routing::{get, post};

    Router::new()
        .route("/api/checkpoints", get(api_checkpoints_list))
        .route("/api/checkpoints/stats", get(api_checkpoints_stats))
        .route("/api/checkpoints/{id}", get(api_checkpoints_show))
        .route("/api/checkpoints/{id}/diff", get(api_checkpoints_diff))
        .route("/api/checkpoints/{id}/file", get(api_checkpoints_file))
        .route(
            "/api/checkpoints/{id}/restore",
            post(api_checkpoints_restore),
        )
        .route("/api/checkpoints/{id}/undo", post(api_checkpoints_undo))
        .with_state(state)
}

/// `repo` query parameter shared by every route: which checkpoint
/// repository the request targets.
#[derive(Deserialize)]
pub(super) struct RepoQuery {
    pub repo: RepoKind,
}

fn error_response(e: &CheckpointError) -> (StatusCode, String) {
    let status = match e {
        CheckpointError::NotFound(_)
        | CheckpointError::PathNotFound(_, _)
        | CheckpointError::InvalidCursor => StatusCode::NOT_FOUND,
        CheckpointError::Git(_) | CheckpointError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string())
}

/// Query parameters for `GET /api/checkpoints`.
#[derive(Deserialize)]
pub(super) struct ListQuery {
    pub repo: RepoKind,
    /// Restrict to checkpoints that changed this path (a file, or a
    /// directory prefix).
    pub path: Option<String>,
    /// Opaque cursor from a previous response's `next_cursor`.
    pub before: Option<String>,
    /// Page size, 1-200 (default 50).
    pub limit: Option<usize>,
}

/// `GET /api/checkpoints?repo=workspace|config&path=&before=&limit=` — one
/// page of checkpoints, newest first.
async fn api_checkpoints_list(
    State(state): State<CheckpointApiState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<crate::checkpoints::CheckpointPage>, (StatusCode, String)> {
    let limit = query.limit.map(|n| n.clamp(1, 200));
    state
        .checkpoints
        .list_checkpoints(query.repo, query.path, query.before, limit)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

/// `GET /api/checkpoints/stats?repo=workspace|config` — on-disk size,
/// checkpoint count, and oldest checkpoint.
async fn api_checkpoints_stats(
    State(state): State<CheckpointApiState>,
    Query(query): Query<RepoQuery>,
) -> Result<Json<crate::checkpoints::RepoStats>, (StatusCode, String)> {
    state
        .checkpoints
        .stats(query.repo)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

/// `GET /api/checkpoints/{id}?repo=workspace|config` — a checkpoint's
/// metadata plus the paths it changed.
async fn api_checkpoints_show(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoQuery>,
) -> Result<Json<crate::checkpoints::CheckpointDetail>, (StatusCode, String)> {
    state
        .checkpoints
        .show_checkpoint(query.repo, id)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

/// Query parameters shared by the diff and file-content routes.
#[derive(Deserialize)]
pub(super) struct RepoPathQuery {
    pub repo: RepoKind,
    pub path: String,
}

/// Response for `GET /api/checkpoints/{id}/diff`.
#[derive(Serialize)]
struct DiffResponse {
    /// Unified diff text, or `null` if `path` didn't change at this
    /// checkpoint.
    diff: Option<String>,
}

/// `GET /api/checkpoints/{id}/diff?repo=&path=` — unified diff for one
/// file at one checkpoint, relative to the checkpoint before it.
async fn api_checkpoints_diff(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoPathQuery>,
) -> Result<Json<DiffResponse>, (StatusCode, String)> {
    state
        .checkpoints
        .file_diff(query.repo, id, query.path)
        .await
        .map(|diff| Json(DiffResponse { diff }))
        .map_err(|e| error_response(&e))
}

/// `GET /api/checkpoints/{id}/file?repo=&path=` — a file's raw content at
/// a checkpoint. `404` if the path doesn't exist there or names a
/// directory.
async fn api_checkpoints_file(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoPathQuery>,
) -> Result<Response, (StatusCode, String)> {
    let content = state
        .checkpoints
        .file_content_at(query.repo, id, query.path.clone())
        .await
        .map_err(|e| error_response(&e))?;
    let Some(bytes) = content else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("{} is not a file at this checkpoint", query.path),
        ));
    };
    let content_type = mime_guess::from_path(&query.path)
        .first_or_octet_stream()
        .to_string();
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, content_type)],
        bytes,
    )
        .into_response())
}

/// Body for `POST /api/checkpoints/{id}/restore`.
#[derive(Deserialize)]
struct RestoreRequest {
    repo: RepoKind,
    /// The file or directory to restore, relative to the repository's root.
    path: String,
}

/// `POST /api/checkpoints/{id}/restore` — restore a path to its content at
/// this checkpoint. Checkpoints the result, so the restore can itself be
/// undone.
async fn api_checkpoints_restore(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<crate::checkpoints::RestoreOutcome>, (StatusCode, String)> {
    let ctx = CheckpointContext::system(
        CheckpointTrigger::Restore,
        format!("restored {} from checkpoint {id}", req.path),
    );
    state
        .checkpoints
        .restore_path(req.repo, id, req.path, ctx)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

/// Body for `POST /api/checkpoints/{id}/undo`.
#[derive(Deserialize)]
struct UndoRequest {
    repo: RepoKind,
}

/// `POST /api/checkpoints/{id}/undo` — revert everything this checkpoint
/// changed back to its content just before it, skipping any path changed
/// again since. Checkpoints the result, so the undo can itself be undone.
async fn api_checkpoints_undo(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Json(req): Json<UndoRequest>,
) -> Result<Json<crate::checkpoints::UndoOutcome>, (StatusCode, String)> {
    let ctx = CheckpointContext::system(CheckpointTrigger::Undo, format!("undid checkpoint {id}"));
    state
        .checkpoints
        .undo_checkpoint(req.repo, id, ctx)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    fn app(engine: Arc<CheckpointEngine>) -> Router {
        checkpoints_api_router(CheckpointApiState {
            checkpoints: engine,
        })
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn list_is_empty_before_any_checkpoint() {
        let router = app(crate::checkpoints::test_engine());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/checkpoints?repo=workspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body.get("items"), Some(&serde_json::json!([])));
    }

    #[tokio::test]
    async fn show_missing_checkpoint_is_404() {
        let router = app(crate::checkpoints::test_engine());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/checkpoints/deadbeef?repo=workspace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stats_report_zero_before_any_checkpoint() {
        let router = app(crate::checkpoints::test_engine());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/checkpoints/stats?repo=config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body.get("checkpoint_count"), Some(&serde_json::json!(0)));
    }
}

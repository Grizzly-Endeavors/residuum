use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use residuum_checkpoints::{
    CheckpointContext, CheckpointEngine, CheckpointError, CheckpointTrigger, RepoKind,
};
use residuum_workspace::team_files::TeamWriter;

pub(crate) const AGENT_REPOS: &[RepoKind] = &[RepoKind::Workspace, RepoKind::AgentConfig];

pub(crate) const HUB_REPOS: &[RepoKind] = &[RepoKind::Hub, RepoKind::Team];

#[derive(Clone)]
pub(crate) struct CheckpointApiState {
    pub checkpoints: Arc<CheckpointEngine>,
    pub repos: &'static [RepoKind],
}

impl CheckpointApiState {
    fn allow(&self, repo: RepoKind) -> Result<(), (StatusCode, String)> {
        if self.repos.contains(&repo) {
            return Ok(());
        }
        let names = self
            .repos
            .iter()
            .map(|kind| format!("{kind:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(" or ");
        Err((
            StatusCode::BAD_REQUEST,
            format!("this route serves the {names} checkpoint repositories only"),
        ))
    }
}

pub(crate) fn checkpoints_api_router(state: CheckpointApiState, prefix: &str) -> Router {
    use axum::routing::{get, post};

    let routes = Router::new()
        .route("/", get(api_checkpoints_list))
        .route("/stats", get(api_checkpoints_stats))
        .route("/{id}", get(api_checkpoints_show))
        .route("/{id}/diff", get(api_checkpoints_diff))
        .route("/{id}/file", get(api_checkpoints_file))
        .route("/{id}/restore", post(api_checkpoints_restore))
        .route("/{id}/undo", post(api_checkpoints_undo));
    Router::new().nest(prefix, routes).with_state(state)
}

#[derive(Deserialize)]
pub(super) struct RepoQuery {
    pub repo: RepoKind,
}

fn error_response(e: &CheckpointError) -> (StatusCode, String) {
    let status = match e {
        CheckpointError::NotFound(_)
        | CheckpointError::PathNotFound(_, _)
        | CheckpointError::InvalidCursor => StatusCode::NOT_FOUND,
        CheckpointError::NotAConfigRepo => StatusCode::BAD_REQUEST,
        CheckpointError::Git(_) | CheckpointError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string())
}

#[derive(Deserialize)]
pub(super) struct ListQuery {
    pub repo: RepoKind,
    pub path: Option<String>,
    pub turn_id: Option<String>,
    pub before: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /api/checkpoints?repo=workspace|agent_config|hub&path=&turn_id=&before=&limit=`
/// — one page of checkpoints, newest first.
async fn api_checkpoints_list(
    State(state): State<CheckpointApiState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<residuum_checkpoints::CheckpointPage>, (StatusCode, String)> {
    state.allow(query.repo)?;
    let limit = query.limit.map(|n| n.clamp(1, 200));
    state
        .checkpoints
        .list_checkpoints(query.repo, query.path, query.turn_id, query.before, limit)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

async fn api_checkpoints_stats(
    State(state): State<CheckpointApiState>,
    Query(query): Query<RepoQuery>,
) -> Result<Json<residuum_checkpoints::RepoStats>, (StatusCode, String)> {
    state.allow(query.repo)?;
    state
        .checkpoints
        .stats(query.repo)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

/// `GET /api/checkpoints/{id}?repo=workspace|agent_config|hub` — a checkpoint's
/// metadata plus the paths it changed.
async fn api_checkpoints_show(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoQuery>,
) -> Result<Json<residuum_checkpoints::CheckpointDetail>, (StatusCode, String)> {
    state.allow(query.repo)?;
    state
        .checkpoints
        .show_checkpoint(query.repo, id)
        .await
        .map(Json)
        .map_err(|e| error_response(&e))
}

#[derive(Deserialize)]
pub(super) struct RepoPathQuery {
    pub repo: RepoKind,
    pub path: String,
}

#[derive(Serialize)]
struct DiffResponse {
    diff: Option<String>,
}

async fn api_checkpoints_diff(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoPathQuery>,
) -> Result<Json<DiffResponse>, (StatusCode, String)> {
    state.allow(query.repo)?;
    state
        .checkpoints
        .file_diff(query.repo, id, query.path)
        .await
        .map(|diff| Json(DiffResponse { diff }))
        .map_err(|e| error_response(&e))
}

/// `GET /api/checkpoints/{id}/file?repo=&path=` — a file's raw content at
/// a checkpoint, marked as a file an agent wrote
/// ([`crate::file_server::mark_agent_file`]). `404` if the path
/// doesn't exist there or names a directory.
async fn api_checkpoints_file(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Query(query): Query<RepoPathQuery>,
) -> Result<Response, (StatusCode, String)> {
    state.allow(query.repo)?;
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
    let mut response = (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, content_type.clone())],
        bytes,
    )
        .into_response();
    crate::file_server::mark_agent_file(response.headers_mut(), &content_type);
    Ok(response)
}

#[derive(Deserialize)]
struct RestoreRequest {
    repo: RepoKind,
    path: String,
}

async fn api_checkpoints_restore(
    State(state): State<CheckpointApiState>,
    Path(id): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<residuum_checkpoints::RestoreOutcome>, (StatusCode, String)> {
    state.allow(req.repo)?;
    let ctx = CheckpointContext::system(
        CheckpointTrigger::Restore,
        format!("restored {} from checkpoint {id}", req.path),
    );
    state
        .checkpoints
        .restore_path(req.repo, id, req.path, ctx, &TeamWriter::User)
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
) -> Result<Json<residuum_checkpoints::UndoOutcome>, (StatusCode, String)> {
    state.allow(req.repo)?;
    let ctx = CheckpointContext::system(CheckpointTrigger::Undo, format!("undid checkpoint {id}"));
    state
        .checkpoints
        .undo_checkpoint(req.repo, id, ctx, &TeamWriter::User)
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
        checkpoints_api_router(
            CheckpointApiState {
                checkpoints: engine,
                repos: &[RepoKind::Workspace, RepoKind::Hub],
            },
            "/api/checkpoints",
        )
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn list_is_empty_before_any_checkpoint() {
        let router = app(residuum_checkpoints::testing::test_engine());
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
        let router = app(residuum_checkpoints::testing::test_engine());
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
    async fn a_checkpointed_page_is_sandboxed_away_from_the_hubs_origin() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("page.html"), "<script></script>").unwrap();
        let engine = Arc::new(
            CheckpointEngine::new(
                "test-agent",
                workspace,
                &residuum_config::paths::TeamPaths::new(dir.path().join("team")),
                dir.path().join("agent-config"),
                dir.path().join("hub"),
                &dir.path().join("checkpoints"),
                None,
            )
            .unwrap(),
        );
        let id = engine
            .checkpoint_workspace_id_before_action(residuum_checkpoints::CheckpointContext::system(
                residuum_checkpoints::CheckpointTrigger::PreAction,
                "test checkpoint",
            ))
            .await
            .unwrap();

        let resp = app(engine)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/checkpoints/{id}/file?repo=workspace&path=page.html"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .unwrap(),
            "sandbox allow-scripts allow-popups allow-downloads"
        );
        assert_eq!(
            headers
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
    }

    #[tokio::test]
    async fn stats_report_zero_before_any_checkpoint() {
        let router = app(residuum_checkpoints::testing::test_engine());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/checkpoints/stats?repo=hub")
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

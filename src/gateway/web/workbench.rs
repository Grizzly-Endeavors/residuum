//! Workbench API: list and delete the team's workbench artifacts, and say
//! where they are served.
//!
//! Artifacts themselves are never served here. They run on the artifacts
//! listener (`crate::workbench::server`), a separate origin, so an
//! agent-written page can't call this API directly; it goes through the web
//! UI's bridge (`web/src/lib/workbench-bridge.ts`), which decides what
//! artifacts may call.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{delete, get};
use serde::Serialize;

use crate::gateway::protocol::{ArtifactSummary, WorkbenchInfo, WorkbenchRelayOrigins};
use crate::tunnel::TunnelStatus;
use crate::workbench::server::WorkbenchServing;
use crate::workbench::{self, ArtifactDeleteError};

#[derive(Clone)]
pub(crate) struct WorkbenchApiState {
    /// `team/workbench`.
    pub dir: PathBuf,
    pub serving: WorkbenchServing,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
    pub checkpoints: std::sync::Arc<crate::checkpoints::CheckpointEngine>,
    /// The user's view of the team namespace. A delete takes the team write
    /// coordinator's locks for the artifact's files and records the user as
    /// their remover. `None` when there is no team (nothing to coordinate).
    pub team: Option<crate::workspace::team_files::TeamFiles>,
}

/// Response from `DELETE /api/workbench/artifacts/{name}`.
#[derive(Debug, Serialize)]
struct DeleteArtifactResponse {
    /// Entries removed: the page or folder (`name/`) and any `<name>.*` data files.
    removed: Vec<String>,
    /// Checkpoint holding the team directory as it was before this delete.
    /// `None` when that checkpoint could not be recorded; the delete still
    /// succeeded, and the UI should not offer Undo.
    checkpoint_id: Option<String>,
}

pub(crate) fn workbench_api_router(state: WorkbenchApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/workbench/info", get(api_workbench_info))
        .route("/api/workbench/artifacts", get(api_workbench_artifacts))
        .route(
            "/api/workbench/artifacts/{name}",
            delete(api_workbench_artifact_delete),
        )
        .with_state(state)
}

/// `GET /api/workbench/info` — where artifacts are served, locally and
/// through the relay. The web UI picks the relay origin when it is itself
/// being viewed through the relay, and the local port otherwise.
async fn api_workbench_info(State(state): State<WorkbenchApiState>) -> Json<WorkbenchInfo> {
    let (port, unavailable_reason) = match &state.serving {
        WorkbenchServing::Running { port } => (Some(*port), None),
        WorkbenchServing::Unavailable { reason } => (None, Some(reason.clone())),
    };
    let relay = match &*state.tunnel_status_rx.borrow() {
        TunnelStatus::Connected {
            origin: Some(ui_origin),
            workbench_origin: Some(artifacts_origin),
            ..
        } => Some(WorkbenchRelayOrigins {
            ui_origin: ui_origin.clone(),
            artifacts_origin: artifacts_origin.clone(),
        }),
        TunnelStatus::Connected { .. } | TunnelStatus::Connecting | TunnelStatus::Disconnected => {
            None
        }
    };
    Json(WorkbenchInfo {
        port,
        unavailable_reason,
        relay,
    })
}

/// `GET /api/workbench/artifacts` — every artifact, most recently modified first.
async fn api_workbench_artifacts(
    State(state): State<WorkbenchApiState>,
) -> Result<Json<Vec<ArtifactSummary>>, (StatusCode, String)> {
    workbench::list_artifacts(&state.dir)
        .await
        .map(Json)
        .map_err(|e| {
            tracing::error!(dir = %state.dir.display(), error = %e, "failed to list workbench artifacts");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't read the workbench folder. Check that the workspace is readable and try again."
                    .to_string(),
            )
        })
}

/// `DELETE /api/workbench/artifacts/{name}` — remove the artifact and its data files.
async fn api_workbench_artifact_delete(
    State(state): State<WorkbenchApiState>,
    Path(name): Path<String>,
) -> Result<Json<DeleteArtifactResponse>, (StatusCode, String)> {
    let checkpoint_id = state
        .checkpoints
        .checkpoint_team_id_before_action(crate::checkpoints::CheckpointContext::system(
            crate::checkpoints::CheckpointTrigger::PreAction,
            format!("delete workbench artifact {name}"),
        ))
        .await;
    match delete_coordinated(&state, &name).await {
        Ok(removed) => {
            tracing::info!(artifact = %name, files = ?removed, "deleted workbench artifact");
            Ok(Json(DeleteArtifactResponse {
                removed,
                checkpoint_id,
            }))
        }
        Err(e @ ArtifactDeleteError::InvalidName(_)) => {
            Err((StatusCode::BAD_REQUEST, e.to_string()))
        }
        Err(ArtifactDeleteError::NotFound(_)) => Err((
            StatusCode::NOT_FOUND,
            "That artifact no longer exists. It may already have been deleted.".to_string(),
        )),
        Err(e @ ArtifactDeleteError::Io { .. }) => {
            tracing::error!(artifact = %name, error = %e, "failed to delete workbench artifact");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Couldn't delete the artifact. Some of its files may remain in the workbench folder; check the workspace permissions and try again.".to_string(),
            ))
        }
    }
}

/// How many times a delete widens its lock set before giving up.
const DELETE_LOCK_ATTEMPTS: usize = 8;

/// Delete the artifact under the team coordinator's locks: the page, the
/// folder and everything in it, and the `<name>.*` data files. A file that
/// appears after the lock set was chosen would be deleted unlocked, so the
/// set is re-listed once the locks are held and widened until it covers
/// everything. Each removed path is recorded as removed by the user, so an
/// agent that read it learns who deleted it.
async fn delete_coordinated(
    state: &WorkbenchApiState,
    name: &str,
) -> Result<Vec<String>, ArtifactDeleteError> {
    let Some(team) = &state.team else {
        return workbench::delete_artifact(&state.dir, name).await;
    };
    if !workbench::is_valid_artifact_name(name) {
        return workbench::delete_artifact(&state.dir, name).await;
    }
    let coordinator = team.coordinator();
    let mut wanted: Vec<PathBuf> = Vec::new();
    let mut held = Vec::new();
    for _ in 0..DELETE_LOCK_ATTEMPTS {
        let listed = workbench::artifact_paths(&state.dir, name).await;
        if !held.is_empty() && listed.iter().all(|path| wanted.contains(path)) {
            break;
        }
        drop(held);
        for path in listed {
            if !wanted.contains(&path) {
                wanted.push(path);
            }
        }
        held = coordinator.lock_all(&wanted).await;
    }
    let result = workbench::delete_artifact(&state.dir, name).await;
    let folder = state.dir.join(name);
    for guard in &held {
        if tokio::fs::symlink_metadata(guard.path()).await.is_ok() {
            continue;
        }
        guard.record_removed(team.writer());
        if guard.path() == folder {
            guard.record_tree_removed(team.writer());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::Response;
    use tower::ServiceExt;

    use super::*;

    fn app(dir: &std::path::Path, serving: WorkbenchServing, status: TunnelStatus) -> axum::Router {
        let (_tx, rx) = tokio::sync::watch::channel(status);
        workbench_api_router(WorkbenchApiState {
            dir: dir.to_path_buf(),
            serving,
            tunnel_status_rx: rx,
            checkpoints: crate::checkpoints::test_engine(),
            team: None,
        })
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn info_reports_local_port_and_relay_origins() {
        let dir = tempfile::tempdir().unwrap();
        let connected = TunnelStatus::Connected {
            user_id: "bear".into(),
            origin: Some("https://bear.agent-residuum.com".into()),
            workbench_origin: Some("https://bear.workbench.agent-residuum.com".into()),
            instance: None,
            a2a_token: None,
        };
        let resp = app(
            dir.path(),
            WorkbenchServing::Running { port: 7702 },
            connected,
        )
        .oneshot(
            Request::get("/api/workbench/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let info = body_json(resp).await;
        assert_eq!(info.get("port"), Some(&serde_json::json!(7702)));
        assert_eq!(
            info.pointer("/relay/artifacts_origin"),
            Some(&serde_json::json!(
                "https://bear.workbench.agent-residuum.com"
            ))
        );
    }

    #[tokio::test]
    async fn info_reports_why_artifacts_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let resp = app(
            dir.path(),
            WorkbenchServing::Unavailable {
                reason: "port 7702 is in use".into(),
            },
            TunnelStatus::Disconnected,
        )
        .oneshot(
            Request::get("/api/workbench/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let info = body_json(resp).await;
        assert_eq!(info.get("port"), Some(&serde_json::Value::Null));
        assert_eq!(info.get("relay"), Some(&serde_json::Value::Null));
        assert_eq!(
            info.get("unavailable_reason"),
            Some(&serde_json::json!("port 7702 is in use"))
        );
    }

    #[tokio::test]
    async fn artifact_pages_are_not_served_on_the_api_origin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.html"), "<title>Chart</title>").unwrap();
        let resp = app(
            dir.path(),
            WorkbenchServing::Running { port: 7702 },
            TunnelStatus::Disconnected,
        )
        .oneshot(
            Request::get("/api/workbench/artifacts/chart")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn list_then_delete_operate_on_the_team_workbench() {
        let root = tempfile::tempdir().unwrap();
        let workbench = crate::workspace::layout::WorkspaceLayout::new(root.path().join("scout"))
            .team()
            .workbench_dir();
        assert_eq!(workbench, root.path().join("team").join("workbench"));
        std::fs::create_dir_all(&workbench).unwrap();
        std::fs::write(workbench.join("chart.html"), "<title>My Chart</title>").unwrap();
        std::fs::write(workbench.join("chart.state.json"), "{}").unwrap();
        let router = app(
            &workbench,
            WorkbenchServing::Running { port: 7702 },
            TunnelStatus::Disconnected,
        );

        let listing = router
            .clone()
            .oneshot(
                Request::get("/api/workbench/artifacts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let artifacts: Vec<ArtifactSummary> =
            serde_json::from_value(body_json(listing).await).unwrap();
        let [artifact] = artifacts.as_slice() else {
            panic!("expected exactly one artifact, got {artifacts:?}");
        };
        assert_eq!(artifact.title, "My Chart");

        let deleted = router
            .clone()
            .oneshot(
                Request::delete("/api/workbench/artifacts/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        assert_eq!(
            body_json(deleted).await.get("removed"),
            Some(&serde_json::json!(["chart.html", "chart.state.json"]))
        );

        let deleted_again = router
            .oneshot(
                Request::delete("/api/workbench/artifacts/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted_again.status(), StatusCode::NOT_FOUND);
        assert!(
            !workbench.join("chart.html").exists() && !workbench.join("chart.state.json").exists(),
            "the artifact and its state file are gone from team/workbench"
        );
    }

    #[tokio::test]
    async fn delete_returns_the_team_checkpoint_taken_before_the_delete() {
        let root = tempfile::tempdir().unwrap();
        let workbench = root.path().join("team").join("workbench");
        std::fs::create_dir_all(&workbench).unwrap();
        std::fs::write(workbench.join("chart.html"), "<title>My Chart</title>").unwrap();
        let checkpoints = std::sync::Arc::new(
            crate::checkpoints::CheckpointEngine::new(
                root.path().join("scout"),
                &crate::config::paths::TeamPaths::new(root.path().join("team")),
                root.path().join("scout").join("config"),
                root.path().join("hub"),
                &root.path().join("hub").join("checkpoints"),
                None,
            )
            .unwrap(),
        );
        let (_tx, rx) = tokio::sync::watch::channel(TunnelStatus::Disconnected);
        let router = workbench_api_router(WorkbenchApiState {
            dir: workbench.clone(),
            serving: WorkbenchServing::Running { port: 7702 },
            tunnel_status_rx: rx,
            checkpoints: std::sync::Arc::clone(&checkpoints),
            team: None,
        });

        let deleted = router
            .oneshot(
                Request::delete("/api/workbench/artifacts/chart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK, "delete should succeed");
        let body = body_json(deleted).await;
        let id = body
            .get("checkpoint_id")
            .and_then(serde_json::Value::as_str)
            .expect("delete should name the checkpoint taken before it")
            .to_string();

        let stored = checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Team,
                id.clone(),
                "workbench/chart.html".to_string(),
            )
            .await
            .unwrap();
        assert!(
            stored.is_some(),
            "the returned team checkpoint must still contain the artifact"
        );
        assert!(!workbench.join("chart.html").exists());

        checkpoints
            .restore_path(
                crate::checkpoints::RepoKind::Team,
                id,
                "workbench/chart.html".to_string(),
                crate::checkpoints::CheckpointContext::system(
                    crate::checkpoints::CheckpointTrigger::Restore,
                    "undo delete",
                ),
                &crate::workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();
        assert!(
            workbench.join("chart.html").exists(),
            "restoring from the team repo brings the artifact back"
        );
    }

    #[tokio::test]
    async fn delete_takes_the_path_locks_and_records_the_user() {
        use crate::tools::team_namespace_tests::Hub;

        let hub = Hub::new();
        let workbench = hub.team().join("workbench");
        std::fs::create_dir_all(&workbench).unwrap();
        std::fs::write(workbench.join("chart.html"), "<title>Chart</title>").unwrap();
        std::fs::write(workbench.join("chart.state.json"), "{}").unwrap();
        let sam = hub.tools_for("sam");
        let read = sam
            .execute(
                "read_file",
                serde_json::json!({ "path": "team/workbench/chart.html" }),
            )
            .await
            .unwrap();
        assert!(!read.is_error, "{}", read.output);

        let (_tx, rx) = tokio::sync::watch::channel(TunnelStatus::Disconnected);
        let router = workbench_api_router(WorkbenchApiState {
            dir: workbench.clone(),
            serving: WorkbenchServing::Running { port: 7702 },
            tunnel_status_rx: rx,
            checkpoints: crate::checkpoints::test_engine(),
            team: Some(hub.coordinator.view_for_user(hub.agent_dir("scout"))),
        });

        let held = hub
            .coordinator
            .lock(&workbench.join("chart.state.json"))
            .await;
        let deleting = router.oneshot(
            Request::delete("/api/workbench/artifacts/chart")
                .body(Body::empty())
                .unwrap(),
        );
        tokio::pin!(deleting);
        let early =
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut deleting).await;
        assert!(
            early.is_err(),
            "the delete must wait for the data file's lock"
        );
        assert!(workbench.join("chart.html").exists());

        drop(held);
        let response = deleting.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!workbench.join("chart.html").exists());

        let refused = sam
            .execute(
                "write_file",
                serde_json::json!({ "path": "team/workbench/chart.html", "content": "again" }),
            )
            .await
            .unwrap();
        assert!(refused.is_error, "{}", refused.output);
        assert!(refused.output.contains("the user"), "{}", refused.output);
    }
}

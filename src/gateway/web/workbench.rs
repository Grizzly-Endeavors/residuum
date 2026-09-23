//! Workbench API: list and delete the agent's workbench artifacts, and say
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
    /// `<workspace>/workbench`.
    pub dir: PathBuf,
    pub serving: WorkbenchServing,
    pub tunnel_status_rx: tokio::sync::watch::Receiver<TunnelStatus>,
}

/// Response from `DELETE /api/workbench/artifacts/{name}`.
#[derive(Debug, Serialize)]
struct DeleteArtifactResponse {
    /// Entries removed: the page or folder (`name/`) and any `<name>.*` data files.
    removed: Vec<String>,
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
    match workbench::delete_artifact(&state.dir, &name).await {
        Ok(removed) => {
            tracing::info!(artifact = %name, files = ?removed, "deleted workbench artifact");
            Ok(Json(DeleteArtifactResponse { removed }))
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
    async fn list_then_delete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chart.html"), "<title>My Chart</title>").unwrap();
        std::fs::write(dir.path().join("chart.state.json"), "{}").unwrap();
        let router = app(
            dir.path(),
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
    }
}

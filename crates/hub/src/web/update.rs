use std::path::PathBuf;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::update::SharedUpdateStatus;
use residuum_core::build_info::BuildInfo;

#[derive(Clone)]
pub(crate) struct UpdateApiState {
    pub build: BuildInfo,
    pub update_status: SharedUpdateStatus,
    pub restart_tx: mpsc::Sender<()>,
    pub gateway_shutdown_tx: mpsc::Sender<()>,
    pub hub_dir: PathBuf,
}

#[derive(Serialize)]
pub(crate) struct RollbackNoticeResponse {
    attempted_version: String,
    reason: String,
    at: String,
}

impl From<crate::update::RollbackNotice> for RollbackNoticeResponse {
    fn from(notice: crate::update::RollbackNotice) -> Self {
        Self {
            attempted_version: notice.attempted_version,
            reason: notice.reason,
            at: notice.at.to_rfc3339(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct UnverifiedUpdateResponse {
    version: String,
    at: String,
}

impl From<crate::update::UnverifiedUpdate> for UnverifiedUpdateResponse {
    fn from(notice: crate::update::UnverifiedUpdate) -> Self {
        Self {
            version: notice.version,
            at: notice.at.to_rfc3339(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct UpdateStatusResponse {
    current: String,
    latest: Option<String>,
    update_available: bool,
    last_checked: Option<String>,
    checking: bool,
    rollback_notice: Option<RollbackNoticeResponse>,
    unverified_update: Option<UnverifiedUpdateResponse>,
}

pub(crate) async fn api_update_status(
    State(state): State<UpdateApiState>,
) -> Json<UpdateStatusResponse> {
    Json(current_status(&state).await)
}

pub(crate) async fn api_update_check(
    State(state): State<UpdateApiState>,
) -> Json<UpdateStatusResponse> {
    crate::update::check_for_update(&state.update_status).await;
    Json(current_status(&state).await)
}

async fn current_status(state: &UpdateApiState) -> UpdateStatusResponse {
    let rollback_notice = crate::update::read_rollback_notice(&state.hub_dir).map(Into::into);
    let unverified_update = crate::update::read_unverified_update(&state.hub_dir).map(Into::into);
    let s = state.update_status.read().await;
    UpdateStatusResponse {
        current: s.current.clone(),
        latest: s.latest.clone(),
        update_available: s.update_available,
        last_checked: s.last_checked.map(|dt| dt.to_rfc3339()),
        checking: s.checking,
        rollback_notice,
        unverified_update,
    }
}

pub(crate) async fn api_update_apply(
    State(state): State<UpdateApiState>,
) -> Result<Json<UpdateStatusResponse>, (StatusCode, String)> {
    let version = state
        .update_status
        .read()
        .await
        .latest
        .clone()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "no update version known — run a check first".to_string(),
            )
        })?;

    crate::update::download_and_install(state.build.version, &version)
        .await
        .map_err(|e| match crate::update::rejection_message(&e) {
            Some(message) => (StatusCode::UNPROCESSABLE_ENTITY, message.to_string()),
            None => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
        })?;

    tracing::info!(version = %version, "update installed, sending restart signal");

    {
        let mut s = state.update_status.write().await;
        s.update_available = false;
    }

    state.restart_tx.send(()).await.map_err(|_closed| {
        tracing::error!(version = %version, "update installed but the restart signal could not be sent");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "The update was installed, but residuum couldn't restart itself. Restart it to finish updating.".to_string(),
        )
    })?;

    Ok(Json(current_status(&state).await))
}

pub(crate) async fn api_update_restart(
    State(state): State<UpdateApiState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.restart_tx.send(()).await.map_err(|_closed| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "restart channel closed".to_string(),
        )
    })?;

    Ok(Json(serde_json::json!({ "restarting": true })))
}

pub(crate) async fn api_shutdown(
    State(state): State<UpdateApiState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .gateway_shutdown_tx
        .send(())
        .await
        .map_err(|_closed| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "shutdown channel closed".to_string(),
            )
        })?;

    Ok(Json(serde_json::json!({ "shutting_down": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(hub_dir: PathBuf) -> UpdateApiState {
        let (restart_tx, _restart_rx) = mpsc::channel(1);
        let (gateway_shutdown_tx, _shutdown_rx) = mpsc::channel(1);
        let build = residuum_core::testing::HARNESS_BUILD;
        UpdateApiState {
            build,
            update_status: crate::update::shared_update_status(build.version),
            restart_tx,
            gateway_shutdown_tx,
            hub_dir,
        }
    }

    #[tokio::test]
    async fn status_has_no_rollback_notice_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().to_path_buf());
        let status = current_status(&state).await;
        assert!(status.rollback_notice.is_none());
        assert!(status.unverified_update.is_none());
    }

    #[tokio::test]
    async fn status_surfaces_a_rollback_notice_left_by_the_watchdog() {
        let dir = tempfile::tempdir().unwrap();
        crate::update::write_rollback_notice(
            dir.path(),
            &crate::update::RollbackNotice {
                attempted_version: "v2026.09.24".to_string(),
                reason: "did not become healthy within 60s".to_string(),
                at: chrono::Utc::now(),
            },
        );

        let state = test_state(dir.path().to_path_buf());
        let status = current_status(&state).await;
        let notice = status
            .rollback_notice
            .expect("rollback notice should be surfaced");
        assert_eq!(notice.attempted_version, "v2026.09.24");
        assert_eq!(notice.reason, "did not become healthy within 60s");
    }

    #[tokio::test]
    async fn status_surfaces_an_unverified_update() {
        let dir = tempfile::tempdir().unwrap();
        crate::update::write_unverified_update(
            dir.path(),
            &crate::update::UnverifiedUpdate {
                version: "v2026.03.02".to_string(),
                at: chrono::Utc::now(),
            },
        );

        let state = test_state(dir.path().to_path_buf());
        let status = current_status(&state).await;
        let notice = status
            .unverified_update
            .expect("unverified update should be surfaced");
        assert_eq!(notice.version, "v2026.03.02");
    }
}

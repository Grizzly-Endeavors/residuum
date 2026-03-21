//! Update status and control API endpoints.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::update::SharedUpdateStatus;

/// Shared state for the update API routes.
#[derive(Clone)]
pub(crate) struct UpdateApiState {
    pub update_status: SharedUpdateStatus,
    pub restart_tx: mpsc::Sender<()>,
}

/// Response from `GET /api/update/status` and `POST /api/update/check`.
#[derive(Serialize)]
pub(crate) struct UpdateStatusResponse {
    current: String,
    latest: Option<String>,
    update_available: bool,
    last_checked: Option<String>,
    checking: bool,
}

async fn read_update_status(status: &SharedUpdateStatus) -> Json<UpdateStatusResponse> {
    let s = status.read().await;
    Json(UpdateStatusResponse {
        current: s.current.clone(),
        latest: s.latest.clone(),
        update_available: s.update_available,
        last_checked: s.last_checked.map(|dt| dt.to_rfc3339()),
        checking: s.checking,
    })
}

/// `GET /api/update/status` — return current update state.
pub(crate) async fn api_update_status(
    State(state): State<UpdateApiState>,
) -> Json<UpdateStatusResponse> {
    read_update_status(&state.update_status).await
}

/// `POST /api/update/check` — trigger an immediate check, return refreshed status.
pub(crate) async fn api_update_check(
    State(state): State<UpdateApiState>,
) -> Json<UpdateStatusResponse> {
    crate::update::check_for_update(&state.update_status).await;
    read_update_status(&state.update_status).await
}

/// `POST /api/update/apply` — download, install, then restart.
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

    crate::update::download_and_install(&version)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;

    tracing::info!(version = %version, "update installed, sending restart signal");

    // Update shared status to reflect the install
    {
        let mut s = state.update_status.write().await;
        s.update_available = false;
    }

    // Signal the event loop to restart
    state.restart_tx.send(()).await.ok();

    Ok(read_update_status(&state.update_status).await)
}

/// `POST /api/update/restart` — send restart signal only (binary already replaced).
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

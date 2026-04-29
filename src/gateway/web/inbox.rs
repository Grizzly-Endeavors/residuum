//! User Inbox API endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Serialize;

use crate::inbox::InboxItem;

use super::ConfigApiState;

/// A wrapper around `InboxItem` that includes the ID (filename stem).
#[derive(Serialize)]
pub(super) struct ApiInboxItem {
    pub id: String,
    #[serde(flatten)]
    pub item: InboxItem,
}

/// `GET /api/inbox` — List all user inbox items.
pub(super) async fn api_inbox_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<Vec<ApiInboxItem>>, (StatusCode, String)> {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();

    let items = crate::inbox::list_items(&user_inbox_dir)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list user inbox items: {e}"),
            )
        })?;

    let api_items = items
        .into_iter()
        .map(|(id, item)| ApiInboxItem { id, item })
        .collect();

    Ok(Json(api_items))
}

/// `PUT /api/inbox/:id/read` — Mark an inbox item as read.
pub(super) async fn api_inbox_read(
    Path(id): Path<String>,
    State(state): State<ConfigApiState>,
) -> Result<Json<ApiInboxItem>, (StatusCode, String)> {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();

    let item = crate::inbox::mark_read(&user_inbox_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to mark inbox item as read: {e}"),
            )
        })?;

    Ok(Json(ApiInboxItem { id, item }))
}

/// `POST /api/inbox/:id/archive` — Archive an inbox item.
pub(super) async fn api_inbox_archive(
    Path(id): Path<String>,
    State(state): State<ConfigApiState>,
) -> Result<Json<()>, (StatusCode, String)> {
    let layout = crate::workspace::layout::WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let archive_dir = layout.user_inbox_archive_dir();

    crate::inbox::archive_item(&user_inbox_dir, &archive_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to archive inbox item: {e}"),
            )
        })?;

    Ok(Json(()))
}

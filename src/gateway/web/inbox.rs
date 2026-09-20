//! User Inbox API endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

use crate::inbox::InboxItem;
use crate::workspace::layout::WorkspaceLayout;

use super::ConfigApiState;

/// Attachment metadata exposed to the web client. The on-disk path never leaves
/// the server — only what's needed to display and fetch the file.
#[derive(Serialize)]
pub(super) struct ApiAttachment {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub url: String,
}

/// A wrapper around `InboxItem` that includes the ID (filename stem) and resolves
/// attachments to servable metadata instead of the raw workspace paths `InboxItem`
/// stores internally.
#[derive(Serialize)]
pub(super) struct ApiInboxItem {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    #[serde(with = "crate::time::minute_format")]
    pub timestamp: chrono::NaiveDateTime,
    pub read: bool,
    pub attachments: Vec<ApiAttachment>,
}

/// Resolve an item's attachments into servable metadata by statting each file
/// under `attachments_root/<id>/`.
///
/// An attachment whose file can't be found on disk is dropped from the listing
/// (rather than shown as a broken link) and logged — this can happen if a
/// workspace was hand-edited, but should not happen in normal operation.
async fn resolve_attachments(
    id: &str,
    item: &InboxItem,
    attachments_root: &std::path::Path,
) -> Vec<ApiAttachment> {
    let mut resolved = Vec::with_capacity(item.attachments.len());
    for (index, stored) in item.attachments.iter().enumerate() {
        let Some(file_name) = stored.file_name().and_then(|f| f.to_str()) else {
            tracing::warn!(id = %id, stored = %stored.display(), "inbox attachment entry has no filename, omitting");
            continue;
        };
        let path = attachments_root.join(id).join(file_name);
        match tokio::fs::metadata(&path).await {
            Ok(meta) => resolved.push(ApiAttachment {
                filename: file_name.to_string(),
                mime_type: crate::interfaces::attachment::detect_mime_type(&path),
                size: meta.len(),
                url: format!("/api/inbox/{id}/attachments/{index}"),
            }),
            Err(e) => {
                tracing::warn!(
                    id = %id,
                    filename = %file_name,
                    error = %e,
                    "inbox attachment file missing on disk, omitting from listing"
                );
            }
        }
    }
    resolved
}

/// Build the API-facing representation of an inbox item, resolving its
/// attachments against `attachments_root`.
async fn to_api_item(
    id: String,
    item: InboxItem,
    attachments_root: &std::path::Path,
) -> ApiInboxItem {
    let attachments = resolve_attachments(&id, &item, attachments_root).await;
    ApiInboxItem {
        id,
        title: item.title,
        body: item.body,
        source: item.source,
        timestamp: item.timestamp,
        read: item.read,
        attachments,
    }
}

/// `GET /api/inbox` — List all user inbox items.
pub(super) async fn api_inbox_list(
    State(state): State<ConfigApiState>,
) -> Result<Json<Vec<ApiInboxItem>>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let attachments_root = layout.user_inbox_attachments_dir();

    let items = crate::inbox::list_items(&user_inbox_dir)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list user inbox items: {e}"),
            )
        })?;

    let mut api_items = Vec::with_capacity(items.len());
    for (id, item) in items {
        api_items.push(to_api_item(id, item, &attachments_root).await);
    }

    Ok(Json(api_items))
}

/// `PUT /api/inbox/:id/read` — Mark an inbox item as read.
pub(super) async fn api_inbox_read(
    Path(id): Path<String>,
    State(state): State<ConfigApiState>,
) -> Result<Json<ApiInboxItem>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let attachments_root = layout.user_inbox_attachments_dir();

    let item = crate::inbox::mark_read(&user_inbox_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to mark inbox item as read: {e}"),
            )
        })?;

    Ok(Json(to_api_item(id, item, &attachments_root).await))
}

/// `POST /api/inbox/:id/archive` — Archive an inbox item.
pub(super) async fn api_inbox_archive(
    Path(id): Path<String>,
    State(state): State<ConfigApiState>,
) -> Result<Json<()>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
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

/// Resolve `root/id/name`, confining it to `root/id`.
///
/// Canonicalizes both the item directory and the candidate file and requires the
/// candidate to sit inside the item directory, and the item directory to sit
/// inside `root` — two checks because `id` and `name` both ultimately come from
/// data the agent controls (the inbox item ID and its stored attachment list) and
/// neither is trusted as a literal filesystem path. Returns `None` on any
/// resolution failure, which callers turn into a 404 rather than a 403 so an
/// out-of-tree path is indistinguishable from one that simply doesn't exist.
async fn confine(
    root: &std::path::Path,
    id: &str,
    name: &std::ffi::OsStr,
) -> Option<std::path::PathBuf> {
    let root_canon = tokio::fs::canonicalize(root).await.ok()?;

    let item_dir = root.join(id);
    let item_dir_canon = tokio::fs::canonicalize(&item_dir).await.ok()?;
    if !item_dir_canon.starts_with(&root_canon) {
        return None;
    }

    let candidate = item_dir.join(name);
    let candidate_canon = tokio::fs::canonicalize(&candidate).await.ok()?;
    candidate_canon
        .starts_with(&item_dir_canon)
        .then_some(candidate_canon)
}

/// `GET /api/inbox/:id/attachments/:index` — Serve one of a user inbox item's
/// attachments by its position in the item's attachment list.
///
/// Checks the active inbox first, then the archive, so a link handed out before
/// an item is archived keeps working afterward. Never trusts the item's stored
/// attachment path as a literal filesystem path — see `confine`.
pub(super) async fn api_inbox_attachment(
    Path((id, index)): Path<(String, usize)>,
    State(state): State<ConfigApiState>,
) -> Response {
    let layout = WorkspaceLayout::new(&state.workspace_dir);

    let active_json = layout.user_inbox_dir().join(format!("{id}.json"));
    let (item, attachments_root) = if let Ok(item) = crate::inbox::load_item(&active_json).await {
        (item, layout.user_inbox_attachments_dir())
    } else {
        let archived_json = layout.user_inbox_archive_dir().join(format!("{id}.json"));
        let Ok(item) = crate::inbox::load_item(&archived_json).await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        (item, layout.user_inbox_archive_attachments_dir())
    };

    let Some(stored) = item.attachments.get(index) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(file_name) = stored.file_name() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Some(resolved) = confine(&attachments_root, &id, file_name).await else {
        tracing::warn!(id = %id, index, "rejected inbox attachment request outside item directory");
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(bytes) = tokio::fs::read(&resolved).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime_type = crate::interfaces::attachment::detect_mime_type(&resolved);
    let safe_filename: String = file_name
        .to_string_lossy()
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let disposition = format!("inline; filename=\"{safe_filename}\"");

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(v) = axum::http::HeaderValue::from_str(&mime_type) {
        headers.insert(axum::http::header::CONTENT_TYPE, v);
    }
    if let Ok(v) = axum::http::HeaderValue::from_str(&disposition) {
        headers.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(workspace_dir: std::path::PathBuf) -> ConfigApiState {
        ConfigApiState {
            config_dir: workspace_dir.clone(),
            workspace_dir,
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn write_active_item(
        layout: &WorkspaceLayout,
        id: &str,
        attachments: Vec<std::path::PathBuf>,
    ) {
        tokio::fs::create_dir_all(layout.user_inbox_dir())
            .await
            .unwrap();
        let item = InboxItem {
            title: "test item".to_string(),
            body: "body".to_string(),
            source: "test".to_string(),
            timestamp: crate::time::now_local(chrono_tz::UTC),
            read: false,
            attachments,
        };
        crate::inbox::save_item(&layout.user_inbox_dir(), &format!("{id}.json"), &item)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn api_inbox_attachment_serves_active_item_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let attachments_dir = layout.user_inbox_attachments_dir().join("item1");
        tokio::fs::create_dir_all(&attachments_dir).await.unwrap();
        tokio::fs::write(attachments_dir.join("note.txt"), b"hello")
            .await
            .unwrap();
        write_active_item(
            &layout,
            "item1",
            vec![std::path::PathBuf::from(
                "inbox/user/attachments/item1/note.txt",
            )],
        )
        .await;

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(Path(("item1".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_inbox_attachment_out_of_range_index_404s() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        write_active_item(&layout, "item1", vec![]).await;

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(Path(("item1".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_inbox_attachment_unknown_item_404s() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_state(dir.path().to_path_buf());
        let response =
            api_inbox_attachment(Path(("does-not-exist".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_inbox_attachment_missing_file_on_disk_404s() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        // The item's own directory exists (so `confine` gets past the first
        // canonicalize), but the file it names was never actually copied there.
        tokio::fs::create_dir_all(layout.user_inbox_attachments_dir().join("item1"))
            .await
            .unwrap();
        write_active_item(
            &layout,
            "item1",
            vec![std::path::PathBuf::from(
                "inbox/user/attachments/item1/never_copied.txt",
            )],
        )
        .await;

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(Path(("item1".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn api_inbox_attachment_rejects_symlink_escaping_item_directory() {
        // The stored filename itself can never contain a traversal component
        // (`Path::file_name` already strips those), so the realistic way a
        // confined-looking path can still resolve outside the item directory is
        // via a symlink planted inside it. `confine`'s canonicalize-then-`starts_with`
        // check must catch that.
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        let outside = dir.path().join("outside.txt");
        tokio::fs::write(&outside, b"secret").await.unwrap();

        let item_dir = layout.user_inbox_attachments_dir().join("item1");
        tokio::fs::create_dir_all(&item_dir).await.unwrap();
        std::os::unix::fs::symlink(&outside, item_dir.join("note.txt")).unwrap();

        write_active_item(
            &layout,
            "item1",
            vec![std::path::PathBuf::from(
                "inbox/user/attachments/item1/note.txt",
            )],
        )
        .await;

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(Path(("item1".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_inbox_attachment_serves_archived_item_file() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let archive_attachments = layout.user_inbox_archive_attachments_dir().join("item1");
        tokio::fs::create_dir_all(&archive_attachments)
            .await
            .unwrap();
        tokio::fs::write(archive_attachments.join("note.txt"), b"archived")
            .await
            .unwrap();

        tokio::fs::create_dir_all(layout.user_inbox_archive_dir())
            .await
            .unwrap();
        let item = InboxItem {
            title: "archived item".to_string(),
            body: "body".to_string(),
            source: "test".to_string(),
            timestamp: crate::time::now_local(chrono_tz::UTC),
            read: true,
            attachments: vec![std::path::PathBuf::from(
                "archive/inbox/user/attachments/item1/note.txt",
            )],
        };
        crate::inbox::save_item(&layout.user_inbox_archive_dir(), "item1.json", &item)
            .await
            .unwrap();

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(Path(("item1".to_string(), 0)), State(state)).await;

        assert_eq!(response.status(), StatusCode::OK);
    }
}

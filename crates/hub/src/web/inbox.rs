//! User inbox API endpoints: the read-only listing plus per-item actions
//! and attachments, file routes that answer whether or not the agent is
//! running.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

use residuum_config::WorkspaceLayout;
use residuum_net::browser_landing::{self, AppNotice, Fallback};
use residuum_workspace::inbox::{InboxAttachment, InboxItem};

use super::AgentFilesState;

/// A wrapper around `InboxItem` that includes the ID (filename stem) and resolves
/// attachments to servable metadata instead of the raw workspace paths `InboxItem`
/// stores internally.
#[derive(Serialize)]
pub(super) struct ApiInboxItem {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    #[serde(with = "residuum_core::time::minute_format")]
    pub timestamp: chrono::NaiveDateTime,
    pub read: bool,
    pub attachments: Vec<InboxAttachment>,
}

/// Build the API-facing representation of an inbox item, resolving its
/// attachments against `attachments_root`.
async fn to_api_item(
    agent: &str,
    id: String,
    item: InboxItem,
    attachments_root: &std::path::Path,
) -> ApiInboxItem {
    let attachments =
        residuum_workspace::inbox::resolve_attachments(agent, &id, &item, attachments_root).await;
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

/// `GET /api/agents/{name}/inbox` — List all user inbox items.
pub(super) async fn api_inbox_list(
    State(state): State<AgentFilesState>,
) -> Result<Json<Vec<ApiInboxItem>>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let attachments_root = layout.user_inbox_attachments_dir();

    let items = residuum_workspace::inbox::list_items_or_empty(&user_inbox_dir)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list user inbox items: {e}"),
            )
        })?;

    let mut api_items = Vec::with_capacity(items.len());
    for (id, item) in items {
        api_items.push(to_api_item(&state.agent_name, id, item, &attachments_root).await);
    }

    Ok(Json(api_items))
}

/// `GET /api/agents/{name}/inbox/archive` — List all archived user inbox items.
pub(super) async fn api_inbox_archive_list(
    State(state): State<AgentFilesState>,
) -> Result<Json<Vec<ApiInboxItem>>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let archive_dir = layout.user_inbox_archive_dir();
    let attachments_root = layout.user_inbox_archive_attachments_dir();

    let items = residuum_workspace::inbox::list_items_or_empty(&archive_dir)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list archived inbox items: {e}"),
            )
        })?;

    let mut api_items = Vec::with_capacity(items.len());
    for (id, item) in items {
        api_items.push(to_api_item(&state.agent_name, id, item, &attachments_root).await);
    }

    Ok(Json(api_items))
}

/// `PUT /api/agents/{name}/inbox/:id/read` — Mark an inbox item as read.
pub(super) async fn api_inbox_read(
    Path(id): Path<String>,
    State(state): State<AgentFilesState>,
) -> Result<Json<ApiInboxItem>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let attachments_root = layout.user_inbox_attachments_dir();

    let item = residuum_workspace::inbox::mark_read(&user_inbox_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to mark inbox item as read: {e}"),
            )
        })?;

    Ok(Json(
        to_api_item(&state.agent_name, id, item, &attachments_root).await,
    ))
}

/// `POST /api/agents/{name}/inbox/:id/archive` — Archive an inbox item.
pub(super) async fn api_inbox_archive(
    Path(id): Path<String>,
    State(state): State<AgentFilesState>,
) -> Result<Json<()>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let archive_dir = layout.user_inbox_archive_dir();

    residuum_workspace::inbox::archive_item(&user_inbox_dir, &archive_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to archive inbox item: {e}"),
            )
        })?;

    Ok(Json(()))
}

/// `POST /api/agents/{name}/inbox/:id/restore` — Restore an archived inbox item back to
/// the active inbox — the one way to undo `api_inbox_archive`.
pub(super) async fn api_inbox_restore(
    Path(id): Path<String>,
    State(state): State<AgentFilesState>,
) -> Result<Json<()>, (StatusCode, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let user_inbox_dir = layout.user_inbox_dir();
    let archive_dir = layout.user_inbox_archive_dir();

    residuum_workspace::inbox::restore_item(&archive_dir, &user_inbox_dir, &id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to restore inbox item: {e}"),
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

/// `GET /api/agents/{name}/inbox/:id/attachments/:index` — Serve one of a user inbox item's
/// attachments by its position in the item's attachment list.
///
/// Checks the active inbox first, then the archive, so a link handed out before
/// an item is archived keeps working afterward. Never trusts the item's stored
/// attachment path as a literal filesystem path — see `confine`.
pub(super) async fn api_inbox_attachment(
    method: Method,
    request_headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    State(state): State<AgentFilesState>,
) -> Response {
    let Some((resolved, file_name)) = find_inbox_attachment(&state, &id, index).await else {
        tracing::debug!(id = %id, index, "inbox attachment requested that no longer exists");
        return missing_attachment(&state, &id, &method, &request_headers);
    };
    let Ok(bytes) = tokio::fs::read(&resolved).await else {
        tracing::warn!(id = %id, index, path = %resolved.display(), "inbox attachment not readable");
        return missing_attachment(&state, &id, &method, &request_headers);
    };

    let mime_type = residuum_bus::attachment::detect_mime_type(&resolved);
    crate::file_server::respond_with_file(bytes, &mime_type, &file_name)
}

/// The answer for an attachment that is gone: a browser that opened the link
/// goes to the item in the Inbox, which says so; anything else gets a JSON
/// error.
fn missing_attachment(
    state: &AgentFilesState,
    id: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    const MESSAGE: &str =
        "This attachment is no longer available. It may have been removed from its inbox item.";
    if browser_landing::is_top_level_navigation(method, headers) {
        let item = format!("{}:{id}", state.agent_name);
        return browser_landing::app_redirect(
            &browser_landing::app_path(
                "/inbox",
                &[
                    ("item", &item),
                    ("notice", AppNotice::InboxAttachmentMissing.code()),
                ],
            ),
            Fallback {
                title: "Attachment unavailable",
                message: MESSAGE,
            },
        );
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": MESSAGE })),
    )
        .into_response()
}

/// The file of attachment `index` of inbox item `id` (active or archived),
/// confined to the item's attachment directory, and its file name.
async fn find_inbox_attachment(
    state: &AgentFilesState,
    id: &str,
    index: usize,
) -> Option<(std::path::PathBuf, String)> {
    let layout = WorkspaceLayout::new(&state.workspace_dir);

    let active_json = layout.user_inbox_dir().join(format!("{id}.json"));
    let (item, attachments_root) =
        if let Ok(item) = residuum_workspace::inbox::load_item(&active_json).await {
            (item, layout.user_inbox_attachments_dir())
        } else {
            let archived_json = layout.user_inbox_archive_dir().join(format!("{id}.json"));
            let item = residuum_workspace::inbox::load_item(&archived_json)
                .await
                .ok()?;
            (item, layout.user_inbox_archive_attachments_dir())
        };

    let file_name = item.attachments.get(index)?.file_name()?.to_owned();
    let Some(resolved) = confine(&attachments_root, id, &file_name).await else {
        tracing::warn!(id = %id, index, "rejected inbox attachment request outside item directory");
        return None;
    };
    Some((resolved, file_name.to_string_lossy().into_owned()))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    fn make_state(workspace_dir: std::path::PathBuf) -> AgentFilesState {
        AgentFilesState::for_test(workspace_dir, None)
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
            timestamp: residuum_core::time::now_local(chrono_tz::UTC),
            read: false,
            attachments,
            in_reply_to: None,
        };
        residuum_workspace::inbox::save_item(
            &layout.user_inbox_dir(),
            &format!("{id}.json"),
            &item,
        )
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
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("item1".to_string(), 0)),
            State(state),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn listed_attachment_urls_are_under_the_owning_agents_routes() {
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
        let Json(items) = api_inbox_list(State(state)).await.unwrap();

        let item = items.first().expect("the seeded item is listed");
        let attachment = item.attachments.first().expect("its attachment is listed");
        assert_eq!(
            attachment.url,
            "/api/agents/test-agent/inbox/item1/attachments/0"
        );
    }

    #[tokio::test]
    async fn api_inbox_attachment_out_of_range_index_404s() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        write_active_item(&layout, "item1", vec![]).await;

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("item1".to_string(), 0)),
            State(state),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_inbox_attachment_unknown_item_404s() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("does-not-exist".to_string(), 0)),
            State(state),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("no longer available")),
            "a fetch is told why, in the API's error shape: {body}"
        );
    }

    #[tokio::test]
    async fn a_tab_opening_a_missing_attachment_goes_to_its_inbox_item() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_state(dir.path().to_path_buf());
        let mut headers = HeaderMap::new();
        headers.insert("sec-fetch-dest", "document".parse().unwrap());
        headers.insert("sec-fetch-mode", "navigate".parse().unwrap());
        let response = api_inbox_attachment(
            Method::GET,
            headers,
            Path(("item1".to_string(), 2)),
            State(state),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER, "redirected");
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .unwrap(),
            "/inbox?item=test-agent%3Aitem1&notice=inbox-attachment-missing",
            "to the item, with the notice"
        );
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
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("item1".to_string(), 0)),
            State(state),
        )
        .await;

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
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("item1".to_string(), 0)),
            State(state),
        )
        .await;

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
            timestamp: residuum_core::time::now_local(chrono_tz::UTC),
            read: true,
            attachments: vec![std::path::PathBuf::from(
                "archive/inbox/user/attachments/item1/note.txt",
            )],
            in_reply_to: None,
        };
        residuum_workspace::inbox::save_item(&layout.user_inbox_archive_dir(), "item1.json", &item)
            .await
            .unwrap();

        let state = make_state(dir.path().to_path_buf());
        let response = api_inbox_attachment(
            Method::GET,
            HeaderMap::new(),
            Path(("item1".to_string(), 0)),
            State(state),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_inbox_archive_then_restore_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        write_active_item(&layout, "item1", vec![]).await;
        let state = make_state(dir.path().to_path_buf());

        let Json(()) = api_inbox_archive(Path("item1".to_string()), State(state.clone()))
            .await
            .unwrap();

        let active = api_inbox_list(State(state.clone())).await.unwrap();
        assert!(
            active.0.is_empty(),
            "item should be gone from the active list"
        );
        let archived = api_inbox_archive_list(State(state.clone())).await.unwrap();
        assert_eq!(
            archived.0.len(),
            1,
            "item should show up in the archive list"
        );
        assert_eq!(archived.0[0].id, "item1");

        let Json(()) = api_inbox_restore(Path("item1".to_string()), State(state.clone()))
            .await
            .unwrap();

        let active_after_restore = api_inbox_list(State(state.clone())).await.unwrap();
        assert_eq!(active_after_restore.0.len(), 1, "item should be back");
        assert_eq!(active_after_restore.0[0].id, "item1");
        let archived_after_restore = api_inbox_archive_list(State(state)).await.unwrap();
        assert!(
            archived_after_restore.0.is_empty(),
            "archive should be empty after restore"
        );
    }

    #[tokio::test]
    async fn inbox_listings_are_empty_before_the_directories_exist() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_state(dir.path().to_path_buf());

        let Json(active) = api_inbox_list(State(state.clone())).await.unwrap();
        let Json(archived) = api_inbox_archive_list(State(state)).await.unwrap();

        assert!(active.is_empty(), "no inbox directory means no items");
        assert!(archived.is_empty(), "no archive directory means no items");
    }

    #[tokio::test]
    async fn an_unreadable_inbox_directory_is_still_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.user_inbox_dir().parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(layout.user_inbox_dir(), b"not a directory")
            .await
            .unwrap();
        let state = make_state(dir.path().to_path_buf());

        let Err((status, message)) = api_inbox_list(State(state)).await else {
            panic!("a directory that can't be read is not an empty inbox");
        };

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            message.contains("failed to list user inbox items"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn api_inbox_restore_missing_item_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_state(dir.path().to_path_buf());

        let err = api_inbox_restore(Path("does-not-exist".to_string()), State(state))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
    }
}

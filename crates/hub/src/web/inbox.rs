//! Inbox attachment bytes: file routes that answer whether or not the agent
//! is running. A user-inbox item's files are at
//! `/api/inbox/{id}/attachments/{index}`, an agent-inbox item's at
//! `/api/inbox/agent/{id}/attachments/{index}`, active or archived (see
//! [`crate::mail::attachment_url`]).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Json, Response};

use residuum_config::WorkspaceLayout;
use residuum_net::browser_landing::{self, AppNotice, Fallback};
use residuum_workspace::inbox::{InboxKind, InboxRef};

use super::AgentFilesState;

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

/// `GET /api/agents/{name}/inbox/{id}/attachments/{index}` — Serve one of a
/// user-inbox item's attachments by its position in the item's attachment
/// list.
pub(super) async fn api_inbox_attachment(
    method: Method,
    request_headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    State(state): State<AgentFilesState>,
) -> Response {
    serve_attachment(
        &state,
        InboxKind::User,
        &id,
        index,
        &method,
        &request_headers,
    )
    .await
}

/// `GET /api/agents/{name}/inbox/agent/{id}/attachments/{index}` — Serve one
/// of an agent-inbox item's attachments, such as a file the user sent the
/// agent.
pub(super) async fn api_agent_inbox_attachment(
    method: Method,
    request_headers: HeaderMap,
    Path((id, index)): Path<(String, usize)>,
    State(state): State<AgentFilesState>,
) -> Response {
    serve_attachment(
        &state,
        InboxKind::Agent,
        &id,
        index,
        &method,
        &request_headers,
    )
    .await
}

/// Serve attachment `index` of the `kind` inbox item `id`.
///
/// Checks the active inbox first, then the archive, so a link handed out before
/// an item is archived keeps working afterward. Never trusts the item's stored
/// attachment path as a literal filesystem path — see `confine`.
async fn serve_attachment(
    state: &AgentFilesState,
    kind: InboxKind,
    id: &str,
    index: usize,
    method: &Method,
    request_headers: &HeaderMap,
) -> Response {
    let Some((resolved, file_name)) = find_inbox_attachment(state, kind, id, index).await else {
        tracing::debug!(id = %id, kind = %kind, index, "inbox attachment requested that no longer exists");
        return missing_attachment(state, kind, id, method, request_headers);
    };
    let Ok(bytes) = tokio::fs::read(&resolved).await else {
        tracing::warn!(id = %id, kind = %kind, index, path = %resolved.display(), "inbox attachment not readable");
        return missing_attachment(state, kind, id, method, request_headers);
    };

    let mime_type = residuum_bus::attachment::detect_mime_type(&resolved);
    crate::file_server::respond_with_file(bytes, &mime_type, &file_name)
}

/// The answer for an attachment that is gone: a browser that opened the link
/// goes to the item's thread in the Inbox, which says so; anything else gets a
/// JSON error.
fn missing_attachment(
    state: &AgentFilesState,
    kind: InboxKind,
    id: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    const MESSAGE: &str =
        "This attachment is no longer available. It may have been removed from its inbox item.";
    if browser_landing::is_top_level_navigation(method, headers) {
        let notice = ("notice", AppNotice::InboxAttachmentMissing.code());
        // An id no ref can carry names no thread; the Inbox still shows the
        // notice.
        let target = match InboxRef::new(&state.agent_name, kind, id) {
            Ok(item) => {
                browser_landing::app_path("/inbox", &[("thread", &item.to_string()), notice])
            }
            Err(_) => browser_landing::app_path("/inbox", &[notice]),
        };
        return browser_landing::app_redirect(
            &target,
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

/// The file of attachment `index` of the `kind` inbox item `id` (active or
/// archived), confined to the item's attachment directory, and its file name.
async fn find_inbox_attachment(
    state: &AgentFilesState,
    kind: InboxKind,
    id: &str,
    index: usize,
) -> Option<(std::path::PathBuf, String)> {
    // An id no ref can carry (a path separator, `..`) names no item, so it is
    // never joined into a path.
    let item_ref = InboxRef::new(&state.agent_name, kind, id).ok()?;
    let layout = WorkspaceLayout::new(&state.workspace_dir);
    let found = match residuum_workspace::inbox::locate_item(&layout, &item_ref).await {
        Ok(found) => found?,
        Err(e) => {
            tracing::warn!(item = %item_ref, error = %e, "couldn't read the inbox item an attachment was requested from");
            return None;
        }
    };

    let file_name = found.item.attachments.get(index)?.file_name()?.to_owned();
    let Some(resolved) = confine(&found.attachments_root, id, &file_name).await else {
        tracing::warn!(id = %id, kind = %kind, index, "rejected inbox attachment request outside item directory");
        return None;
    };
    Some((resolved, file_name.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use residuum_workspace::inbox::InboxItem;

    use super::*;

    fn make_state(workspace_dir: std::path::PathBuf) -> AgentFilesState {
        AgentFilesState::for_test(workspace_dir, None)
    }

    /// Write the `kind` item `id`, active or archived, with `attachments`.
    async fn write_item(
        layout: &WorkspaceLayout,
        kind: InboxKind,
        archived: bool,
        id: &str,
        attachments: Vec<std::path::PathBuf>,
    ) {
        write_item_at(&kind.items_dir(layout, archived), id, attachments).await;
    }

    /// Write item `id` with `attachments` into `dir`.
    async fn write_item_at(dir: &std::path::Path, id: &str, attachments: Vec<std::path::PathBuf>) {
        tokio::fs::create_dir_all(dir).await.unwrap();
        let item = InboxItem {
            title: "test item".to_string(),
            body: "body".to_string(),
            source: "web".to_string(),
            timestamp: residuum_core::time::now_local(chrono_tz::UTC),
            read: false,
            attachments,
            in_reply_to: None,
        };
        residuum_workspace::inbox::save_item(dir, &format!("{id}.json"), &item)
            .await
            .unwrap();
    }

    /// Put `bytes` at `note.txt` in the `kind` item `id`'s attachment
    /// directory, and return the path the item records.
    async fn write_attachment(
        layout: &WorkspaceLayout,
        kind: InboxKind,
        archived: bool,
        id: &str,
        bytes: &[u8],
    ) -> std::path::PathBuf {
        let dir = kind.attachments_dir(layout, archived).join(id);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("note.txt"), bytes).await.unwrap();
        kind.attachments_dir(&WorkspaceLayout::new(""), archived)
            .join(id)
            .join("note.txt")
    }

    async fn fetch(state: AgentFilesState, kind: InboxKind, id: &str, index: usize) -> Response {
        let path = Path((id.to_string(), index));
        match kind {
            InboxKind::User => {
                api_inbox_attachment(Method::GET, HeaderMap::new(), path, State(state)).await
            }
            InboxKind::Agent => {
                api_agent_inbox_attachment(Method::GET, HeaderMap::new(), path, State(state)).await
            }
        }
    }

    async fn body_of(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn both_inboxes_serve_their_items_files_active_and_archived() {
        for kind in [InboxKind::User, InboxKind::Agent] {
            for archived in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let layout = WorkspaceLayout::new(dir.path());
                let stored = write_attachment(&layout, kind, archived, "item1", b"hello").await;
                write_item(&layout, kind, archived, "item1", vec![stored]).await;

                let response = fetch(make_state(dir.path().to_path_buf()), kind, "item1", 0).await;

                assert_eq!(response.status(), StatusCode::OK, "{kind} {archived}");
                assert_eq!(body_of(response).await, b"hello", "{kind} {archived}");
            }
        }
    }

    #[tokio::test]
    async fn each_route_reads_only_its_own_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let stored = write_attachment(&layout, InboxKind::Agent, false, "item1", b"hi").await;
        write_item(&layout, InboxKind::Agent, false, "item1", vec![stored]).await;

        let response = fetch(
            make_state(dir.path().to_path_buf()),
            InboxKind::User,
            "item1",
            0,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_id_no_ref_can_carry_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        // An item one directory above the inbox, which `../planted` would
        // reach if the id were joined into a path unchecked.
        let planted = InboxKind::User.items_dir(&layout, false).join("..");
        write_item_at(&planted, "planted", vec![]).await;

        for id in ["../planted", "a/b", ""] {
            let response =
                fetch(make_state(dir.path().to_path_buf()), InboxKind::User, id, 0).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{id:?}");
        }
    }

    #[tokio::test]
    async fn an_out_of_range_index_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        write_item(&layout, InboxKind::User, false, "item1", vec![]).await;

        let response = fetch(
            make_state(dir.path().to_path_buf()),
            InboxKind::User,
            "item1",
            0,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_item_is_not_found_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let response = fetch(
            make_state(dir.path().to_path_buf()),
            InboxKind::User,
            "does-not-exist",
            0,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = serde_json::from_slice(&body_of(response).await).unwrap();
        assert!(
            body.get("error")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|e| e.contains("no longer available")),
            "a fetch is told why, in the API's error shape: {body}"
        );
    }

    #[tokio::test]
    async fn a_tab_opening_a_missing_attachment_goes_to_its_thread() {
        for (kind, handler_kind) in [("user", InboxKind::User), ("agent", InboxKind::Agent)] {
            let dir = tempfile::tempdir().unwrap();
            let state = make_state(dir.path().to_path_buf());
            let mut headers = HeaderMap::new();
            headers.insert("sec-fetch-dest", "document".parse().unwrap());
            headers.insert("sec-fetch-mode", "navigate".parse().unwrap());
            let path = Path(("item1".to_string(), 2));
            let response = match handler_kind {
                InboxKind::User => {
                    api_inbox_attachment(Method::GET, headers, path, State(state)).await
                }
                InboxKind::Agent => {
                    api_agent_inbox_attachment(Method::GET, headers, path, State(state)).await
                }
            };

            assert_eq!(response.status(), StatusCode::SEE_OTHER, "redirected");
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .unwrap(),
                format!(
                    "/inbox?thread=test-agent%2F{kind}%2Fitem1&notice=inbox-attachment-missing"
                )
                .as_str(),
                "to the item's thread, with the notice"
            );
        }
    }

    #[tokio::test]
    async fn a_file_missing_on_disk_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        // The item's own directory exists (so `confine` gets past the first
        // canonicalize), but the file it names was never actually copied there.
        tokio::fs::create_dir_all(layout.user_inbox_attachments_dir().join("item1"))
            .await
            .unwrap();
        write_item(
            &layout,
            InboxKind::User,
            false,
            "item1",
            vec![std::path::PathBuf::from(
                "inbox/user/attachments/item1/never_copied.txt",
            )],
        )
        .await;

        let response = fetch(
            make_state(dir.path().to_path_buf()),
            InboxKind::User,
            "item1",
            0,
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_escaping_the_item_directory_is_refused() {
        // The stored filename itself can never contain a traversal component
        // (`Path::file_name` already strips those), so the realistic way a
        // confined-looking path can still resolve outside the item directory is
        // via a symlink planted inside it. `confine`'s canonicalize-then-`starts_with`
        // check must catch that.
        for kind in [InboxKind::User, InboxKind::Agent] {
            let dir = tempfile::tempdir().unwrap();
            let layout = WorkspaceLayout::new(dir.path());

            let outside = dir.path().join("outside.txt");
            tokio::fs::write(&outside, b"secret").await.unwrap();

            let item_dir = kind.attachments_dir(&layout, false).join("item1");
            tokio::fs::create_dir_all(&item_dir).await.unwrap();
            std::os::unix::fs::symlink(&outside, item_dir.join("note.txt")).unwrap();
            let stored = kind
                .attachments_dir(&WorkspaceLayout::new(""), false)
                .join("item1/note.txt");
            write_item(&layout, kind, false, "item1", vec![stored]).await;

            let response = fetch(make_state(dir.path().to_path_buf()), kind, "item1", 0).await;

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{kind}");
        }
    }
}

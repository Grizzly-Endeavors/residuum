//! Inbox API endpoints: the user inbox (read-only listing plus per-item
//! actions) and the agent inbox's one write endpoint for workbench
//! artifacts.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use crate::gateway::types::GatewayState;
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

/// Build the agent-inbox API router (`POST /api/agent-inbox`).
///
/// A workbench artifact's only way to hand the agent something to triage
/// later — there is no equivalent write endpoint for the user inbox, which
/// only the agent's `user_inbox_add` tool may populate.
pub(crate) fn agent_inbox_api_router(state: GatewayState) -> axum::Router {
    axum::Router::new()
        .route("/api/agent-inbox", axum::routing::post(api_agent_inbox_add))
        .with_state(state)
}

/// Request body for `POST /api/agent-inbox`.
#[derive(Debug, Deserialize)]
pub(super) struct AgentInboxAddRequest {
    /// Defaults to the body's first line, in full, when absent or blank.
    #[serde(default)]
    pub title: Option<String>,
    pub body: String,
}

/// Response body for `POST /api/agent-inbox`.
#[derive(Debug, Serialize)]
pub(super) struct AgentInboxAddResponse {
    /// The new item's ID (its filename stem), as used by `inbox_read` and
    /// `inbox_archive`.
    pub id: String,
}

/// `POST /api/agent-inbox` — add an item to the agent's inbox, the same
/// place the WS `/inbox` command and the notification router's `inbox`
/// target write to. The source is `artifact:<name>` when the request carries
/// the artifact identity header (set by the workbench bridge), `"web"`
/// otherwise.
///
/// # Errors
/// `400` for a blank body or an invalid identity header, `500` if the item
/// can't be saved.
pub(super) async fn api_agent_inbox_add(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AgentInboxAddRequest>,
) -> Result<Json<AgentInboxAddResponse>, (StatusCode, String)> {
    if req.body.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "body must not be blank".to_string(),
        ));
    }

    let title = req
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| crate::inbox::derive_title(&req.body));
    let source = match super::artifact_identity::artifact_identity(&headers) {
        Ok(Some(name)) => format!("artifact:{name}"),
        Ok(None) => "web".to_string(),
        Err(message) => return Err((StatusCode::BAD_REQUEST, message)),
    };

    let filename = crate::inbox::quick_add(
        &state.agent_inbox_dir,
        &title,
        &req.body,
        &source,
        state.tz,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, source = %source, "failed to add agent inbox item via http");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("couldn't save the inbox item: {e}"),
        )
    })?;

    let id = filename.trim_end_matches(".json").to_string();
    Ok(Json(AgentInboxAddResponse { id }))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
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
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    /// A minimal but real `GatewayState`, for exercising the agent-inbox
    /// handler directly rather than through the full HTTP stack.
    fn make_gateway_state(workspace_dir: &std::path::Path) -> GatewayState {
        let (core, _receivers) =
            crate::gateway::types::GatewayCore::new(workspace_dir.to_path_buf());
        let (_tunnel_tx, tunnel_status_rx) =
            tokio::sync::watch::channel(crate::tunnel::TunnelStatus::Disconnected);
        let session_registry =
            std::sync::Arc::new(crate::background::registry::SessionRegistry::new());
        let session_store = std::sync::Arc::new(crate::background::store::SessionStore::new(
            workspace_dir.join("sessions"),
        ));
        let agent_messenger =
            std::sync::Arc::new(crate::background::messaging::AgentMessenger::new(
                std::sync::Arc::clone(&session_registry),
                core.publisher.clone(),
                std::sync::Arc::clone(&session_store),
                crate::agent::hop::HopLimits { soft: 8, hard: 32 },
            ));

        let agent_inbox_dir = workspace_dir.join("inbox/agent");
        std::fs::create_dir_all(&agent_inbox_dir).unwrap();

        GatewayState {
            reload_tx: core.reload_tx,
            command_tx: core.command_tx,
            stop_tx: core.stop_tx,
            agent_inbox_dir,
            tz: chrono_tz::UTC,
            tunnel_status_rx,
            publisher: core.publisher,
            bus_handle: core.bus_handle,
            file_registry: crate::gateway::file_server::FileRegistry::new(),
            webhooks: crate::interfaces::webhook::WebhookTable::default(),
            session_registry,
            session_store,
            agent_messenger,
            skill_state: crate::skills::SkillState::new_shared(
                crate::skills::SkillIndex::default(),
                vec![],
            ),
            workspace_watch_health: tokio::sync::watch::channel(
                crate::workspace::watch::WatchHealth::Native,
            )
            .1,
            action_store: std::sync::Arc::new(tokio::sync::Mutex::new(
                crate::actions::store::ActionStore::new_empty(
                    workspace_dir.join("scheduled_actions.json"),
                ),
            )),
            layout: crate::workspace::layout::WorkspaceLayout::new(workspace_dir),
        }
    }

    #[tokio::test]
    async fn agent_inbox_add_defaults_title_and_web_source() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let response = api_agent_inbox_add(
            State(state.clone()),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: None,
                body: "First line of the item\nmore detail".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = crate::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, response.0.id);
        assert_eq!(items[0].1.title, "First line of the item");
        assert_eq!(items[0].1.source, "web");
    }

    #[tokio::test]
    async fn agent_inbox_add_uses_given_title() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let _response = api_agent_inbox_add(
            State(state.clone()),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: Some("Custom title".to_string()),
                body: "body text".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = crate::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items[0].1.title, "Custom title");
    }

    #[tokio::test]
    async fn agent_inbox_add_attributes_artifact_source() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());
        let mut headers = HeaderMap::new();
        headers.insert(
            super::super::artifact_identity::ARTIFACT_HEADER,
            "pricing-explorer".parse().unwrap(),
        );

        let _response = api_agent_inbox_add(
            State(state.clone()),
            headers,
            Json(AgentInboxAddRequest {
                title: None,
                body: "body text".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = crate::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items[0].1.source, "artifact:pricing-explorer");
    }

    #[tokio::test]
    async fn agent_inbox_add_rejects_blank_body() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let err = api_agent_inbox_add(
            State(state),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: None,
                body: "   \n  ".to_string(),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
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

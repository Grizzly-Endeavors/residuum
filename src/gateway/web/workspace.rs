//! Workspace file browser API endpoints.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use crate::gateway::ReloadSignal;
use crate::pulse::types::HeartbeatConfig;
use crate::workspace::access::is_blocked_path;
use crate::workspace::version::{modified_unix_ms, version_token};

use super::ConfigApiState;

/// Maximum size for a workspace text read or write, in bytes (8 MiB). The
/// `PUT /api/workspace/file` route's request body limit is raised to match,
/// so an over-limit write is refused before the handler even runs.
pub(crate) const TEXT_FILE_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// A single entry in a workspace directory listing.
#[derive(Serialize)]
pub(super) struct WorkspaceEntry {
    pub name: String,
    pub entry_type: String,
    pub size: Option<u64>,
    pub modified: u64,
    pub version: String,
}

/// Query parameters for `GET /api/workspace/files` (directory listing).
#[derive(Deserialize)]
pub(super) struct FilesQuery {
    pub path: Option<String>,
}

/// Query parameters for `GET /api/workspace/file` (single file read).
#[derive(Deserialize)]
pub(super) struct FileQuery {
    pub path: String,
}

/// Request body for `PUT /api/workspace/file`.
#[derive(Deserialize)]
pub(super) struct WriteFileRequest {
    pub path: String,
    pub content: String,
}

/// Response from `PUT /api/workspace/file`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct WriteResponse {
    pub saved: bool,
    pub version: String,
}

/// Body for a `412 Precondition Failed` response from a conditional write.
#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ConditionalWriteError {
    error: String,
    current_version: Option<String>,
}

/// Canonicalize the workspace root directory.
pub(super) async fn canonicalize_workspace_root(
    workspace_dir: &Path,
) -> Result<PathBuf, (StatusCode, String)> {
    tokio::fs::canonicalize(workspace_dir).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to resolve workspace directory: {e}"),
        )
    })
}

/// Resolve and validate a path relative to the workspace directory for
/// reading.
///
/// Canonicalizes both the workspace root and the joined path, then verifies
/// the result is still inside the workspace. Returns 403 if the path
/// escapes the workspace boundary, 404 if it doesn't exist.
pub(super) async fn validate_workspace_path(
    workspace_dir: &Path,
    relative: &str,
) -> Result<PathBuf, (StatusCode, String)> {
    let canonical_root = canonicalize_workspace_root(workspace_dir).await?;

    let target = workspace_dir.join(relative);
    let canonical_target = tokio::fs::canonicalize(&target).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (StatusCode::NOT_FOUND, format!("path not found: {relative}"))
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to resolve path: {e}"),
            )
        }
    })?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("path traversal rejected: {relative}"),
        ));
    }

    Ok(canonical_target)
}

/// Resolve a path relative to the workspace directory for writing.
///
/// Unlike `validate_workspace_path`, the target need not exist yet, and
/// neither do its parent directories: this walks up from the target's
/// parent to the nearest ancestor that does exist, canonicalizes *that*,
/// and re-appends the missing segments. That keeps the escape check intact
/// — canonicalization always happens on a path segment that is actually on
/// disk, so a symlink planted inside the workspace can't walk a
/// still-nonexistent path outside it — while letting the caller create the
/// missing parent directories afterward.
async fn resolve_workspace_path_for_write(
    workspace_dir: &Path,
    relative: &str,
) -> Result<PathBuf, (StatusCode, String)> {
    let canonical_root = canonicalize_workspace_root(workspace_dir).await?;

    let target = workspace_dir.join(relative);
    let file_name = target.file_name().map(OsString::from).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no file name: {relative}"),
        )
    })?;
    let parent = target.parent().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no parent directory: {relative}"),
        )
    })?;

    let (canonical_existing, missing_segments) =
        nearest_existing_ancestor(parent, relative).await?;

    if !canonical_existing.starts_with(&canonical_root) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("path traversal rejected: {relative}"),
        ));
    }

    let mut resolved = canonical_existing;
    for segment in missing_segments.into_iter().rev() {
        resolved.push(segment);
    }
    resolved.push(file_name);

    Ok(resolved)
}

/// Walk up from `start` until an existing directory is found, returning its
/// canonical path and the segment names that don't exist yet (nearest
/// first).
async fn nearest_existing_ancestor(
    start: &Path,
    relative: &str,
) -> Result<(PathBuf, Vec<OsString>), (StatusCode, String)> {
    let mut existing = start.to_path_buf();
    let mut missing = Vec::new();

    loop {
        match tokio::fs::metadata(&existing).await {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("a parent of {relative} exists but is not a directory"),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(OsString::from) else {
                    return Err((
                        StatusCode::FORBIDDEN,
                        format!("path traversal rejected: {relative}"),
                    ));
                };
                missing.push(name);
                if !existing.pop() {
                    return Err((
                        StatusCode::FORBIDDEN,
                        format!("path traversal rejected: {relative}"),
                    ));
                }
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to resolve path: {e}"),
                ));
            }
        }
    }

    let canonical = tokio::fs::canonicalize(&existing).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to resolve path: {e}"),
        )
    })?;

    Ok((canonical, missing))
}

/// Returns true if the path refers to a workspace identity file.
///
/// Checks the file name only (ignores any leading directory components) so that
/// identity files nested inside subdirectories are also recognised.
fn is_identity_file(relative: &str) -> bool {
    const IDENTITY_FILES: &[&str] = &["SOUL.md", "AGENTS.md", "USER.md", "HEARTBEAT.yml"];

    let file_name = Path::new(relative)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    IDENTITY_FILES.contains(&file_name)
}

/// Returns true if the path refers to the pulse scheduler's `HEARTBEAT.yml`.
///
/// Checks the file name only (ignoring leading directory components), matching
/// how `is_identity_file` recognises identity files.
fn is_heartbeat_file(relative: &str) -> bool {
    Path::new(relative)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name == "HEARTBEAT.yml")
}

/// `GET /api/workspace/files` — list directory contents inside the workspace.
///
/// Defaults to the workspace root when no `path` query parameter is provided.
/// Directories are sorted before files; entries within each group are sorted
/// alphabetically. Blocked paths (internal databases, index directories) are
/// excluded from the listing.
pub(super) async fn api_workspace_files(
    Query(query): Query<FilesQuery>,
    State(state): State<ConfigApiState>,
) -> Result<Json<Vec<WorkspaceEntry>>, (StatusCode, String)> {
    let relative = query.path.unwrap_or_default();

    if is_blocked_path(&relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let dir_path = if relative.is_empty() {
        state.workspace_dir.clone()
    } else {
        validate_workspace_path(&state.workspace_dir, &relative).await?
    };

    let mut read_dir = tokio::fs::read_dir(&dir_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read directory: {e}"),
        )
    })?;

    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read directory entry: {e}"),
        )
    })? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_relative = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };

        if is_blocked_path(&entry_relative) {
            continue;
        }

        if let Some(item) = workspace_entry(name, &entry).await? {
            entries.push(item);
        }
    }

    // Sort: directories first, then alphabetically within each group.
    entries.sort_by(|a, b| {
        let a_is_dir = a.entry_type == "directory";
        let b_is_dir = b.entry_type == "directory";
        b_is_dir.cmp(&a_is_dir).then_with(|| a.name.cmp(&b.name))
    });

    Ok(Json(entries))
}

/// Build a `WorkspaceEntry` for one directory entry.
///
/// Returns `Ok(None)` if the entry disappeared between listing the
/// directory and statting it — a benign race with a concurrent delete, not
/// an error the caller should surface.
async fn workspace_entry(
    name: String,
    entry: &tokio::fs::DirEntry,
) -> Result<Option<WorkspaceEntry>, (StatusCode, String)> {
    let metadata = match entry.metadata().await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read metadata for {name}: {e}"),
            ));
        }
    };

    let (entry_type, size) = if metadata.is_dir() {
        ("directory".to_string(), None)
    } else {
        ("file".to_string(), Some(metadata.len()))
    };

    Ok(Some(WorkspaceEntry {
        name,
        entry_type,
        size,
        modified: modified_unix_ms(&metadata),
        version: version_token(&metadata),
    }))
}

/// `GET /api/workspace/file` — read a workspace file as plain text.
///
/// Returns 404 if the file does not exist, 403 if the path is blocked, 413
/// if the file exceeds the 8 MiB text limit, and 415 if the file is not
/// valid UTF-8 (pointing the caller at the raw endpoint instead). On
/// success the response carries an `ETag` header with the file's version.
pub(super) async fn api_workspace_file_read(
    Query(query): Query<FileQuery>,
    State(state): State<ConfigApiState>,
) -> Result<Response, (StatusCode, String)> {
    let relative = &query.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let path = validate_workspace_path(&state.workspace_dir, relative).await?;

    let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (StatusCode::NOT_FOUND, format!("file not found: {relative}"))
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat file: {e}"),
            )
        }
    })?;

    if metadata.len() > TEXT_FILE_LIMIT_BYTES as u64 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "file exceeds the {TEXT_FILE_LIMIT_BYTES}-byte text limit ({} bytes)",
                metadata.len()
            ),
        ));
    }

    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read file: {e}"),
        )
    })?;

    let content = String::from_utf8(bytes).map_err(|utf8_err| {
        tracing::debug!(
            path = %relative,
            error = %utf8_err,
            "workspace file read rejected: not valid utf-8"
        );
        (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!(
                "{relative} is not valid UTF-8 text; read it from \
                 /api/workspace/raw?path={relative} instead"
            ),
        )
    })?;

    let version = version_token(&metadata);
    let mut response = content.into_response();
    if let Ok(header_value) = HeaderValue::from_str(&version) {
        response.headers_mut().insert(header::ETAG, header_value);
    }

    Ok(response)
}

/// A header's value as `&str`, `None` if absent or not valid UTF-8.
fn header_str(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

/// Build a `412 Precondition Failed` response with the conditional-write
/// error body: `{ "error": "...", "current_version": "<version>" | null }`.
fn precondition_failed(error: String, current_version: Option<String>) -> Response {
    (
        StatusCode::PRECONDITION_FAILED,
        Json(ConditionalWriteError {
            error,
            current_version,
        }),
    )
        .into_response()
}

/// Check `If-Match`/`If-None-Match` against the file currently at
/// `target_path`. Returns `Ok(Some(response))` with a `412` when the
/// precondition fails, `Ok(None)` when the write may proceed (including
/// when neither header is present, which is unconditional).
async fn check_conditional_write(
    target_path: &Path,
    headers: &HeaderMap,
) -> Result<Option<Response>, (StatusCode, String)> {
    let if_match = header_str(headers, header::IF_MATCH);
    let if_none_match_star = header_str(headers, header::IF_NONE_MATCH) == Some("*");

    if if_match.is_none() && !if_none_match_star {
        return Ok(None);
    }

    let current_version = match tokio::fs::metadata(target_path).await {
        Ok(meta) => Some(version_token(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat file: {e}"),
            ));
        }
    };

    if if_none_match_star && current_version.is_some() {
        return Ok(Some(precondition_failed(
            "file already exists".to_string(),
            current_version,
        )));
    }

    if let Some(expected) = if_match
        && current_version.as_deref() != Some(expected)
    {
        return Ok(Some(precondition_failed(
            "file has changed since it was last read".to_string(),
            current_version,
        )));
    }

    Ok(None)
}

/// `PUT /api/workspace/file` — write content to a workspace file.
///
/// Creates the file and any missing parent directories inside the
/// workspace. Writes are atomic (temp file in the target directory, then
/// rename) and capped at 8 MiB. `If-Match: <version>` and
/// `If-None-Match: *` are honored as conditional-write preconditions,
/// answering `412` on a mismatch; without either header the write is
/// unconditional. Returns `{ saved: true, version }` on success.
///
/// If the written file is a workspace identity file and a reload channel is
/// available, sends a `Workspace` reload signal.
///
/// Writes to `HEARTBEAT.yml` are validated as parseable `HeartbeatConfig`
/// YAML before being accepted: the pulse scheduler hot-reloads this file on
/// every tick, so an unvalidated write that saves broken YAML would
/// silently stop every scheduled pulse from firing while reporting success
/// to the caller.
pub(super) async fn api_workspace_file_write(
    State(state): State<ConfigApiState>,
    headers: HeaderMap,
    Json(req): Json<WriteFileRequest>,
) -> Result<Response, (StatusCode, String)> {
    let relative = &req.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    if req.content.len() > TEXT_FILE_LIMIT_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "content exceeds the {TEXT_FILE_LIMIT_BYTES}-byte text limit ({} bytes)",
                req.content.len()
            ),
        ));
    }

    if is_heartbeat_file(relative)
        && let Err(e) = serde_yaml_ng::from_str::<HeartbeatConfig>(&req.content)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "invalid HEARTBEAT.yml: {e}; fix the yaml before saving — this file is \
                 hot-reloaded by the pulse scheduler, so invalid content would silently \
                 stop all scheduled pulses from firing"
            ),
        ));
    }

    let target_path = resolve_workspace_path_for_write(&state.workspace_dir, relative).await?;

    if let Some(conflict) = check_conditional_write(&target_path, &headers).await? {
        return Ok(conflict);
    }

    let parent = target_path.parent().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no parent directory: {relative}"),
        )
    })?;
    tokio::fs::create_dir_all(parent).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create parent directory: {e}"),
        )
    })?;

    crate::util::fs::atomic_write(&target_path, &req.content)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to write file: {e}"),
            )
        })?;

    let metadata = tokio::fs::metadata(&target_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to stat file after write: {e}"),
        )
    })?;
    let version = version_token(&metadata);

    if is_identity_file(relative)
        && let Some(tx) = &state.reload_tx
    {
        // Best-effort: receiver may have been dropped during shutdown.
        drop(tx.send(ReloadSignal::Workspace));
    }

    Ok(Json(WriteResponse {
        saved: true,
        version,
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_files() {
        assert!(is_identity_file("SOUL.md"));
        assert!(is_identity_file("AGENTS.md"));
        assert!(is_identity_file("subdir/SOUL.md"));
        assert!(!is_identity_file("skills/research.md"));
        assert!(!is_identity_file("random.txt"));
    }

    #[test]
    fn heartbeat_file_recognised() {
        assert!(is_heartbeat_file("HEARTBEAT.yml"));
        assert!(is_heartbeat_file("subdir/HEARTBEAT.yml"));
        assert!(!is_heartbeat_file("SOUL.md"));
        assert!(!is_heartbeat_file("not_HEARTBEAT.yml.txt"));
    }

    #[tokio::test]
    async fn path_traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            validate_workspace_path(dir.path(), "../etc/passwd")
                .await
                .is_err()
        );
        assert!(
            validate_workspace_path(dir.path(), "/etc/passwd")
                .await
                .is_err()
        );
        assert!(
            validate_workspace_path(dir.path(), "foo/../../etc/passwd")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn path_traversal_for_write_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();

        assert!(
            resolve_workspace_path_for_write(&ws_dir, "../etc/shadow")
                .await
                .is_err(),
            "escaping via a relative .. should be rejected"
        );
        assert!(
            resolve_workspace_path_for_write(&ws_dir, "/etc/shadow")
                .await
                .is_err(),
            "an absolute path should be rejected"
        );
    }

    #[tokio::test]
    async fn valid_paths_accepted() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("test.md"), "hello")
            .await
            .unwrap();
        let result = validate_workspace_path(dir.path(), "test.md").await;
        assert!(result.is_ok());
    }

    fn make_state(ws_dir: PathBuf) -> ConfigApiState {
        super::super::ConfigApiState {
            config_dir: ws_dir.clone(),
            workspace_dir: ws_dir,
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[tokio::test]
    async fn workspace_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("SOUL.md"), "# Soul")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "some notes")
            .await
            .unwrap();
        tokio::fs::create_dir(ws_dir.join("skills")).await.unwrap();
        tokio::fs::write(ws_dir.join("skills").join("research.md"), "skill content")
            .await
            .unwrap();
        // Create a blocked file to verify filtering
        tokio::fs::write(ws_dir.join("vectors.db"), "binary data")
            .await
            .unwrap();

        let state = make_state(ws_dir.clone());

        // List root
        let entries = api_workspace_files(Query(FilesQuery { path: None }), State(state.clone()))
            .await
            .unwrap();
        let names: Vec<&str> = entries.0.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"SOUL.md"));
        assert!(names.contains(&"notes.md"));
        assert!(names.contains(&"skills"));
        assert!(!names.contains(&"vectors.db"));
        for entry in &entries.0 {
            assert!(
                !entry.version.is_empty(),
                "every entry should carry a version"
            );
        }

        // Read a file
        let response = api_workspace_file_read(
            Query(FileQuery {
                path: "SOUL.md".to_string(),
            }),
            State(state.clone()),
        )
        .await
        .unwrap();
        assert!(response.headers().get(header::ETAG).is_some());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"# Soul");

        // Write a file
        let write_response = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "SOUL.md".to_string(),
                content: "# Updated Soul".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(write_response.status(), StatusCode::OK);
        let write_body = axum::body::to_bytes(write_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let write_result: WriteResponse = serde_json::from_slice(&write_body).unwrap();
        assert!(write_result.saved);
        assert!(!write_result.version.is_empty());

        // Verify write persisted
        let updated_content = tokio::fs::read_to_string(ws_dir.join("SOUL.md"))
            .await
            .unwrap();
        assert_eq!(updated_content, "# Updated Soul");

        // List subdirectory
        let subdir_entries = api_workspace_files(
            Query(FilesQuery {
                path: Some("skills".to_string()),
            }),
            State(state.clone()),
        )
        .await
        .unwrap();
        assert_eq!(subdir_entries.0.len(), 1);
        assert_eq!(
            subdir_entries.0.first().map(|e| e.name.as_str()),
            Some("research.md")
        );
    }

    #[tokio::test]
    async fn workspace_file_write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        // Write a new file that does not exist yet
        let response = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "new_file.md".to_string(),
                content: "# New File".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let content = tokio::fs::read_to_string(ws_dir.join("new_file.md"))
            .await
            .unwrap();
        assert_eq!(content, "# New File");

        // Path traversal via write for new file should be rejected
        let traversal_result = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "../etc/shadow".to_string(),
                content: "hacked".to_string(),
            }),
        )
        .await;
        assert!(
            traversal_result.is_err(),
            "path traversal should be rejected for new file write"
        );
    }

    #[tokio::test]
    async fn workspace_file_write_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_file_write(
            State(state),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "wiki/pages/new-topic.md".to_string(),
                content: "# New Topic".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let content = tokio::fs::read_to_string(ws_dir.join("wiki/pages/new-topic.md"))
            .await
            .unwrap();
        assert_eq!(content, "# New Topic");
    }

    #[tokio::test]
    async fn workspace_file_read_rejects_non_utf8_with_415() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("binary.dat"), [0xff, 0xfe, 0x00, 0xff])
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_file_read(
            Query(FileQuery {
                path: "binary.dat".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(
            err.1.contains("/api/workspace/raw"),
            "415 message should point to the raw endpoint: {}",
            err.1
        );
    }

    #[tokio::test]
    async fn workspace_file_roundtrips_a_three_mebibyte_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        let content = "a".repeat(3 * 1024 * 1024);
        let response = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "big.md".to_string(),
                content: content.clone(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let read_response = api_workspace_file_read(
            Query(FileQuery {
                path: "big.md".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(read_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.len(), content.len());
    }

    #[tokio::test]
    async fn workspace_file_write_over_limit_answers_413() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let content = "a".repeat(TEXT_FILE_LIMIT_BYTES + 1);
        let err = api_workspace_file_write(
            State(state),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "too_big.md".to_string(),
                content,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn conditional_write_if_match_stale_answers_412_with_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "original")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_static("stale-version"));

        let response = api_workspace_file_write(
            State(state),
            headers,
            Json(WriteFileRequest {
                path: "notes.md".to_string(),
                content: "changed".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ConditionalWriteError = serde_json::from_slice(&body).unwrap();
        assert!(parsed.current_version.is_some());
    }

    #[tokio::test]
    async fn conditional_write_if_match_matching_version_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "original")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let metadata = tokio::fs::metadata(ws_dir.join("notes.md")).await.unwrap();
        let current = version_token(&metadata);

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_str(&current).unwrap());

        let response = api_workspace_file_write(
            State(state),
            headers,
            Json(WriteFileRequest {
                path: "notes.md".to_string(),
                content: "changed".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn conditional_write_if_none_match_star_rejects_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "original")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));

        let response = api_workspace_file_write(
            State(state),
            headers,
            Json(WriteFileRequest {
                path: "notes.md".to_string(),
                content: "changed".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn conditional_write_if_none_match_star_allows_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("*"));

        let response = api_workspace_file_write(
            State(state),
            headers,
            Json(WriteFileRequest {
                path: "brand_new.md".to_string(),
                content: "hello".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn identity_file_write_sends_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();

        let (tx, mut rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let state = super::super::ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: ws_dir,
            memory_dir: None,
            reload_tx: Some(tx),
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        };

        api_workspace_file_write(
            State(state),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "SOUL.md".to_string(),
                content: "# hi".to_string(),
            }),
        )
        .await
        .unwrap();

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ReloadSignal::Workspace);
    }

    #[tokio::test]
    async fn workspace_file_write_rejects_invalid_heartbeat_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("HEARTBEAT.yml"), "pulses: []")
            .await
            .unwrap();

        let state = make_state(ws_dir.clone());

        // Malformed YAML must be rejected, not silently accepted with `{"saved": true}` —
        // the pulse scheduler hot-reloads this file every tick, so an unvalidated write
        // that breaks the YAML would silently stop every scheduled pulse from firing.
        let result = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "HEARTBEAT.yml".to_string(),
                content: "not: valid: yaml: [[[".to_string(),
            }),
        )
        .await;
        assert!(
            result.is_err(),
            "invalid HEARTBEAT.yml content should be rejected"
        );
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // The on-disk file must be untouched by the rejected write.
        let unchanged = tokio::fs::read_to_string(ws_dir.join("HEARTBEAT.yml"))
            .await
            .unwrap();
        assert_eq!(
            unchanged, "pulses: []",
            "rejected write should not modify the existing file"
        );

        // Valid YAML should still be accepted.
        let ok_response = api_workspace_file_write(
            State(state),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "HEARTBEAT.yml".to_string(),
                content: "pulses:\n  - name: test\n    schedule: \"1h\"\n    tasks: []\n"
                    .to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(ok_response.status(), StatusCode::OK);
    }
}

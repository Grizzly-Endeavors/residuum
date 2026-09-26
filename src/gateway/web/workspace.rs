//! Workspace file browser API endpoints.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

use crate::gateway::ReloadSignal;
use crate::workspace::access::{dir_holds_internal_data, is_blocked_path};
use crate::workspace::version::{modified_unix_ms, version_token};

use super::ConfigApiState;

/// Maximum size for a workspace text or raw read or write, in bytes (8 MiB).
/// The `PUT /api/workspace/file` and `PUT /api/workspace/raw` routes' request
/// body limits are raised to match, so an over-limit write is refused before
/// the handler even runs.
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

/// Request body for `POST /api/workspace/validate`.
#[derive(Deserialize)]
pub(super) struct ValidateFileRequest {
    pub path: String,
    pub content: String,
}

/// Response from `POST /api/workspace/validate`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct ValidateFileResponse {
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

/// Response from `PUT /api/workspace/file`.
///
/// `diagnostics` is always populated when `path` is one of the
/// strictly-parsed files `crate::diagnostics` understands, whether or not
/// `content` had a problem — empty means clean. The save always succeeds
/// (`saved` is always `true` here; a validation problem is reported, not
/// rejected), matching `write_file`/`edit_file`'s "write, then report" shape.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct WriteResponse {
    pub saved: bool,
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

/// Body for a `412 Precondition Failed` response from a conditional write.
#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ConditionalWriteError {
    error: String,
    current_version: Option<String>,
}

/// Query parameters for `DELETE /api/workspace/file`.
#[derive(Deserialize)]
pub(super) struct DeleteFileQuery {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

/// Response from `DELETE /api/workspace/file`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct DeleteResponse {
    pub deleted: bool,
    /// Checkpoint holding the workspace as it was before this delete.
    /// `None` when that checkpoint could not be recorded; the delete still
    /// succeeded, and the UI should not offer Undo.
    pub checkpoint_id: Option<String>,
}

/// Request body for `POST /api/workspace/dir`.
#[derive(Deserialize)]
pub(super) struct MkdirRequest {
    pub path: String,
}

/// Response from `POST /api/workspace/dir`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct MkdirResponse {
    pub created: bool,
}

/// Request body for `POST /api/workspace/move`.
#[derive(Deserialize)]
pub(super) struct MoveRequest {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// Response from `POST /api/workspace/move`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
pub(super) struct MoveResponse {
    pub moved: bool,
    /// The moved file's new version. `None` for a moved directory, since
    /// version tokens are file-specific.
    pub version: Option<String>,
    /// Diagnostics for the moved file's content at its new name, if `to` is
    /// one of the strictly-parsed files `crate::diagnostics` understands
    /// (most relevantly, a move that lands on `HEARTBEAT.yml`). Empty for a
    /// directory or an unrecognized file — the move always succeeds either way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
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

/// Diagnostics for `bytes` about to be written to workspace-relative
/// `relative`, if it's one of the strictly-parsed files `crate::diagnostics`
/// understands. Never blocks the write — a strictly-parsed file with a
/// problem (invalid `HEARTBEAT.yml`, say) is still saved, and the caller
/// reports these diagnostics alongside the save so the problem is visible
/// rather than accepted silently. Empty means either `relative` isn't one of
/// these files, or its content has nothing to report.
fn diagnose_write_content(
    state: &ConfigApiState,
    relative: &str,
    bytes: &[u8],
) -> Vec<crate::diagnostics::Diagnostic> {
    let path = Path::new(relative);
    let paths = crate::diagnostics::DiagnosticsPaths {
        config_dir: state.config_dir.clone(),
        workspace_dir: state.workspace_dir.clone(),
    };

    let Ok(text) = std::str::from_utf8(bytes) else {
        // Every file this module understands is text (YAML/TOML/JSON/MD), so
        // non-UTF-8 content on a recognized path is itself worth reporting,
        // even though there's no parser to run on bytes that aren't text.
        return if crate::diagnostics::is_recognized(path, &paths) {
            vec![crate::diagnostics::Diagnostic::error(
                "content is not valid UTF-8 text",
            )]
        } else {
            Vec::new()
        };
    };

    crate::diagnostics::diagnose(path, text, &paths).unwrap_or_default()
}

/// `POST /api/workspace/validate` — diagnostics for `content` as if it were
/// saved to `path`, without writing anything.
///
/// `path` is resolved the same way a write would resolve it: against the app
/// config directory for `config.toml`/`providers.toml`, against the
/// workspace root for `config/channels.toml`, `config/mcp.json`,
/// `config/a2a.json`, and by filename alone for `HEARTBEAT.yml` and a skill
/// `SKILL.md`. Empty diagnostics means either the content is clean or
/// `path` isn't one of these files — this endpoint never errors on an
/// unrecognized path, since the editor calls it on every debounced
/// keystroke and most files have nothing to check.
pub(super) async fn api_workspace_validate(
    State(state): State<ConfigApiState>,
    Json(req): Json<ValidateFileRequest>,
) -> Json<ValidateFileResponse> {
    let diagnostics = diagnose_write_content(&state, &req.path, req.content.as_bytes());
    Json(ValidateFileResponse { diagnostics })
}

/// Send the workspace reload signal if `relative` names an identity file.
/// Every write path that can leave an identity file's content changed —
/// text write, raw write, delete, and either side of a move — applies this
/// one function rather than repeating the check.
fn signal_identity_reload(relative: &str, state: &ConfigApiState) {
    if is_identity_file(relative)
        && let Some(tx) = &state.reload_tx
    {
        // Best-effort: receiver may have been dropped during shutdown.
        drop(tx.send(ReloadSignal::Workspace));
    }
}

/// Write `bytes` to workspace-relative `relative`: blocked-path check, the
/// conditional-write precondition, an atomic write that creates missing
/// parent directories, and the identity-file reload signal. Shared by the
/// text and raw write endpoints; size limits are checked by the caller
/// before this runs, since the two endpoints report the limit against a
/// different unit (`content` characters vs. body bytes). Diagnostics for a
/// strictly-parsed file are computed and reported alongside the save, never
/// blocking it — see [`diagnose_write_content`].
async fn write_workspace_bytes(
    state: &ConfigApiState,
    relative: &str,
    bytes: &[u8],
    headers: &HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let diagnostics = diagnose_write_content(state, relative, bytes);

    let target_path = resolve_workspace_path_for_write(&state.workspace_dir, relative).await?;

    if let Some(conflict) = check_conditional_write(&target_path, headers).await? {
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

    crate::util::fs::atomic_write(&target_path, bytes)
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

    signal_identity_reload(relative, state);

    Ok(Json(WriteResponse {
        saved: true,
        version,
        diagnostics,
    })
    .into_response())
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
/// The write always succeeds, even for a strictly-parsed file like
/// `HEARTBEAT.yml` with invalid content — the pulse scheduler simply won't
/// pick up an unparseable `HEARTBEAT.yml` on its next hot-reload. Instead of
/// rejecting the save, the response's `diagnostics` names the problem (see
/// `crate::diagnostics`) so the editor can show it right away.
pub(super) async fn api_workspace_file_write(
    State(state): State<ConfigApiState>,
    headers: HeaderMap,
    Json(req): Json<WriteFileRequest>,
) -> Result<Response, (StatusCode, String)> {
    if req.content.len() > TEXT_FILE_LIMIT_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "content exceeds the {TEXT_FILE_LIMIT_BYTES}-byte text limit ({} bytes)",
                req.content.len()
            ),
        ));
    }

    write_workspace_bytes(&state, &req.path, req.content.as_bytes(), &headers).await
}

/// `GET /api/workspace/raw` — read a workspace file as raw bytes.
///
/// Returns the file's bytes with a `Content-Type` guessed from its
/// extension and an `ETag` header carrying its version. `404` if missing,
/// `403` if the path is blocked, `413` over the 8 MiB limit shared with the
/// text read endpoint.
pub(super) async fn api_workspace_raw_read(
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
                "file exceeds the {TEXT_FILE_LIMIT_BYTES}-byte raw read limit ({} bytes)",
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

    let version = version_token(&metadata);
    let mime_type = crate::interfaces::attachment::detect_mime_type(&path);

    let mut response = bytes.into_response();
    let response_headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime_type) {
        response_headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&version) {
        response_headers.insert(header::ETAG, v);
    }

    Ok(response)
}

/// `PUT /api/workspace/raw` — write raw bytes to a workspace file.
///
/// The request body is the file's exact bytes, up to 8 MiB (the route's
/// body limit is raised to match). Otherwise identical to the text write
/// endpoint: atomic, creates parents, honors `If-Match`/`If-None-Match`,
/// sends the identity-file reload signal, and always succeeds — a
/// non-UTF-8 raw write to a recognized name like `HEARTBEAT.yml` is saved
/// with a diagnostic reported alongside it, the same as malformed YAML
/// would be. Returns `{ saved: true, version, diagnostics }`.
pub(super) async fn api_workspace_raw_write(
    Query(query): Query<FileQuery>,
    State(state): State<ConfigApiState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    if body.len() > TEXT_FILE_LIMIT_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "body exceeds the {TEXT_FILE_LIMIT_BYTES}-byte raw write limit ({} bytes)",
                body.len()
            ),
        ));
    }

    checkpoint_before_destructive_action(&state, &format!("raw write {}", query.path)).await;
    write_workspace_bytes(&state, &query.path, &body, &headers).await
}

/// Checkpoint the workspace before a destructive workspace API action
/// (delete, overwrite, move/rename with overwrite). Never blocks or fails
/// the action — see `crate::checkpoints`. Returns the id of the checkpoint
/// that holds the pre-action tree, or `None` when it could not be recorded.
async fn checkpoint_before_destructive_action(
    state: &ConfigApiState,
    summary: &str,
) -> Option<String> {
    state
        .checkpoint_workspace_id_before_write(summary.to_string())
        .await
}

/// `DELETE /api/workspace/file` — delete a workspace file or directory.
///
/// A directory requires `recursive=true`; without it the request answers
/// `409`. The workspace root cannot be deleted (`400`). `If-Match` applies
/// to files, not directories, since a directory has no single version
/// token. Deleting an identity file sends the reload signal.
/// Refuse a bulk operation on the directory at `dir` when it holds
/// Residuum's own data (the search index or a database), which Residuum
/// keeps open while it runs.
async fn refuse_if_dir_holds_internal_data(
    dir: &Path,
    relative: &str,
) -> Result<(), (StatusCode, String)> {
    let owned = dir.to_path_buf();
    let holds = tokio::task::spawn_blocking(move || dir_holds_internal_data(&owned))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to check {relative} for internal data: {e}"),
            )
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to check {relative} for internal data: {e}"),
            )
        })?;
    if holds {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "{relative} holds Residuum's own memory index or database files, which can't be deleted, moved, or replaced through the file API"
            ),
        ));
    }
    Ok(())
}

pub(super) async fn api_workspace_delete(
    Query(query): Query<DeleteFileQuery>,
    State(state): State<ConfigApiState>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let relative = &query.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let path = validate_workspace_path(&state.workspace_dir, relative).await?;
    let canonical_root = canonicalize_workspace_root(&state.workspace_dir).await?;
    if path == canonical_root {
        return Err((
            StatusCode::BAD_REQUEST,
            "the workspace root cannot be deleted".to_string(),
        ));
    }

    let metadata = tokio::fs::metadata(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to stat path: {e}"),
        )
    })?;

    let checkpoint_id;
    if metadata.is_dir() {
        if !query.recursive {
            return Err((
                StatusCode::CONFLICT,
                format!("{relative} is a directory; pass recursive=true to delete it"),
            ));
        }
        refuse_if_dir_holds_internal_data(&path, relative).await?;
        checkpoint_id =
            checkpoint_before_destructive_action(&state, &format!("delete {relative}")).await;
        tokio::fs::remove_dir_all(&path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete directory: {e}"),
            )
        })?;
    } else {
        if let Some(conflict) = check_conditional_write(&path, &headers).await? {
            return Ok(conflict);
        }
        checkpoint_id =
            checkpoint_before_destructive_action(&state, &format!("delete {relative}")).await;
        tokio::fs::remove_file(&path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete file: {e}"),
            )
        })?;
    }

    signal_identity_reload(relative, &state);

    Ok(Json(DeleteResponse {
        deleted: true,
        checkpoint_id,
    })
    .into_response())
}

/// `POST /api/workspace/dir` — create a workspace directory and any missing
/// parents. Idempotent: creating a directory that already exists succeeds
/// without changing anything. Answers `409` if a non-directory file already
/// exists at that path.
pub(super) async fn api_workspace_mkdir(
    State(state): State<ConfigApiState>,
    Json(req): Json<MkdirRequest>,
) -> Result<Response, (StatusCode, String)> {
    let relative = &req.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }
    if relative.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "path is required".to_string()));
    }

    let target = resolve_workspace_path_for_write(&state.workspace_dir, relative).await?;

    match tokio::fs::metadata(&target).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err((
                StatusCode::CONFLICT,
                format!("{relative} already exists and is not a directory"),
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(&target)
                .await
                .map_err(|create_err| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to create directory: {create_err}"),
                    )
                })?;
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat path: {e}"),
            ));
        }
    }

    Ok(Json(MkdirResponse { created: true }).into_response())
}

/// Get `to_path` ready to receive a rename: creates its missing parent
/// directories, and checks what already exists there. Without `overwrite`,
/// an existing destination answers `409`.
///
/// A file replacing a file is left to the rename itself, which replaces the
/// target in one step on every platform (on Windows `std::fs::rename` uses
/// `MOVEFILE_REPLACE_EXISTING`), so a failed move never loses the existing
/// file. Only when a directory is involved on either side is the destination
/// cleared first, since no platform renames over a non-empty directory or
/// between a file and a directory.
async fn ready_move_destination(
    to_path: &Path,
    to_relative: &str,
    overwrite: bool,
    source_is_dir: bool,
) -> Result<(), (StatusCode, String)> {
    let to_exists = match tokio::fs::metadata(to_path).await {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat destination path: {e}"),
            ));
        }
    };

    if to_exists.is_some() && !overwrite {
        return Err((
            StatusCode::CONFLICT,
            format!("{to_relative} already exists; pass overwrite: true to replace it"),
        ));
    }

    let parent = to_path.parent().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no parent directory: {to_relative}"),
        )
    })?;
    tokio::fs::create_dir_all(parent).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create parent directory: {e}"),
        )
    })?;

    let Some(existing) = to_exists else {
        return Ok(());
    };
    if !existing.is_dir() && !source_is_dir {
        return Ok(());
    }
    if existing.is_dir() {
        refuse_if_dir_holds_internal_data(to_path, to_relative).await?;
        tokio::fs::remove_dir_all(to_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to remove existing destination directory: {e}"),
            )
        })?;
    } else {
        tokio::fs::remove_file(to_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to remove existing destination file: {e}"),
            )
        })?;
    }
    Ok(())
}

/// `POST /api/workspace/move` — move or rename a workspace file or
/// directory.
///
/// Creates `to`'s missing parent directories. Answers `409` if `to` already
/// exists and `overwrite` isn't `true`. `If-Match` applies to `from` when it
/// is a file. Moving onto or away from an identity file sends the reload
/// signal. The move always succeeds; if `to` is one of the strictly-parsed
/// files `crate::diagnostics` understands, the response's `diagnostics`
/// names any problem with the moved file's content at its new name.
pub(super) async fn api_workspace_move(
    State(state): State<ConfigApiState>,
    headers: HeaderMap,
    Json(req): Json<MoveRequest>,
) -> Result<Response, (StatusCode, String)> {
    let from_relative = &req.from;
    let to_relative = &req.to;

    if is_blocked_path(from_relative) || is_blocked_path(to_relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let from_path = validate_workspace_path(&state.workspace_dir, from_relative).await?;
    let from_metadata = tokio::fs::metadata(&from_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to stat source path: {e}"),
        )
    })?;

    let mut diagnostics = Vec::new();
    if from_metadata.is_file() {
        if let Some(conflict) = check_conditional_write(&from_path, &headers).await? {
            return Ok(conflict);
        }
        let bytes = tokio::fs::read(&from_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read source file: {e}"),
            )
        })?;
        diagnostics = diagnose_write_content(&state, to_relative, &bytes);
    }

    let to_path = resolve_workspace_path_for_write(&state.workspace_dir, to_relative).await?;

    // Moving a path onto itself is a no-op success, not a 409: the
    // "destination already exists" check would otherwise see the source
    // file, and an `overwrite: true` move would delete it out from under
    // itself before the rename that was supposed to preserve it.
    if to_path == from_path {
        let version = from_metadata
            .is_file()
            .then(|| version_token(&from_metadata));
        return Ok(Json(MoveResponse {
            moved: true,
            version,
            diagnostics: Vec::new(),
        })
        .into_response());
    }

    if from_metadata.is_dir() {
        refuse_if_dir_holds_internal_data(&from_path, from_relative).await?;
    }

    if from_metadata.is_dir() && to_path.starts_with(&from_path) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("can't move {from_relative} into itself ({to_relative})"),
        ));
    }

    ready_move_destination(&to_path, to_relative, req.overwrite, from_metadata.is_dir()).await?;

    checkpoint_before_destructive_action(&state, &format!("move {from_relative} to {to_relative}"))
        .await;
    tokio::fs::rename(&from_path, &to_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to move {from_relative} to {to_relative}: {e}"),
        )
    })?;

    let version = if from_metadata.is_file() {
        let metadata = tokio::fs::metadata(&to_path).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat moved file: {e}"),
            )
        })?;
        Some(version_token(&metadata))
    } else {
        None
    };

    signal_identity_reload(from_relative, &state);
    signal_identity_reload(to_relative, &state);

    Ok(Json(MoveResponse {
        moved: true,
        version,
        diagnostics,
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
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    /// Deserialize a handler's JSON response body.
    async fn response_json<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
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
            checkpoints: crate::checkpoints::test_engine(),
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
    async fn workspace_file_write_saves_invalid_heartbeat_yaml_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("HEARTBEAT.yml"), "pulses: []")
            .await
            .unwrap();

        let state = make_state(ws_dir.clone());

        // Malformed YAML is saved, not rejected — the pulse scheduler hot-reloads
        // this file every tick and simply won't pick up broken YAML on its next
        // tick, so the write still succeeds and reports the problem instead of
        // silently discarding the edit.
        let response = api_workspace_file_write(
            State(state.clone()),
            HeaderMap::new(),
            Json(WriteFileRequest {
                path: "HEARTBEAT.yml".to_string(),
                content: "not: valid: yaml: [[[".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: WriteResponse = response_json(response).await;
        assert!(body.saved, "invalid HEARTBEAT.yml should still be saved");
        assert!(
            !body.diagnostics.is_empty(),
            "invalid YAML should produce a diagnostic"
        );

        // The on-disk file reflects the write, invalid content and all.
        let saved = tokio::fs::read_to_string(ws_dir.join("HEARTBEAT.yml"))
            .await
            .unwrap();
        assert_eq!(
            saved, "not: valid: yaml: [[[",
            "the write should have happened"
        );

        // Valid YAML has no diagnostics.
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
        let ok_body: WriteResponse = response_json(ok_response).await;
        assert!(
            ok_body.diagnostics.is_empty(),
            "valid HEARTBEAT.yml should have no diagnostics"
        );
    }

    // ── Validate ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn validate_reports_diagnostics_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        let Json(response) = api_workspace_validate(
            State(state),
            Json(ValidateFileRequest {
                path: "HEARTBEAT.yml".to_string(),
                content: "not: valid: yaml: [[[".to_string(),
            }),
        )
        .await;

        assert!(
            !response.diagnostics.is_empty(),
            "invalid YAML should produce a diagnostic"
        );
        assert!(
            !ws_dir.join("HEARTBEAT.yml").exists(),
            "validate must not write anything"
        );
    }

    #[tokio::test]
    async fn validate_clean_content_has_no_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let Json(response) = api_workspace_validate(
            State(state),
            Json(ValidateFileRequest {
                path: "HEARTBEAT.yml".to_string(),
                content: "pulses:\n  - name: test\n    schedule: \"1h\"\n    tasks: []\n"
                    .to_string(),
            }),
        )
        .await;

        assert!(response.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn validate_unrecognized_path_has_no_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let Json(response) = api_workspace_validate(
            State(state),
            Json(ValidateFileRequest {
                path: "notes.md".to_string(),
                content: "whatever content".to_string(),
            }),
        )
        .await;

        assert!(response.diagnostics.is_empty());
    }

    // ── Raw read/write ──────────────────────────────────────────────

    #[tokio::test]
    async fn raw_roundtrip_is_byte_identical_for_arbitrary_binary_content() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let content: Vec<u8> = (0_u8..=255).cycle().take(4096).collect();
        let write_response = api_workspace_raw_write(
            Query(FileQuery {
                path: "image.bin".to_string(),
            }),
            State(state.clone()),
            HeaderMap::new(),
            Bytes::from(content.clone()),
        )
        .await
        .unwrap();
        assert_eq!(write_response.status(), StatusCode::OK);

        let read_response = api_workspace_raw_read(
            Query(FileQuery {
                path: "image.bin".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap();
        assert!(read_response.headers().get(header::ETAG).is_some());
        let body = axum::body::to_bytes(read_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            &body[..],
            &content[..],
            "raw round trip must be byte-identical"
        );
    }

    #[tokio::test]
    async fn raw_read_guesses_content_type_from_extension() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("photo.png"), [0x89, b'P', b'N', b'G'])
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = api_workspace_raw_read(
            Query(FileQuery {
                path: "photo.png".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(content_type, "image/png");
    }

    #[tokio::test]
    async fn raw_read_missing_file_answers_404() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_raw_read(
            Query(FileQuery {
                path: "gone.bin".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn raw_write_blocked_path_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_raw_write(
            Query(FileQuery {
                path: "vectors.db".to_string(),
            }),
            State(state),
            HeaderMap::new(),
            Bytes::from_static(b"data"),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn raw_write_over_limit_answers_413() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let body = vec![0_u8; TEXT_FILE_LIMIT_BYTES + 1];
        let err = api_workspace_raw_write(
            Query(FileQuery {
                path: "too_big.bin".to_string(),
            }),
            State(state),
            HeaderMap::new(),
            Bytes::from(body),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn raw_read_over_limit_answers_413() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(
            ws_dir.join("huge.bin"),
            vec![0_u8; TEXT_FILE_LIMIT_BYTES + 1],
        )
        .await
        .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_raw_read(
            Query(FileQuery {
                path: "huge.bin".to_string(),
            }),
            State(state),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn raw_write_conditional_if_match_stale_answers_412() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("data.bin"), b"original")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_static("stale-version"));

        let response = api_workspace_raw_write(
            Query(FileQuery {
                path: "data.bin".to_string(),
            }),
            State(state),
            headers,
            Bytes::from_static(b"changed"),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn raw_write_saves_non_utf8_heartbeat_content_with_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_raw_write(
            Query(FileQuery {
                path: "HEARTBEAT.yml".to_string(),
            }),
            State(state),
            HeaderMap::new(),
            Bytes::from_static(&[0xff, 0xfe, 0x00]),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: WriteResponse = response_json(response).await;
        assert!(body.saved, "non-UTF-8 content should still be saved");
        assert!(
            !body.diagnostics.is_empty(),
            "non-UTF-8 content on a recognized path should produce a diagnostic"
        );

        let saved = tokio::fs::read(ws_dir.join("HEARTBEAT.yml")).await.unwrap();
        assert_eq!(
            saved,
            vec![0xff, 0xfe, 0x00],
            "the write should have happened"
        );
    }

    // ── Delete ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_file_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("gone.md"), "bye")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "gone.md".to_string(),
                recursive: false,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!ws_dir.join("gone.md").exists());
    }

    #[tokio::test]
    async fn delete_returns_the_checkpoint_taken_before_the_delete() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_support::watching_state(dir.path());
        let ws_dir = state.workspace_dir.clone();
        tokio::fs::write(ws_dir.join("gone.md"), "bye")
            .await
            .unwrap();

        let response = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "gone.md".to_string(),
                recursive: false,
            }),
            State(state.clone()),
            HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "delete should succeed");
        let body: DeleteResponse = response_json(response).await;
        let id = body
            .checkpoint_id
            .expect("delete should name the checkpoint taken before it");
        let stored = state
            .checkpoints
            .file_content_at(
                crate::checkpoints::RepoKind::Workspace,
                id.clone(),
                "gone.md".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            stored.as_deref(),
            Some(b"bye".as_slice()),
            "the returned checkpoint must still contain the deleted file"
        );

        tokio::fs::write(ws_dir.join("later.txt"), "after")
            .await
            .unwrap();
        let later = state
            .checkpoint_workspace_id_before_write("later workspace write")
            .await
            .expect("a later checkpoint should be recorded");
        assert_ne!(
            later, id,
            "the id returned for Undo stays the pre-delete checkpoint after a newer one is taken"
        );
    }

    #[tokio::test]
    async fn delete_missing_path_answers_404() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "nope.md".to_string(),
                recursive: false,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_blocked_path_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("vectors.db"), "x")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "vectors.db".to_string(),
                recursive: false,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_directory_without_recursive_answers_409() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("folder"))
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "folder".to_string(),
                recursive: false,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn delete_directory_recursive_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("folder"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("folder/a.md"), "a")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "folder".to_string(),
                recursive: true,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!ws_dir.join("folder").exists());
    }

    #[tokio::test]
    async fn delete_workspace_root_answers_400() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        for root in ["", "."] {
            let err = api_workspace_delete(
                Query(DeleteFileQuery {
                    path: root.to_string(),
                    recursive: true,
                }),
                State(state.clone()),
                HeaderMap::new(),
            )
            .await
            .unwrap_err();
            assert_eq!(
                err.0,
                StatusCode::BAD_REQUEST,
                "root path {root:?} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn delete_file_with_stale_if_match_answers_412() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "content")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_static("stale-version"));

        let response = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "notes.md".to_string(),
                recursive: false,
            }),
            State(state),
            headers,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert!(
            ws_dir.join("notes.md").exists(),
            "stale delete must not touch the file"
        );
    }

    #[tokio::test]
    async fn delete_identity_file_sends_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("SOUL.md"), "# hi")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let state = super::super::ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: ws_dir,
            memory_dir: None,
            reload_tx: Some(tx),
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };

        api_workspace_delete(
            Query(DeleteFileQuery {
                path: "SOUL.md".to_string(),
                recursive: false,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap();

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ReloadSignal::Workspace);
    }

    // ── Mkdir ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mkdir_creates_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_mkdir(
            State(state),
            Json(MkdirRequest {
                path: "wiki/pages/nested".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(ws_dir.join("wiki/pages/nested").is_dir());
    }

    #[tokio::test]
    async fn mkdir_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("folder"))
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let response = api_workspace_mkdir(
            State(state),
            Json(MkdirRequest {
                path: "folder".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mkdir_conflicts_with_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("notes.md"), "x")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_mkdir(
            State(state),
            Json(MkdirRequest {
                path: "notes.md".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn mkdir_blocked_path_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_mkdir(
            State(state),
            Json(MkdirRequest {
                path: ".index".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    // ── Move ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn move_file_renames_and_returns_a_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("old.md"), "content")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "old.md".to_string(),
                to: "new.md".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: MoveResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.moved);
        assert!(parsed.version.is_some());
        assert!(!ws_dir.join("old.md").exists());
        assert_eq!(
            tokio::fs::read_to_string(ws_dir.join("new.md"))
                .await
                .unwrap(),
            "content"
        );
    }

    #[tokio::test]
    async fn move_directory_has_no_version_in_the_response() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("folder"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("folder/a.md"), "a")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "folder".to_string(),
                to: "renamed".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: MoveResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.moved);
        assert!(parsed.version.is_none());
        assert!(ws_dir.join("renamed/a.md").exists());
    }

    #[tokio::test]
    async fn move_missing_source_answers_404() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "nope.md".to_string(),
                to: "dest.md".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn move_onto_existing_destination_without_overwrite_answers_409() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "a").await.unwrap();
        tokio::fs::write(ws_dir.join("b.md"), "b").await.unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "a.md".to_string(),
                to: "b.md".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn move_onto_existing_destination_with_overwrite_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "a-content")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("b.md"), "b-content")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "a.md".to_string(),
                to: "b.md".to_string(),
                overwrite: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tokio::fs::read_to_string(ws_dir.join("b.md"))
                .await
                .unwrap(),
            "a-content"
        );
        assert!(!ws_dir.join("a.md").exists());
    }

    /// A workspace whose `memory/` holds a search index next to ordinary notes.
    async fn workspace_with_memory_index(dir: &Path) -> PathBuf {
        let ws_dir = dir.join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("memory/.index"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("memory/.index/segment.bin"), "x")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("memory/notes.md"), "n")
            .await
            .unwrap();
        ws_dir
    }

    #[tokio::test]
    async fn recursive_delete_of_a_dir_holding_internal_data_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = workspace_with_memory_index(dir.path()).await;
        let state = make_state(ws_dir.clone());

        let err = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "memory".to_string(),
                recursive: true,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(ws_dir.join("memory/.index/segment.bin").exists());
        assert!(ws_dir.join("memory/notes.md").exists());
    }

    #[tokio::test]
    async fn moving_a_dir_holding_internal_data_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = workspace_with_memory_index(dir.path()).await;
        let state = make_state(ws_dir.clone());

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "memory".to_string(),
                to: "old-memory".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(ws_dir.join("memory/.index/segment.bin").exists());
    }

    #[tokio::test]
    async fn overwriting_a_dir_holding_internal_data_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = workspace_with_memory_index(dir.path()).await;
        tokio::fs::create_dir_all(ws_dir.join("scratch"))
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "scratch".to_string(),
                to: "memory".to_string(),
                overwrite: true,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(ws_dir.join("memory/.index/segment.bin").exists());
    }

    #[tokio::test]
    async fn recursive_delete_of_an_ordinary_dir_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = workspace_with_memory_index(dir.path()).await;
        tokio::fs::create_dir_all(ws_dir.join("notes/deep"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/deep/a.md"), "a")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_delete(
            Query(DeleteFileQuery {
                path: "notes".to_string(),
                recursive: true,
            }),
            State(state),
            HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!ws_dir.join("notes").exists());
    }

    #[tokio::test]
    async fn move_directory_into_itself_answers_400() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(ws_dir.join("notes"))
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("notes/a.md"), "a")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "notes".to_string(),
                to: "notes/archive".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(ws_dir.join("notes/a.md").exists());
    }

    #[tokio::test]
    async fn move_creates_destination_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "a").await.unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "a.md".to_string(),
                to: "deep/nested/b.md".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(ws_dir.join("deep/nested/b.md").exists());
    }

    #[tokio::test]
    async fn move_blocked_path_answers_403() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("vectors.db"), "x")
            .await
            .unwrap();
        let state = make_state(ws_dir);

        let err = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "vectors.db".to_string(),
                to: "dest.db".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn move_file_with_stale_if_match_answers_412() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "a").await.unwrap();
        let state = make_state(ws_dir.clone());

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_static("stale-version"));

        let response = api_workspace_move(
            State(state),
            headers,
            Json(MoveRequest {
                from: "a.md".to_string(),
                to: "b.md".to_string(),
                overwrite: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert!(
            ws_dir.join("a.md").exists(),
            "stale move must not touch the file"
        );
    }

    #[tokio::test]
    async fn move_onto_identity_file_sends_reload_signal() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("draft.md"), "# hi")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let state = super::super::ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: ws_dir,
            memory_dir: None,
            reload_tx: Some(tx),
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
        };

        api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "draft.md".to_string(),
                to: "SOUL.md".to_string(),
                overwrite: true,
            }),
        )
        .await
        .unwrap();

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ReloadSignal::Workspace);
    }

    #[tokio::test]
    async fn move_onto_heartbeat_with_invalid_content_still_moves_with_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("draft.yml"), "not: valid: yaml: [[[")
            .await
            .unwrap();
        tokio::fs::write(ws_dir.join("HEARTBEAT.yml"), "pulses: []")
            .await
            .unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "draft.yml".to_string(),
                to: "HEARTBEAT.yml".to_string(),
                overwrite: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: MoveResponse = response_json(response).await;
        assert!(body.moved, "the move should still happen");
        assert!(
            !body.diagnostics.is_empty(),
            "invalid content at the destination should produce a diagnostic"
        );

        assert!(
            !ws_dir.join("draft.yml").exists(),
            "the source should have moved away"
        );
        assert_eq!(
            tokio::fs::read_to_string(ws_dir.join("HEARTBEAT.yml"))
                .await
                .unwrap(),
            "not: valid: yaml: [[[",
            "the destination should hold the moved content"
        );
    }

    #[tokio::test]
    async fn move_onto_self_is_a_no_op_success() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        tokio::fs::write(ws_dir.join("a.md"), "a").await.unwrap();
        let state = make_state(ws_dir.clone());

        let response = api_workspace_move(
            State(state),
            HeaderMap::new(),
            Json(MoveRequest {
                from: "a.md".to_string(),
                to: "a.md".to_string(),
                overwrite: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tokio::fs::read_to_string(ws_dir.join("a.md"))
                .await
                .unwrap(),
            "a"
        );
    }
}

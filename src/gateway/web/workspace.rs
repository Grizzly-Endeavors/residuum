//! Workspace file browser API endpoints.

use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::gateway::ReloadSignal;

use super::ConfigApiState;

/// A single entry in a workspace directory listing.
#[derive(Serialize)]
pub(super) struct WorkspaceEntry {
    pub name: String,
    pub entry_type: String,
    pub size: Option<u64>,
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
#[derive(Serialize)]
pub(super) struct WriteResponse {
    pub saved: bool,
}

/// Resolve and validate a path relative to the workspace directory.
///
/// Canonicalizes both the workspace root and the joined path, then verifies the
/// result is still inside the workspace. Returns 403 if the path escapes the
/// workspace boundary.
fn validate_workspace_path(
    workspace_dir: &Path,
    relative: &str,
) -> Result<PathBuf, (StatusCode, String)> {
    let canonical_root = workspace_dir.canonicalize().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to resolve workspace directory: {e}"),
        )
    })?;

    let target = workspace_dir.join(relative);
    let canonical_target = target.canonicalize().map_err(|e| {
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

/// Resolve and validate a path relative to the workspace directory for writing.
///
/// Unlike `validate_workspace_path`, this canonicalizes the *parent* directory so
/// that paths to files that do not exist yet can still be validated. Returns 404
/// if the parent directory does not exist.
fn validate_workspace_path_for_write(
    workspace_dir: &Path,
    relative: &str,
) -> Result<PathBuf, (StatusCode, String)> {
    let canonical_root = workspace_dir.canonicalize().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to resolve workspace directory: {e}"),
        )
    })?;

    let target = workspace_dir.join(relative);
    let parent = target.parent().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no parent directory: {relative}"),
        )
    })?;

    let canonical_parent = parent.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (
                StatusCode::NOT_FOUND,
                format!("parent directory does not exist: {relative}"),
            )
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to resolve parent directory: {e}"),
            )
        }
    })?;

    if !canonical_parent.starts_with(&canonical_root) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("path traversal rejected: {relative}"),
        ));
    }

    let file_name = target.file_name().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("path has no file name: {relative}"),
        )
    })?;

    Ok(canonical_parent.join(file_name))
}

/// Returns true if the path refers to an internal index or database file that
/// should never be exposed through the file browser API.
fn is_blocked_path(relative: &str) -> bool {
    let path = Path::new(relative);

    // Block anything inside .index/
    if relative.contains(".index/") {
        return true;
    }

    // Block by extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && (ext == "db" || ext == "sqlite")
    {
        return true;
    }

    false
}

/// Returns true if the path refers to a workspace identity file.
///
/// Checks the file name only (ignores any leading directory components) so that
/// identity files nested inside subdirectories are also recognised.
fn is_identity_file(relative: &str) -> bool {
    const IDENTITY_FILES: &[&str] = &[
        "SOUL.md",
        "AGENTS.md",
        "USER.md",
        "MEMORY.md",
        "ENVIRONMENT.md",
        "PRESENCE.toml",
        "HEARTBEAT.yml",
    ];

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
        validate_workspace_path(&state.workspace_dir, &relative)?
    };

    let read_dir = std::fs::read_dir(&dir_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read directory: {e}"),
        )
    })?;

    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read directory entry: {e}"),
            )
        })?;

        let name = entry.file_name().to_string_lossy().into_owned();

        // Build a relative path for the blocked-path check
        let entry_relative = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };

        if is_blocked_path(&entry_relative) {
            continue;
        }

        let metadata = entry.metadata().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read metadata for {name}: {e}"),
            )
        })?;

        let (entry_type, size) = if metadata.is_dir() {
            ("directory".to_string(), None)
        } else {
            ("file".to_string(), Some(metadata.len()))
        };

        entries.push(WorkspaceEntry {
            name,
            entry_type,
            size,
        });
    }

    // Sort: directories first, then alphabetically within each group.
    entries.sort_by(|a, b| {
        let a_is_dir = a.entry_type == "directory";
        let b_is_dir = b.entry_type == "directory";
        b_is_dir.cmp(&a_is_dir).then_with(|| a.name.cmp(&b.name))
    });

    Ok(Json(entries))
}

/// `GET /api/workspace/file` — read a workspace file as plain text.
///
/// Returns 404 if the file does not exist, 403 if the path is blocked, and 413
/// if the file exceeds the 1 MiB size limit.
pub(super) async fn api_workspace_file_read(
    Query(query): Query<FileQuery>,
    State(state): State<ConfigApiState>,
) -> Result<String, (StatusCode, String)> {
    let relative = &query.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    let path = validate_workspace_path(&state.workspace_dir, relative)?;

    let metadata = std::fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (StatusCode::NOT_FOUND, format!("file not found: {relative}"))
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to stat file: {e}"),
            )
        }
    })?;

    if metadata.len() > 1_048_576 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("file exceeds 1 MiB limit ({} bytes)", metadata.len()),
        ));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read file: {e}"),
        )
    })?;

    Ok(content)
}

/// `PUT /api/workspace/file` — write content to a workspace file.
///
/// Creates the file if it does not exist. If the written file is a workspace
/// identity file and a reload channel is available, sends a `Workspace` reload
/// signal.
pub(super) async fn api_workspace_file_write(
    State(state): State<ConfigApiState>,
    Json(req): Json<WriteFileRequest>,
) -> Result<Json<WriteResponse>, (StatusCode, String)> {
    let relative = &req.path;

    if is_blocked_path(relative) {
        return Err((
            StatusCode::FORBIDDEN,
            "access to this path is blocked".to_string(),
        ));
    }

    // Determine write target: prefer validate_workspace_path for existing files
    // (cheaper, no parent lookup), fall back to validate_workspace_path_for_write
    // for new files.
    let target_path = match validate_workspace_path(&state.workspace_dir, relative) {
        Ok(p) => p,
        Err((StatusCode::NOT_FOUND, _)) => {
            validate_workspace_path_for_write(&state.workspace_dir, relative)?
        }
        Err(e) => return Err(e),
    };

    std::fs::write(&target_path, &req.content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write file: {e}"),
        )
    })?;

    if is_identity_file(relative)
        && let Some(tx) = &state.reload_tx
    {
        // Best-effort: receiver may have been dropped during shutdown.
        drop(tx.send(ReloadSignal::Workspace));
    }

    Ok(Json(WriteResponse { saved: true }))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn blocked_paths() {
        assert!(is_blocked_path(".index/foo"));
        assert!(is_blocked_path("data.db"));
        assert!(is_blocked_path("store.sqlite"));
        assert!(is_blocked_path("vectors.db"));
        assert!(!is_blocked_path("SOUL.md"));
        assert!(!is_blocked_path("skills/research.md"));
    }

    #[test]
    fn identity_files() {
        assert!(is_identity_file("SOUL.md"));
        assert!(is_identity_file("AGENTS.md"));
        assert!(is_identity_file("subdir/SOUL.md"));
        assert!(!is_identity_file("skills/research.md"));
        assert!(!is_identity_file("random.txt"));
    }

    #[test]
    fn path_traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_workspace_path(dir.path(), "../etc/passwd").is_err());
        assert!(validate_workspace_path(dir.path(), "/etc/passwd").is_err());
        assert!(validate_workspace_path(dir.path(), "foo/../../etc/passwd").is_err());
    }

    #[test]
    fn valid_paths_accepted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.md"), "hello").unwrap();
        let result = validate_workspace_path(dir.path(), "test.md");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn workspace_file_roundtrip() {
        use axum::Json;
        use axum::extract::{Query, State};

        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("workspace");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(ws_dir.join("SOUL.md"), "# Soul").unwrap();
        std::fs::write(ws_dir.join("notes.md"), "some notes").unwrap();
        std::fs::create_dir(ws_dir.join("skills")).unwrap();
        std::fs::write(ws_dir.join("skills").join("research.md"), "skill content").unwrap();
        // Create a blocked file to verify filtering
        std::fs::write(ws_dir.join("vectors.db"), "binary data").unwrap();

        let state = super::super::ConfigApiState {
            config_dir: dir.path().to_path_buf(),
            workspace_dir: ws_dir.clone(),
            memory_dir: None,
            reload_tx: None,
            setup_done: None,
            secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        };

        // List root
        let entries = api_workspace_files(Query(FilesQuery { path: None }), State(state.clone()))
            .await
            .unwrap();
        let names: Vec<&str> = entries.0.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"SOUL.md"));
        assert!(names.contains(&"notes.md"));
        assert!(names.contains(&"skills"));
        assert!(!names.contains(&"vectors.db"));

        // Read a file
        let content = api_workspace_file_read(
            Query(FileQuery {
                path: "SOUL.md".to_string(),
            }),
            State(state.clone()),
        )
        .await
        .unwrap();
        assert_eq!(content, "# Soul");

        // Write a file
        let write_result = api_workspace_file_write(
            State(state.clone()),
            Json(WriteFileRequest {
                path: "SOUL.md".to_string(),
                content: "# Updated Soul".to_string(),
            }),
        )
        .await
        .unwrap();
        assert!(write_result.0.saved);

        // Verify write persisted
        let updated = api_workspace_file_read(
            Query(FileQuery {
                path: "SOUL.md".to_string(),
            }),
            State(state.clone()),
        )
        .await
        .unwrap();
        assert_eq!(updated, "# Updated Soul");

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
}

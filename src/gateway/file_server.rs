//! File serving for WebSocket-connected clients.
//!
//! Registers files with a TTL and serves them via HTTP. WebSocket messages
//! reference files by URL rather than embedding binary data.
//!
//! A file inside the workspace is served by its workspace-relative path
//! instead (see [`FileRegistry::url_for`]) — a link that keeps working for
//! as long as the file exists, not just for the TTL. Only a file outside
//! the workspace (or the workspace boundary was never configured) falls
//! back to the token-registry scheme below.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::time::Instant;

/// How long a registered file remains available (1 hour).
const FILE_TTL_SECS: u64 = 3600;

/// How often the cleanup task sweeps expired entries (10 minutes).
const CLEANUP_INTERVAL_SECS: u64 = 600;

/// A registered file entry with expiration.
struct FileEntry {
    path: PathBuf,
    mime_type: String,
    filename: String,
    expires_at: Instant,
}

/// Thread-safe registry of files available for HTTP serving.
#[derive(Clone)]
pub struct FileRegistry {
    entries: Arc<RwLock<HashMap<String, FileEntry>>>,
    /// The workspace root, when configured — see [`Self::with_workspace_root`].
    workspace_root: Option<PathBuf>,
}

impl FileRegistry {
    /// Create a new empty file registry, with no workspace root configured
    /// (every file is served through the expiring token scheme).
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            workspace_root: None,
        }
    }

    /// Serve a file under `root` by its durable workspace-relative URL
    /// (see [`Self::url_for`]) instead of the expiring token scheme.
    #[must_use]
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(root);
        self
    }

    /// Register a file and return its unique serving ID.
    pub async fn register(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = FileEntry {
            path,
            mime_type,
            filename,
            expires_at: Instant::now() + std::time::Duration::from_secs(FILE_TTL_SECS),
        };
        self.entries.write().await.insert(id.clone(), entry);
        id
    }

    /// The URL a client should use to fetch `path` (an absolute filesystem
    /// path already known to exist and be readable): a durable
    /// workspace-relative URL when `path` is inside the configured
    /// workspace root and not blocked from exposure by
    /// `workspace::access::is_blocked_path`, else the existing
    /// [`Self::register`]-backed token URL.
    pub async fn url_for(&self, path: PathBuf, mime_type: String, filename: String) -> String {
        if let Some(relative) = self.workspace_relative_path(&path).await {
            let encoded: String =
                url::form_urlencoded::byte_serialize(relative.as_bytes()).collect();
            return format!("/api/files/workspace?path={encoded}");
        }
        let id = self.register(path, mime_type, filename).await;
        format!("/api/files/{id}")
    }

    /// `path`'s location relative to the workspace root, as a `/`-separated
    /// string, when it's inside the workspace and not one of the paths
    /// `workspace::access::is_blocked_path` hides — `None` otherwise
    /// (outside the workspace, no workspace root configured, or either
    /// path fails to canonicalize).
    async fn workspace_relative_path(&self, path: &Path) -> Option<String> {
        let root = self.workspace_root.as_ref()?;
        let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
        let canonical_path = tokio::fs::canonicalize(path).await.ok()?;
        let relative = canonical_path.strip_prefix(&canonical_root).ok()?;
        let relative_str = relative.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
        if crate::workspace::access::is_blocked_path(&relative_str) {
            return None;
        }
        Some(relative_str)
    }

    /// Look up a file by ID. Returns `(path, mime_type, filename)` if found and not expired.
    pub async fn get(&self, id: &str) -> Option<(PathBuf, String, String)> {
        let entries = self.entries.read().await;
        let entry = entries.get(id)?;
        if entry.expires_at < Instant::now() {
            return None;
        }
        Some((
            entry.path.clone(),
            entry.mime_type.clone(),
            entry.filename.clone(),
        ))
    }

    /// Remove all expired entries. Returns `(removed, remaining)`.
    pub async fn sweep_expired(&self) -> (usize, usize) {
        let now = Instant::now();
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|_, entry| entry.expires_at > now);
        let remaining = entries.len();
        (before - remaining, remaining)
    }

    /// Spawn a background task that periodically sweeps expired entries.
    pub fn spawn_cleanup_task(&self) {
        let registry = self.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                interval.tick().await;
                let (removed, remaining) = registry.sweep_expired().await;
                if removed > 0 {
                    tracing::debug!(removed, remaining, "swept expired file entries");
                }
            }
        });
    }
}

impl Default for FileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum handler to serve a registered file by ID.
pub async fn serve_file(
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::State(registry): axum::extract::State<FileRegistry>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let Some((path, mime_type, filename)) = registry.get(&id).await else {
        tracing::debug!(file_id = %id, "file link expired or unknown, refusing to serve");
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(bytes) = tokio::fs::read(&path).await else {
        tracing::warn!(file_id = %id, path = %path.display(), "registered file not readable");
        return StatusCode::NOT_FOUND.into_response();
    };

    respond_with_file(bytes, &mime_type, &filename)
}

/// Query parameters for [`serve_workspace_file`].
#[derive(Debug, serde::Deserialize)]
pub struct WorkspaceFileQuery {
    /// The file's path relative to the workspace root.
    path: String,
}

/// Axum handler serving a file by its workspace-relative path — the
/// counterpart to [`serve_file`] for files inside the workspace, which
/// never expires as long as the file itself still exists there.
pub async fn serve_workspace_file(
    axum::extract::Query(query): axum::extract::Query<WorkspaceFileQuery>,
    axum::extract::State(registry): axum::extract::State<FileRegistry>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let Some(root) = &registry.workspace_root else {
        tracing::warn!("workspace file link requested but no workspace root is configured");
        return StatusCode::NOT_FOUND.into_response();
    };

    if crate::workspace::access::is_blocked_path(&query.path) {
        tracing::warn!(path = %query.path, "refusing to serve a blocked workspace path");
        return StatusCode::NOT_FOUND.into_response();
    }

    let Ok(canonical_root) = tokio::fs::canonicalize(root).await else {
        tracing::warn!(root = %root.display(), "failed to canonicalize workspace root while serving a file link");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let missing_file_response = || {
        (
            StatusCode::NOT_FOUND,
            "This file is no longer available in the workspace.",
        )
            .into_response()
    };

    let target = root.join(&query.path);
    let canonical_target = match tokio::fs::canonicalize(&target).await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(path = %query.path, error = %e, "workspace file link points at a file that no longer exists");
            return missing_file_response();
        }
    };
    if !canonical_target.starts_with(&canonical_root) {
        tracing::warn!(path = %query.path, "workspace file link escaped the workspace root, refusing");
        return StatusCode::FORBIDDEN.into_response();
    }

    let Ok(bytes) = tokio::fs::read(&canonical_target).await else {
        tracing::warn!(path = %query.path, "workspace file link target not readable");
        return missing_file_response();
    };

    let filename = canonical_target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let mime_type = crate::interfaces::attachment::detect_mime_type(&canonical_target);
    respond_with_file(bytes, &mime_type, filename)
}

/// Build the HTTP response for a file's bytes, with a content-disposition
/// naming `filename` (quotes and backslashes stripped, since they'd need
/// escaping in the header value) and, when it parses as a valid header
/// value, `mime_type` as the content type.
fn respond_with_file(bytes: Vec<u8>, mime_type: &str, filename: &str) -> axum::response::Response {
    use axum::http::{HeaderValue, header};
    use axum::response::IntoResponse;

    let safe_filename: String = filename
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let disposition = format!("inline; filename=\"{safe_filename}\"");
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_lookup() {
        let registry = FileRegistry::new();
        let id = registry
            .register(
                PathBuf::from("/tmp/test.pdf"),
                "application/pdf".to_string(),
                "test.pdf".to_string(),
            )
            .await;

        let entry = registry.get(&id).await;
        assert!(entry.is_some(), "registered file should be found");
        let (path, mime, filename) = entry.unwrap();
        assert_eq!(path, PathBuf::from("/tmp/test.pdf"));
        assert_eq!(mime, "application/pdf");
        assert_eq!(filename, "test.pdf");
    }

    #[tokio::test]
    async fn lookup_unknown_id_returns_none() {
        let registry = FileRegistry::new();
        assert!(registry.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn sweep_removes_expired() {
        let registry = FileRegistry::new();
        // Insert with already-expired time
        {
            let mut entries = registry.entries.write().await;
            entries.insert(
                "expired-id".to_string(),
                FileEntry {
                    path: PathBuf::from("/tmp/old.pdf"),
                    mime_type: "application/pdf".to_string(),
                    filename: "old.pdf".to_string(),
                    expires_at: Instant::now() - std::time::Duration::from_secs(1),
                },
            );
        }
        let (removed, remaining) = registry.sweep_expired().await;
        assert_eq!(removed, 1, "one expired entry should be removed");
        assert_eq!(remaining, 0, "no entries should remain");
        assert!(registry.get("expired-id").await.is_none());
    }

    #[tokio::test]
    async fn sweep_concurrent_register_does_not_underflow() {
        // Regression: earlier implementation computed removed = before - after across
        // separate lock acquisitions, which could underflow if a register raced in.
        let registry = FileRegistry::new();
        registry
            .register(
                PathBuf::from("/tmp/live.pdf"),
                "application/pdf".to_string(),
                "live.pdf".to_string(),
            )
            .await;
        let (removed, remaining) = registry.sweep_expired().await;
        assert_eq!(removed, 0, "no expired entries should be removed");
        assert_eq!(remaining, 1, "live entry should remain");
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn url_for_a_file_inside_the_workspace_is_a_durable_workspace_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        let file_path = dir.path().join("notes/plan.md");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new().with_workspace_root(dir.path().to_path_buf());
        let url = registry
            .url_for(
                file_path,
                "text/markdown".to_string(),
                "plan.md".to_string(),
            )
            .await;

        assert_eq!(
            url, "/api/files/workspace?path=notes%2Fplan.md",
            "should be a workspace-path link, not an expiring token: {url}"
        );
    }

    #[tokio::test]
    async fn url_for_a_file_outside_the_workspace_falls_back_to_a_token_link() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file_path = outside.path().join("scratch.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new().with_workspace_root(dir.path().to_path_buf());
        let url = registry
            .url_for(
                file_path,
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;

        assert!(
            url.starts_with("/api/files/") && !url.starts_with("/api/files/workspace"),
            "a file outside the workspace should still get a token link: {url}"
        );
    }

    #[tokio::test]
    async fn url_for_with_no_workspace_root_configured_always_uses_a_token_link() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("scratch.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let registry = FileRegistry::new();
        let url = registry
            .url_for(
                file_path,
                "text/plain".to_string(),
                "scratch.txt".to_string(),
            )
            .await;

        assert!(!url.starts_with("/api/files/workspace"), "got: {url}");
    }

    #[tokio::test]
    async fn serve_workspace_file_serves_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.md"), "# hello").unwrap();
        let registry = FileRegistry::new().with_workspace_root(dir.path().to_path_buf());

        let resp = serve_workspace_file(
            axum::extract::Query(WorkspaceFileQuery {
                path: "report.md".to_string(),
            }),
            axum::extract::State(registry),
        )
        .await;

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(body_text(resp).await, "# hello");
    }

    #[tokio::test]
    async fn serve_workspace_file_missing_file_gives_a_plain_explanation() {
        let dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::new().with_workspace_root(dir.path().to_path_buf());

        let resp = serve_workspace_file(
            axum::extract::Query(WorkspaceFileQuery {
                path: "gone.md".to_string(),
            }),
            axum::extract::State(registry),
        )
        .await;

        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(
            body_text(resp).await.contains("no longer available"),
            "a missing file should explain itself in plain language, not a bare 404"
        );
    }

    #[tokio::test]
    async fn serve_workspace_file_rejects_a_path_that_escapes_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ws")).unwrap();
        let workspace = dir.path().join("ws");
        std::fs::write(dir.path().join("secret.txt"), "nope").unwrap();
        let registry = FileRegistry::new().with_workspace_root(workspace);

        let resp = serve_workspace_file(
            axum::extract::Query(WorkspaceFileQuery {
                path: "../secret.txt".to_string(),
            }),
            axum::extract::State(registry),
        )
        .await;

        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn serve_workspace_file_rejects_a_blocked_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".index")).unwrap();
        std::fs::write(dir.path().join(".index/segment.bin"), "data").unwrap();
        let registry = FileRegistry::new().with_workspace_root(dir.path().to_path_buf());

        let resp = serve_workspace_file(
            axum::extract::Query(WorkspaceFileQuery {
                path: ".index/segment.bin".to_string(),
            }),
            axum::extract::State(registry),
        )
        .await;

        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }
}

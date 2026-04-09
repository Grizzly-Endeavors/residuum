//! File serving for WebSocket-connected clients.
//!
//! Registers files with a TTL and serves them via HTTP. WebSocket messages
//! reference files by URL rather than embedding binary data.

use std::collections::HashMap;
use std::path::PathBuf;
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
}

impl FileRegistry {
    /// Create a new empty file registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
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
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse;

    let Some((path, mime_type, filename)) = registry.get(&id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(bytes) = tokio::fs::read(&path).await else {
        tracing::warn!(file_id = %id, path = %path.display(), "registered file not readable");
        return StatusCode::NOT_FOUND.into_response();
    };

    let safe_filename: String = filename
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let disposition = format!("inline; filename=\"{safe_filename}\"");
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    response
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
}

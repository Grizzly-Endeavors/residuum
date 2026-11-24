use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use residuum_bus::attachment::detect_mime_type;

pub const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

pub const RETENTION: Duration = Duration::hours(24);

const UPLOADS_DIR: &str = "uploads";
const CONTENT_DIR: &str = "content";
const META_FILE: &str = "meta.json";
const STAGING_PREFIX: &str = ".staging-";

#[derive(Debug, Clone)]
pub struct UploadStore {
    dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpload {
    pub id: String,
    pub filename: String,
    pub size: u64,
    pub content_type: String,
    pub created_at: DateTime<Utc>,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("the upload is empty")]
    Empty,
    #[error("the upload is {size} bytes, more than the {limit}-byte limit")]
    TooLarge { size: usize, limit: usize },
    #[error("the filename {reason}")]
    InvalidFilename { reason: &'static str },
    #[error("`{0}` is not an upload id")]
    InvalidId(String),
    #[error("no staged upload has the id `{0}`")]
    NotFound(String),
    #[error("failed to {action} the staged upload at {path}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct UploadMeta {
    filename: String,
    size: u64,
    content_type: String,
    created_at: DateTime<Utc>,
}

impl UploadStore {
    pub fn open(hub_dir: &Path) -> Result<Self, UploadError> {
        let dir = hub_dir.join(UPLOADS_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| UploadError::Io {
            action: "create",
            path: dir.clone(),
            source,
        })?;
        let entries = std::fs::read_dir(&dir).map_err(|source| UploadError::Io {
            action: "read",
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| UploadError::Io {
                action: "read",
                path: dir.clone(),
                source,
            })?;
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
            {
                continue;
            }
            let path = entry.path();
            std::fs::remove_dir_all(&path).map_err(|source| UploadError::Io {
                action: "remove",
                path,
                source,
            })?;
        }
        Ok(Self { dir })
    }

    pub async fn stage(&self, filename: &str, bytes: &[u8]) -> Result<StagedUpload, UploadError> {
        self.stage_at(filename, bytes, Utc::now()).await
    }

    pub async fn resolve(&self, id: &str) -> Result<StagedUpload, UploadError> {
        check_id(id)?;
        let dir = self.dir.join(id);
        let meta_path = dir.join(META_FILE);
        let text = match tokio::fs::read_to_string(&meta_path).await {
            Ok(text) => text,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(UploadError::NotFound(id.to_string()));
            }
            Err(source) => {
                return Err(UploadError::Io {
                    action: "read",
                    path: meta_path,
                    source,
                });
            }
        };
        let meta: UploadMeta = serde_json::from_str(&text).map_err(|source| UploadError::Io {
            action: "parse",
            path: meta_path.clone(),
            source: std::io::Error::new(ErrorKind::InvalidData, source),
        })?;
        if validate_filename(&meta.filename).is_err() {
            return Err(UploadError::Io {
                action: "parse",
                path: meta_path,
                source: std::io::Error::new(
                    ErrorKind::InvalidData,
                    "the stored filename is not a base name",
                ),
            });
        }
        Ok(StagedUpload {
            id: id.to_string(),
            size: meta.size,
            content_type: meta.content_type,
            created_at: meta.created_at,
            path: dir.join(CONTENT_DIR).join(&meta.filename),
            filename: meta.filename,
        })
    }

    pub async fn remove(&self, id: &str) -> Result<(), UploadError> {
        check_id(id)?;
        let dir = self.dir.join(id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                Err(UploadError::NotFound(id.to_string()))
            }
            Err(source) => Err(UploadError::Io {
                action: "remove",
                path: dir,
                source,
            }),
        }
    }

    pub async fn prune(&self, now: DateTime<Utc>) -> Result<usize, UploadError> {
        let mut entries =
            tokio::fs::read_dir(&self.dir)
                .await
                .map_err(|source| UploadError::Io {
                    action: "read",
                    path: self.dir.clone(),
                    source,
                })?;
        let mut removed = 0;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|source| UploadError::Io {
                action: "read",
                path: self.dir.clone(),
                source,
            })?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if check_id(&name).is_err() {
                continue;
            }
            let meta_path = entry.path().join(META_FILE);
            let meta = match tokio::fs::read_to_string(&meta_path).await {
                Ok(text) => serde_json::from_str::<UploadMeta>(&text),
                Err(source) => {
                    tracing::warn!(
                        error = %source,
                        path = %meta_path.display(),
                        "skipping staged upload with unreadable metadata"
                    );
                    continue;
                }
            };
            let meta = match meta {
                Ok(meta) => meta,
                Err(source) => {
                    tracing::warn!(
                        error = %source,
                        path = %meta_path.display(),
                        "skipping staged upload with malformed metadata"
                    );
                    continue;
                }
            };
            if now - meta.created_at <= RETENTION {
                continue;
            }
            match tokio::fs::remove_dir_all(entry.path()).await {
                Ok(()) => removed += 1,
                Err(source) => {
                    tracing::warn!(
                        error = %source,
                        path = %entry.path().display(),
                        "failed to remove expired staged upload"
                    );
                }
            }
        }
        Ok(removed)
    }

    async fn stage_at(
        &self,
        filename: &str,
        bytes: &[u8],
        now: DateTime<Utc>,
    ) -> Result<StagedUpload, UploadError> {
        validate_filename(filename)?;
        if bytes.is_empty() {
            return Err(UploadError::Empty);
        }
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(UploadError::TooLarge {
                size: bytes.len(),
                limit: MAX_UPLOAD_BYTES,
            });
        }
        if let Err(err) = self.prune(now).await {
            tracing::warn!(error = %err, "failed to prune expired staged uploads");
        }

        let id = uuid::Uuid::new_v4().simple().to_string();
        let staging = self.dir.join(format!("{STAGING_PREFIX}{id}"));
        let meta = UploadMeta {
            filename: filename.to_string(),
            size: bytes.len() as u64,
            content_type: detect_mime_type(Path::new(filename)),
            created_at: now,
        };
        let written = write_upload(&staging, &meta, bytes).await;
        if let Err(source) = written {
            if let Err(cleanup) = tokio::fs::remove_dir_all(&staging).await
                && cleanup.kind() != ErrorKind::NotFound
            {
                tracing::warn!(
                    error = %cleanup,
                    path = %staging.display(),
                    "failed to remove partly written upload"
                );
            }
            return Err(UploadError::Io {
                action: "write",
                path: staging,
                source,
            });
        }
        let final_dir = self.dir.join(&id);
        tokio::fs::rename(&staging, &final_dir)
            .await
            .map_err(|source| UploadError::Io {
                action: "write",
                path: final_dir.clone(),
                source,
            })?;
        Ok(StagedUpload {
            id,
            path: final_dir.join(CONTENT_DIR).join(&meta.filename),
            filename: meta.filename,
            size: meta.size,
            content_type: meta.content_type,
            created_at: meta.created_at,
        })
    }
}

async fn write_upload(dir: &Path, meta: &UploadMeta, bytes: &[u8]) -> std::io::Result<()> {
    tokio::fs::create_dir(dir).await?;
    let meta_json = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    tokio::fs::write(dir.join(META_FILE), meta_json).await?;
    let content_dir = dir.join(CONTENT_DIR);
    tokio::fs::create_dir(&content_dir).await?;
    tokio::fs::write(content_dir.join(&meta.filename), bytes).await
}

pub fn validate_filename(filename: &str) -> Result<(), UploadError> {
    let reason = if filename.trim().is_empty() {
        "is empty"
    } else if filename == "." || filename == ".." {
        "must name a file, not a directory"
    } else if filename.contains(['/', '\\']) {
        "must be a file name, without folders"
    } else if filename.chars().any(char::is_control) {
        "must not contain control characters"
    } else {
        return Ok(());
    };
    Err(UploadError::InvalidFilename { reason })
}

fn check_id(id: &str) -> Result<(), UploadError> {
    let valid = id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if valid {
        Ok(())
    } else {
        Err(UploadError::InvalidId(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, hour, 0, 0).unwrap()
    }

    fn store() -> (tempfile::TempDir, UploadStore) {
        let root = tempfile::tempdir().unwrap();
        let store = UploadStore::open(root.path()).unwrap();
        (root, store)
    }

    #[tokio::test]
    async fn staged_content_resolves_to_its_bytes() {
        let (_root, store) = store();
        let staged = store.stage_at("notes.txt", b"hello", at(9)).await.unwrap();
        assert_eq!(staged.filename, "notes.txt");
        assert_eq!(staged.size, 5);
        assert_eq!(staged.content_type, "text/plain");
        let resolved = store.resolve(&staged.id).await.unwrap();
        assert_eq!(resolved, staged);
        assert_eq!(
            resolved.path.file_name().and_then(|n| n.to_str()),
            Some("notes.txt"),
            "the content file carries the upload's name, so a copy by path keeps it"
        );
        let content = tokio::fs::read(&resolved.path).await.unwrap();
        assert_eq!(content, b"hello");
    }

    #[tokio::test]
    async fn an_unknown_or_malformed_id_is_a_typed_error() {
        let (_root, store) = store();
        let unknown = "0".repeat(32);
        assert!(matches!(
            store.resolve(&unknown).await,
            Err(UploadError::NotFound(_))
        ));
        assert!(matches!(
            store.resolve("../../etc").await,
            Err(UploadError::InvalidId(_))
        ));
        assert!(matches!(
            store.resolve(&"A".repeat(32)).await,
            Err(UploadError::InvalidId(_))
        ));
        assert!(matches!(
            store.remove(&unknown).await,
            Err(UploadError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn remove_takes_the_upload_away() {
        let (_root, store) = store();
        let staged = store.stage_at("a.bin", b"x", at(9)).await.unwrap();
        store.remove(&staged.id).await.unwrap();
        assert!(matches!(
            store.resolve(&staged.id).await,
            Err(UploadError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn prune_removes_only_uploads_older_than_retention() {
        let (_root, store) = store();
        let old = store.stage_at("old.txt", b"o", at(0)).await.unwrap();
        let fresh = store.stage_at("fresh.txt", b"f", at(12)).await.unwrap();

        let removed = store
            .prune(at(0) + RETENTION + Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert!(matches!(
            store.resolve(&old.id).await,
            Err(UploadError::NotFound(_))
        ));
        assert!(store.resolve(&fresh.id).await.is_ok());
    }

    #[tokio::test]
    async fn an_upload_exactly_at_retention_is_kept() {
        let (_root, store) = store();
        let staged = store.stage_at("edge.txt", b"e", at(0)).await.unwrap();
        let removed = store.prune(at(0) + RETENTION).await.unwrap();
        assert_eq!(removed, 0);
        assert!(store.resolve(&staged.id).await.is_ok());
    }

    #[tokio::test]
    async fn stage_prunes_expired_uploads_first() {
        let (_root, store) = store();
        let old = store.stage_at("old.txt", b"o", at(0)).await.unwrap();
        store
            .stage_at("new.txt", b"n", at(0) + RETENTION * 2)
            .await
            .unwrap();
        assert!(matches!(
            store.resolve(&old.id).await,
            Err(UploadError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn open_clears_staging_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let store = UploadStore::open(root.path()).unwrap();
        let leftover = root
            .path()
            .join(UPLOADS_DIR)
            .join(format!("{STAGING_PREFIX}abc"));
        tokio::fs::create_dir_all(&leftover).await.unwrap();
        drop(store);
        UploadStore::open(root.path()).unwrap();
        assert!(!leftover.exists());
    }

    #[test]
    fn filenames_must_be_base_names() {
        for name in ["", "   ", ".", "..", "a/b.txt", "..\\x", "bad\nname"] {
            assert!(
                matches!(
                    validate_filename(name),
                    Err(UploadError::InvalidFilename { .. })
                ),
                "{name:?} should be rejected"
            );
        }
        for name in ["report.pdf", "my file (2).png", ".hidden"] {
            assert!(
                validate_filename(name).is_ok(),
                "{name:?} should be accepted"
            );
        }
    }

    #[tokio::test]
    async fn empty_and_oversized_bodies_are_refused() {
        let (_root, store) = store();
        assert!(matches!(
            store.stage_at("a.txt", b"", at(0)).await,
            Err(UploadError::Empty)
        ));
        let big = vec![0_u8; MAX_UPLOAD_BYTES + 1];
        assert!(matches!(
            store.stage_at("a.bin", &big, at(0)).await,
            Err(UploadError::TooLarge { .. })
        ));
    }
}

//! Hub-staged uploads: files a client sends before the message that uses them.
//!
//! A client posts a file to `POST /api/hub/uploads` and gets an upload id back.
//! A later send (a chat message, or mail) names that id, and the sender moves
//! the staged file into its destination. The store lives in the hub directory
//! under `uploads/`, so it works while no agent runs. Each upload is one
//! directory: `<id>/content` holds the bytes and `<id>/meta.json` the filename,
//! size, content type, and creation time. Uploads older than
//! [`RETENTION`] are removed when the store prunes, which happens at startup
//! and before every stage.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use residuum_bus::attachment::detect_mime_type;

/// The largest upload the store accepts, in bytes.
pub const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// How long an unsent upload is kept before the store removes it.
pub const RETENTION: Duration = Duration::hours(24);

/// The directory under the hub directory that holds the uploads.
const UPLOADS_DIR: &str = "uploads";
/// The file holding an upload's bytes.
const CONTENT_FILE: &str = "content";
/// The sidecar file holding an upload's metadata.
const META_FILE: &str = "meta.json";
/// Prefix of the directory a new upload is written into before it is renamed
/// to its id. A leftover one is a crashed write and is removed at startup.
const STAGING_PREFIX: &str = ".staging-";

/// Staged uploads under one hub directory.
///
/// The store is cheap to clone: it holds only its directory. Every clone reads
/// and writes the same files, so the hub's routes and the mail and chat sends
/// share one store.
#[derive(Debug, Clone)]
pub struct UploadStore {
    dir: PathBuf,
}

/// An upload the store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpload {
    /// The id a send names to use the upload.
    pub id: String,
    /// The base name the client gave the file.
    pub filename: String,
    /// The size of the content in bytes.
    pub size: u64,
    /// The MIME type detected from the filename's extension.
    pub content_type: String,
    /// When the upload was staged.
    pub created_at: DateTime<Utc>,
    /// The file holding the content, for a sender to copy or move.
    pub path: PathBuf,
}

/// Why an upload operation failed. The messages are plain enough to show a
/// user; the `Io` variant carries the path and source for the log.
#[derive(Debug, Error)]
pub enum UploadError {
    /// The body had no bytes.
    #[error("the upload is empty")]
    Empty,
    /// The body is larger than [`MAX_UPLOAD_BYTES`].
    #[error("the upload is {size} bytes, more than the {limit}-byte limit")]
    TooLarge {
        /// The size of the body.
        size: usize,
        /// The limit it exceeds.
        limit: usize,
    },
    /// The filename is not a usable base name.
    #[error("the filename {reason}")]
    InvalidFilename {
        /// What is wrong with it, for the end of the sentence "the filename ...".
        reason: &'static str,
    },
    /// The id is not one the store issues, so no upload can have it.
    #[error("`{0}` is not an upload id")]
    InvalidId(String),
    /// No staged upload has the id.
    #[error("no staged upload has the id `{0}`")]
    NotFound(String),
    /// The store's files could not be read or written.
    #[error("failed to {action} the staged upload at {path}")]
    Io {
        /// What the store was doing, such as "write" or "read".
        action: &'static str,
        /// The file or directory the operation used.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
}

/// The sidecar file's contents.
#[derive(Debug, Serialize, Deserialize)]
struct UploadMeta {
    filename: String,
    size: u64,
    content_type: String,
    created_at: DateTime<Utc>,
}

impl UploadStore {
    /// Open the store under `hub_dir`, creating `uploads/` if it is missing.
    ///
    /// Removes the staging directories a crashed write left behind. Call it
    /// before the store is shared: it is not safe alongside a stage in flight.
    /// The hub prunes expired uploads separately, with [`Self::prune`].
    ///
    /// # Errors
    /// Returns [`UploadError::Io`] when the directory cannot be created or read.
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

    /// Store `bytes` as an upload named `filename`, pruning expired uploads
    /// first.
    ///
    /// # Errors
    /// Returns [`UploadError::InvalidFilename`] for a name that is not a base
    /// name, [`UploadError::Empty`] or [`UploadError::TooLarge`] for a body
    /// outside the limit, and [`UploadError::Io`] when the upload cannot be
    /// written. A failed prune is logged and does not fail the stage.
    pub async fn stage(&self, filename: &str, bytes: &[u8]) -> Result<StagedUpload, UploadError> {
        self.stage_at(filename, bytes, Utc::now()).await
    }

    /// The upload with `id`.
    ///
    /// # Errors
    /// Returns [`UploadError::InvalidId`] for an id the store never issues,
    /// [`UploadError::NotFound`] when no upload has it, and
    /// [`UploadError::Io`] when its metadata cannot be read.
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
            path: meta_path,
            source: std::io::Error::new(ErrorKind::InvalidData, source),
        })?;
        Ok(StagedUpload {
            id: id.to_string(),
            filename: meta.filename,
            size: meta.size,
            content_type: meta.content_type,
            created_at: meta.created_at,
            path: dir.join(CONTENT_FILE),
        })
    }

    /// Remove the upload with `id`, once a sender has copied or moved its
    /// content.
    ///
    /// # Errors
    /// Returns [`UploadError::InvalidId`] or [`UploadError::NotFound`] as
    /// [`Self::resolve`] does, and [`UploadError::Io`] when the directory cannot
    /// be removed.
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

    /// Remove every upload staged more than [`RETENTION`] before `now`, and
    /// return how many were removed.
    ///
    /// An upload whose metadata is unreadable is logged and kept: removing it
    /// could drop one that a concurrent stage is still writing.
    ///
    /// # Errors
    /// Returns [`UploadError::Io`] when the store directory cannot be read. A
    /// directory that fails to remove is logged and skipped, so one bad entry
    /// does not stop the rest.
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

    /// [`Self::stage`] with the creation time given, so tests need not touch
    /// the clock.
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
            filename: meta.filename,
            size: meta.size,
            content_type: meta.content_type,
            created_at: meta.created_at,
            path: final_dir.join(CONTENT_FILE),
        })
    }
}

/// Write the content and metadata into the staging directory `dir`.
async fn write_upload(dir: &Path, meta: &UploadMeta, bytes: &[u8]) -> std::io::Result<()> {
    tokio::fs::create_dir(dir).await?;
    let meta_json = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    tokio::fs::write(dir.join(META_FILE), meta_json).await?;
    tokio::fs::write(dir.join(CONTENT_FILE), bytes).await
}

/// Accept `filename` only when it is a base name: non-empty, not `.` or `..`,
/// with no path separators and no control characters.
///
/// # Errors
/// Returns [`UploadError::InvalidFilename`] with the reason otherwise.
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

/// Accept `id` only when it is the 32-character lowercase hex form the store
/// issues, so no id can name a path outside the store.
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

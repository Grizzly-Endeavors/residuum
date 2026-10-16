//! Files a chat interface sends out: their metadata and MIME type detection.

use std::path::{Path, PathBuf};

/// Detect MIME type from a file's extension.
///
/// Uses `mime_guess` for known extensions, falls back to `application/octet-stream`.
#[must_use]
pub fn detect_mime_type(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Metadata for an outbound file attachment.
#[derive(Debug, Clone)]
pub struct FileAttachment {
    /// Absolute path to the file on disk.
    pub path: PathBuf,
    /// Original filename (from the path).
    pub filename: String,
    /// Detected MIME type.
    pub mime_type: String,
    /// File size in bytes.
    pub size: u64,
}

impl FileAttachment {
    /// Create a `FileAttachment` by validating and inspecting a file path.
    ///
    /// # Errors
    ///
    /// Returns a descriptive error string if the file does not exist or is not readable.
    pub async fn from_path(path: &Path) -> Result<Self, String> {
        let metadata = tokio::fs::metadata(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("file not found: {}", path.display())
            } else {
                format!("file not readable: {}", path.display())
            }
        })?;

        let filename = path.file_name().map_or_else(
            || "unnamed".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        let mime_type = detect_mime_type(path);
        let size = metadata.len();

        Ok(Self {
            path: path.to_path_buf(),
            filename,
            mime_type,
            size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mime_known_types() {
        assert_eq!(detect_mime_type(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(detect_mime_type(Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(detect_mime_type(Path::new("image.png")), "image/png");
        assert_eq!(detect_mime_type(Path::new("anim.gif")), "image/gif");
        assert_eq!(detect_mime_type(Path::new("pic.webp")), "image/webp");
        assert_eq!(detect_mime_type(Path::new("song.mp3")), "audio/mpeg");
        assert_eq!(detect_mime_type(Path::new("voice.ogg")), "audio/ogg");
        assert_eq!(detect_mime_type(Path::new("clip.wav")), "audio/wav");
        assert_eq!(detect_mime_type(Path::new("note.m4a")), "audio/m4a");
        assert_eq!(detect_mime_type(Path::new("doc.pdf")), "application/pdf");
    }

    #[test]
    fn detect_mime_unknown_extension() {
        // mime_guess doesn't know .zzz — falls back to application/octet-stream
        assert_eq!(
            detect_mime_type(Path::new("data.zzz")),
            "application/octet-stream"
        );
    }

    #[test]
    fn detect_mime_no_extension() {
        assert_eq!(
            detect_mime_type(Path::new("README")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn file_attachment_from_path_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("report.pdf");
        tokio::fs::write(&file_path, b"fake pdf content")
            .await
            .unwrap();

        let attachment = FileAttachment::from_path(&file_path).await.unwrap();
        assert_eq!(attachment.filename, "report.pdf");
        assert_eq!(attachment.mime_type, "application/pdf");
        assert_eq!(attachment.size, 16); // b"fake pdf content".len()
        assert_eq!(attachment.path, file_path);
    }

    #[tokio::test]
    async fn file_attachment_from_path_not_found() {
        let result =
            FileAttachment::from_path(Path::new("/tmp/nonexistent_residuum_test.pdf")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("file not found"),
            "error should mention not found: {err}"
        );
    }
}

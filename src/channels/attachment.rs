//! Attachment downloading and metadata formatting for Discord messages.
//!
//! Feature-gated behind `--features discord`. Downloads attachments to the
//! workspace inbox directory and formats metadata lines for the agent.

use std::path::{Path, PathBuf};

/// Maximum attachment size in bytes (25 MB — Discord's own limit).
const MAX_ATTACHMENT_SIZE: u32 = 25 * 1024 * 1024;

/// Metadata about an attachment from an incoming message.
pub struct AttachmentInfo {
    /// Original filename.
    pub filename: String,
    /// Download URL.
    pub url: String,
    /// File size in bytes.
    pub size: u32,
    /// MIME content type, if known.
    pub content_type: Option<String>,
}

/// Result of a successful attachment download.
#[derive(Debug)]
pub struct SavedAttachment {
    /// Local path where the file was saved.
    pub local_path: PathBuf,
}

/// Download an attachment to the inbox directory.
///
/// Files are saved as `{timestamp}_{filename}` to prevent collisions.
/// Attachments larger than 25 MB are skipped.
///
/// # Errors
///
/// Returns an error if the download or file write fails.
pub async fn download_attachment(
    info: &AttachmentInfo,
    inbox_dir: &Path,
) -> Result<SavedAttachment, String> {
    if info.size > MAX_ATTACHMENT_SIZE {
        return Err(format!(
            "attachment '{}' exceeds 25 MB limit ({} bytes)",
            info.filename, info.size,
        ));
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let saved_name = format!("{timestamp}_{}", info.filename);
    let local_path = inbox_dir.join(&saved_name);

    let response = reqwest::get(&info.url)
        .await
        .map_err(|e| format!("failed to download attachment '{}': {e}", info.filename,))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("failed to read attachment body '{}': {e}", info.filename,))?;

    tokio::fs::write(&local_path, &bytes).await.map_err(|e| {
        format!(
            "failed to save attachment '{}' to {}: {e}",
            info.filename,
            local_path.display(),
        )
    })?;

    Ok(SavedAttachment { local_path })
}

/// Format a metadata line for a successfully saved attachment.
#[must_use]
pub fn format_attachment_line(saved: &SavedAttachment, info: &AttachmentInfo) -> String {
    format!(
        "[Attachment: {} ({} bytes) \u{2192} {}]",
        info.filename,
        info.size,
        saved.local_path.display(),
    )
}

/// Format a metadata line for a failed attachment download.
#[must_use]
pub fn format_failed_attachment_line(info: &AttachmentInfo, reason: &str) -> String {
    format!(
        "[Attachment: {} ({} bytes) \u{2014} download failed: {reason}]",
        info.filename, info.size,
    )
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn format_attachment_line_output() {
        let saved = SavedAttachment {
            local_path: PathBuf::from("/workspace/inbox/20260222_120000_photo.jpg"),
        };
        let info = AttachmentInfo {
            filename: "photo.jpg".to_string(),
            url: String::new(),
            size: 1024,
            content_type: Some("image/jpeg".to_string()),
        };
        let line = format_attachment_line(&saved, &info);
        assert!(
            line.contains("photo.jpg"),
            "should contain filename: {line}"
        );
        assert!(line.contains("1024 bytes"), "should contain size: {line}");
        assert!(
            line.contains("/workspace/inbox/20260222_120000_photo.jpg"),
            "should contain path: {line}"
        );
    }

    #[test]
    fn format_failed_line_output() {
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            url: String::new(),
            size: 2048,
            content_type: None,
        };
        let line = format_failed_attachment_line(&info, "timeout");
        assert!(line.contains("doc.pdf"), "should contain filename: {line}");
        assert!(
            line.contains("download failed: timeout"),
            "should contain reason: {line}"
        );
    }

    #[tokio::test]
    async fn download_to_temp_dir() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world"))
            .mount(&mock_server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "test.txt".to_string(),
            url: format!("{}/file", mock_server.uri()),
            size: 11,
            content_type: Some("text/plain".to_string()),
        };

        let saved = download_attachment(&info, dir.path()).await.unwrap();
        assert!(saved.local_path.exists(), "file should exist on disk");

        let content = tokio::fs::read_to_string(&saved.local_path).await.unwrap();
        assert_eq!(content, "hello world", "content should match");
    }

    #[tokio::test]
    async fn skip_oversized_attachment() {
        let info = AttachmentInfo {
            filename: "huge.bin".to_string(),
            url: String::new(),
            size: MAX_ATTACHMENT_SIZE + 1,
            content_type: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let result = download_attachment(&info, dir.path()).await;
        assert!(result.is_err(), "should reject oversized attachment");
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds 25 MB"),
            "error should mention size limit: {err}"
        );
    }
}

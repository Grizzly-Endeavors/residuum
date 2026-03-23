//! Attachment downloading and metadata formatting for channel messages.
//!
//! Downloads attachments to the workspace inbox directory and formats metadata
//! lines for the agent. Supported image formats are base64-encoded for inline
//! delivery to the model.

use std::path::{Path, PathBuf};

use base64::Engine;

use crate::models::ImageData;

/// Maximum attachment size in bytes (25 MB — Discord's own limit).
const MAX_ATTACHMENT_SIZE: u32 = 25 * 1024 * 1024;

/// Images larger than 20 MB are saved but not sent inline to the model.
pub const MAX_IMAGE_INLINE_SIZE: u32 = 20 * 1024 * 1024;

/// MIME types that can be sent inline to the model as images.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Check if a content type is a supported image format for inline encoding.
#[must_use]
pub fn is_supported_image(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| SUPPORTED_IMAGE_TYPES.contains(&ct))
}

/// Base64-encode an image file from disk.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub async fn encode_image_from_file(path: &Path, media_type: &str) -> Result<ImageData, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("failed to read image {}: {e}", path.display()))?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(ImageData {
        media_type: media_type.to_string(),
        data,
    })
}

/// Metadata about an attachment from an incoming message.
pub struct AttachmentInfo {
    /// Original filename.
    pub filename: String,
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
    url: &str,
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

    let response = reqwest::get(url)
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

/// Finalize a downloaded attachment: append metadata to content, optionally encode inline,
/// and create a companion inbox item.
///
/// `platform` is the display name of the interface (e.g. `"Discord"`, `"Telegram"`); the inbox
/// item title is `"{platform} attachment: {filename}"` and the source field is its lowercase form.
///
/// Returns an `ImageData` if the attachment is a supported image within the inline size limit.
pub async fn finalize_attachment(
    saved: &SavedAttachment,
    info: &AttachmentInfo,
    content: &mut String,
    author: &str,
    inbox_dir: &Path,
    tz: chrono_tz::Tz,
    platform: &str,
) -> Option<ImageData> {
    let line = format_attachment_line(saved, info);
    content.push('\n');
    content.push_str(&line);

    let image = match info.content_type.as_deref() {
        Some(ct) if is_supported_image(Some(ct)) => {
            if info.size <= MAX_IMAGE_INLINE_SIZE {
                match encode_image_from_file(&saved.local_path, ct).await {
                    Ok(img) => Some(img),
                    Err(e) => {
                        tracing::warn!(
                            filename = %info.filename,
                            error = %e,
                            "failed to encode attachment image for inline delivery"
                        );
                        None
                    }
                }
            } else {
                tracing::warn!(
                    filename = %info.filename,
                    size = info.size,
                    max = MAX_IMAGE_INLINE_SIZE,
                    "attachment image exceeds inline size limit, saved but not sent to model"
                );
                None
            }
        }
        _ => None,
    };

    let Some(file_name_os) = saved.local_path.file_name() else {
        tracing::warn!(
            path = %saved.local_path.display(),
            "attachment path has no filename, skipping companion item"
        );
        return image;
    };
    let saved_name = file_name_os.to_string_lossy().to_string();
    let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
    let companion = crate::inbox::InboxItem {
        title: format!("{platform} attachment: {}", info.filename),
        body: format!(
            "From: {author}\nSize: {} bytes\nContent-Type: {content_type_str}",
            info.size,
        ),
        source: platform.to_lowercase(),
        timestamp: crate::time::now_local(tz),
        read: false,
        attachments: vec![PathBuf::from("inbox").join(&saved_name)],
    };
    let filename = crate::inbox::generate_filename(&companion.title, companion.timestamp);
    if let Err(e) = crate::inbox::save_item(inbox_dir, &filename, &companion).await {
        tracing::warn!(
            filename = %info.filename,
            error = %e,
            "failed to create companion inbox item for attachment"
        );
    }

    image
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
        let url = format!("{}/file", mock_server.uri());
        let info = AttachmentInfo {
            filename: "test.txt".to_string(),
            size: 11,
            content_type: Some("text/plain".to_string()),
        };

        let saved = download_attachment(&info, &url, dir.path()).await.unwrap();
        assert!(saved.local_path.exists(), "file should exist on disk");

        let content = tokio::fs::read_to_string(&saved.local_path).await.unwrap();
        assert_eq!(content, "hello world", "content should match");
    }

    #[tokio::test]
    async fn skip_oversized_attachment() {
        let info = AttachmentInfo {
            filename: "huge.bin".to_string(),
            size: MAX_ATTACHMENT_SIZE + 1,
            content_type: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let result = download_attachment(&info, "", dir.path()).await;
        assert!(result.is_err(), "should reject oversized attachment");
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds 25 MB"),
            "error should mention size limit: {err}"
        );
    }

    #[test]
    fn is_supported_image_types() {
        assert!(
            is_supported_image(Some("image/jpeg")),
            "jpeg should be supported"
        );
        assert!(
            is_supported_image(Some("image/png")),
            "png should be supported"
        );
        assert!(
            is_supported_image(Some("image/gif")),
            "gif should be supported"
        );
        assert!(
            is_supported_image(Some("image/webp")),
            "webp should be supported"
        );
        assert!(
            !is_supported_image(Some("application/pdf")),
            "pdf should not be supported"
        );
        assert!(
            !is_supported_image(None),
            "None content type should not be supported"
        );
    }

    #[tokio::test]
    async fn encode_image_from_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jpg");
        tokio::fs::write(&path, b"fake image bytes").await.unwrap();

        let img = encode_image_from_file(&path, "image/jpeg").await.unwrap();
        assert_eq!(img.media_type, "image/jpeg", "media type should match");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .unwrap();
        assert_eq!(
            decoded, b"fake image bytes",
            "decoded base64 should equal original bytes"
        );
    }

    #[tokio::test]
    async fn encode_image_from_file_missing_path() {
        let path = std::path::Path::new("/tmp/nonexistent_image_test_file_xyz.jpg");
        let result = encode_image_from_file(path, "image/jpeg").await;
        assert!(result.is_err(), "missing file should return error");
        let err = result.unwrap_err();
        assert!(
            err.contains("nonexistent_image_test_file_xyz.jpg"),
            "error should contain the path: {err}"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_inline_image() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let image_path = dir.path().join("photo.jpg");
        tokio::fs::write(&image_path, b"fake image bytes")
            .await
            .unwrap();

        let saved = SavedAttachment {
            local_path: image_path,
        };
        let info = AttachmentInfo {
            filename: "photo.jpg".to_string(),
            size: 1024,
            content_type: Some("image/jpeg".to_string()),
        };
        let mut content = String::new();

        let result = finalize_attachment(
            &saved,
            &info,
            &mut content,
            "author",
            &inbox_dir,
            chrono_tz::UTC,
            "Discord",
        )
        .await;

        assert!(
            result.is_some(),
            "supported image within size limit should return ImageData"
        );
        let img = result.unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .unwrap();
        assert_eq!(
            decoded, b"fake image bytes",
            "decoded base64 should equal original bytes"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_oversized_image_skips_inline() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let image_path = dir.path().join("large.jpg");
        tokio::fs::write(&image_path, b"bytes").await.unwrap();

        let saved = SavedAttachment {
            local_path: image_path,
        };
        let info = AttachmentInfo {
            filename: "large.jpg".to_string(),
            size: MAX_IMAGE_INLINE_SIZE + 1,
            content_type: Some("image/jpeg".to_string()),
        };
        let mut content = String::new();

        let result = finalize_attachment(
            &saved,
            &info,
            &mut content,
            "author",
            &inbox_dir,
            chrono_tz::UTC,
            "Discord",
        )
        .await;

        assert!(
            result.is_none(),
            "image exceeding inline size limit should not be encoded"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_non_image_skips_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let file_path = dir.path().join("doc.pdf");
        tokio::fs::write(&file_path, b"pdf content").await.unwrap();

        let saved = SavedAttachment {
            local_path: file_path,
        };
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 1024,
            content_type: Some("application/pdf".to_string()),
        };
        let mut content = String::new();

        let result = finalize_attachment(
            &saved,
            &info,
            &mut content,
            "author",
            &inbox_dir,
            chrono_tz::UTC,
            "Discord",
        )
        .await;

        assert!(
            result.is_none(),
            "non-image content type should not be encoded"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_no_path_filename_skips_inbox_item() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let saved = SavedAttachment {
            local_path: PathBuf::from("/"),
        };
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 100,
            content_type: Some("application/pdf".to_string()),
        };
        let mut content = String::new();

        let result = finalize_attachment(
            &saved,
            &info,
            &mut content,
            "author",
            &inbox_dir,
            chrono_tz::UTC,
            "Discord",
        )
        .await;

        assert!(
            result.is_none(),
            "non-image with no path filename should return None"
        );
        let entries: Vec<_> = std::fs::read_dir(&inbox_dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no inbox item should be created when path has no filename"
        );
    }
}

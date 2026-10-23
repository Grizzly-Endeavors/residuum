//! Attachment downloading and metadata formatting for channel messages.
//!
//! Each inbound attachment is filed under a companion agent-inbox item, in that
//! item's own directory: `inbox/agent/attachments/<item id>/<filename>`. The item
//! is reserved first ([`reserve_attachment_item`]) so its ID is known before the
//! file is written; the file is then saved into [`AttachmentItem::attachments_dir`]
//! and finalized into the item. Supported image formats are base64-encoded for
//! inline delivery to the model.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::NaiveDateTime;

use residuum_inference::ImageData;

use super::{agent_inbox_attachments_rel, dedupe_filename, sanitize_attachment_filename};

/// Images larger than 20 MB are saved but not sent inline to the model.
pub const MAX_IMAGE_INLINE_SIZE: u32 = 20 * 1024 * 1024;

/// MIME types that can be sent inline to the model as images.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Check if a content type is a supported image format for inline encoding.
#[must_use]
pub fn is_supported_image(content_type: &str) -> bool {
    SUPPORTED_IMAGE_TYPES.contains(&content_type)
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

/// The companion inbox item an attachment is filed under.
///
/// Reserved by [`reserve_attachment_item`] before the file is written, so the
/// file can go into the item's own attachment directory. Nothing is written to
/// the inbox until [`finalize_attachment`] saves the item.
#[derive(Debug)]
pub struct AttachmentItem {
    inbox_dir: PathBuf,
    /// Filename stem of the item, which also names its attachment directory.
    id: String,
    /// The item's JSON filename (`<id>.json`).
    filename: String,
    title: String,
    source: String,
    timestamp: NaiveDateTime,
    attachments_dir: PathBuf,
}

impl AttachmentItem {
    /// Directory the item's attachment file is saved into: `<inbox>/attachments/<id>/`.
    #[must_use]
    pub fn attachments_dir(&self) -> &Path {
        &self.attachments_dir
    }
}

/// Reserve the companion inbox item for an attachment arriving on `platform`.
///
/// Picks an item filename that is unused in `inbox_dir` and its archive, and the
/// attachment directory the file belongs in. Nothing is written to disk here, so
/// a download that fails leaves no trace.
pub async fn reserve_attachment_item(
    inbox_dir: &Path,
    info: &AttachmentInfo,
    platform: &str,
    tz: chrono_tz::Tz,
) -> AttachmentItem {
    let timestamp = residuum_core::time::now_local(tz);
    let title = format!("{platform} attachment: {}", info.filename);
    let filename = crate::inbox::unique_filename(inbox_dir, &title, timestamp).await;
    let id = filename.trim_end_matches(".json").to_string();
    let attachments_dir = inbox_dir.join("attachments").join(&id);
    AttachmentItem {
        inbox_dir: inbox_dir.to_path_buf(),
        id,
        filename,
        title,
        source: platform.to_lowercase(),
        timestamp,
        attachments_dir,
    }
}

/// Download an attachment into `dir`.
///
/// The file keeps its sanitized name, with `_2`, `_3`, ... appended when `dir`
/// already holds a file of that name. There is no size cap here: the file already
/// exists on the sending platform (which enforced its own upload limit, if any,
/// when it was sent), so downloading it is just an HTTP fetch with no platform
/// limit of its own.
///
/// # Errors
///
/// Returns an error if the download or file write fails.
pub async fn download_attachment(
    info: &AttachmentInfo,
    url: &str,
    dir: &Path,
) -> Result<SavedAttachment, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("failed to download attachment '{}': {e}", info.filename))?
        .error_for_status()
        .map_err(|e| format!("failed to download attachment '{}': {e}", info.filename))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("failed to read attachment body '{}': {e}", info.filename))?;

    save_attachment_bytes(info, &bytes, dir).await
}

/// Save already-downloaded attachment bytes into `dir`.
///
/// For interfaces whose files need an authenticated fetch the caller performs.
/// Names the file as [`download_attachment`] does, and likewise has no size cap
/// of its own.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or the file write fails.
pub async fn save_attachment_bytes(
    info: &AttachmentInfo,
    bytes: &[u8],
    dir: &Path,
) -> Result<SavedAttachment, String> {
    let local_path = unique_attachment_path(dir, &info.filename).await?;

    tokio::fs::write(&local_path, bytes).await.map_err(|e| {
        format!(
            "failed to save attachment '{}' to {}: {e}",
            info.filename,
            local_path.display(),
        )
    })?;

    Ok(SavedAttachment { local_path })
}

/// Choose the path a file called `filename` is saved to in `dir`, creating `dir`
/// if needed.
///
/// The name is sanitized, then `_2`, `_3`, ... is appended before the extension
/// while `dir` already holds a file of that name.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or read.
pub async fn unique_attachment_path(dir: &Path, filename: &str) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("failed to create attachment dir {}: {e}", dir.display()))?;

    let mut existing = HashSet::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| format!("failed to read attachment dir {}: {e}", dir.display()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("failed to read attachment dir {}: {e}", dir.display()))?
    {
        existing.insert(entry.file_name().to_string_lossy().into_owned());
    }

    let name = dedupe_filename(&sanitize_attachment_filename(filename), &existing);
    Ok(dir.join(name))
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

/// Finalize a saved attachment: append metadata to content, optionally encode inline,
/// and save its companion inbox item.
///
/// `saved` must be a file saved into `item.attachments_dir()`; the companion item
/// records it at its workspace-relative path there. `author` is the sender named
/// in the item body.
///
/// Returns an `ImageData` if the attachment is a supported image within the inline size limit.
pub async fn finalize_attachment(
    saved: &SavedAttachment,
    info: &AttachmentInfo,
    content: &mut String,
    author: &str,
    item: AttachmentItem,
) -> Option<ImageData> {
    let line = format_attachment_line(saved, info);
    content.push('\n');
    content.push_str(&line);

    let image = match info.content_type.as_deref() {
        Some(ct) if is_supported_image(ct) => {
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
    let file_name = file_name_os.to_string_lossy().to_string();
    let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
    let companion = crate::inbox::InboxItem {
        title: item.title.clone(),
        body: format!(
            "From: {author}\nSize: {} bytes\nContent-Type: {content_type_str}",
            info.size,
        ),
        source: item.source.clone(),
        timestamp: item.timestamp,
        read: false,
        attachments: vec![agent_inbox_attachments_rel().join(&item.id).join(file_name)],
        in_reply_to: None,
    };
    if let Err(e) = crate::inbox::save_item(&item.inbox_dir, &item.filename, &companion).await {
        tracing::warn!(
            filename = %info.filename,
            error = %e,
            "failed to create companion inbox item for attachment"
        );
    }

    image
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    use residuum_config::WorkspaceLayout;

    /// Reserve a companion item under `root`'s agent inbox and save `bytes` into it.
    async fn reserve_and_save(
        root: &Path,
        info: &AttachmentInfo,
        bytes: &[u8],
    ) -> (AttachmentItem, SavedAttachment) {
        let inbox_dir = WorkspaceLayout::new(root).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let item = reserve_attachment_item(&inbox_dir, info, "Discord", chrono_tz::UTC).await;
        let saved = save_attachment_bytes(info, bytes, item.attachments_dir())
            .await
            .unwrap();
        (item, saved)
    }

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
    async fn large_attachment_is_not_rejected_for_its_size() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Downloading an attachment is just an HTTP fetch — the sending
        // platform already enforced its own upload limit, if any. There is
        // no size cap here, unlike the old 25 MB blanket refusal.
        let mock_server = MockServer::start().await;
        let body = vec![0_u8; 30 * 1024 * 1024];
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&mock_server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/file", mock_server.uri());
        let info = AttachmentInfo {
            filename: "huge.bin".to_string(),
            size: 30 * 1024 * 1024,
            content_type: None,
        };
        let saved = download_attachment(&info, &url, dir.path())
            .await
            .expect("a large attachment should be accepted");
        let saved_len = tokio::fs::metadata(&saved.local_path).await.unwrap().len();
        assert_eq!(saved_len, body.len() as u64);
    }

    #[test]
    fn is_supported_image_types() {
        assert!(is_supported_image("image/jpeg"), "jpeg should be supported");
        assert!(is_supported_image("image/png"), "png should be supported");
        assert!(is_supported_image("image/gif"), "gif should be supported");
        assert!(is_supported_image("image/webp"), "webp should be supported");
        assert!(
            !is_supported_image("application/pdf"),
            "pdf should not be supported"
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
    async fn encode_image_from_file_error_on_missing_file() {
        let result = encode_image_from_file(
            std::path::Path::new("/tmp/nonexistent_residuum_test_image.jpg"),
            "image/jpeg",
        )
        .await;
        assert!(result.is_err(), "should fail for non-existent file");
        let err = result.unwrap_err();
        assert!(
            err.contains("nonexistent_residuum_test_image.jpg"),
            "error should contain the path: {err}"
        );
    }

    #[tokio::test]
    async fn download_attachment_returns_error_on_http_500() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/file", mock_server.uri());
        let info = AttachmentInfo {
            filename: "broken.jpg".to_string(),
            size: 100,
            content_type: Some("image/jpeg".to_string()),
        };

        let result = download_attachment(&info, &url, dir.path()).await;
        assert!(result.is_err(), "500 response should return error");
        let err = result.unwrap_err();
        assert!(
            err.contains("broken.jpg"),
            "error should contain filename: {err}"
        );
    }

    #[tokio::test]
    async fn companion_item_records_attachment_path_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 9,
            content_type: Some("application/pdf".to_string()),
        };
        let (item, saved) = reserve_and_save(dir.path(), &info, b"pdf bytes").await;
        let mut content = String::new();
        let image = finalize_attachment(&saved, &info, &mut content, "author", item).await;
        assert!(image.is_none(), "pdf should not be encoded inline");

        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1, "one companion item should be saved");
        let (_, companion) = &items[0];
        assert_eq!(
            companion.attachments.len(),
            1,
            "item should record its file"
        );

        let recorded = &companion.attachments[0];
        assert!(
            recorded.starts_with("inbox/agent/attachments"),
            "recorded path should be workspace-relative: {}",
            recorded.display()
        );
        assert_eq!(
            dir.path().join(recorded),
            saved.local_path,
            "recorded path should name the saved file"
        );
        assert!(
            saved.local_path.is_file(),
            "the recorded file should exist on disk"
        );
    }

    #[tokio::test]
    async fn attachments_with_same_filename_on_one_item_are_deduped() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "notes.txt".to_string(),
            size: 3,
            content_type: None,
        };
        let item_dir = dir.path().join("inbox/agent/attachments/item1");

        let first = save_attachment_bytes(&info, b"one", &item_dir)
            .await
            .unwrap();
        let second = save_attachment_bytes(&info, b"two", &item_dir)
            .await
            .unwrap();

        assert_eq!(first.local_path, item_dir.join("notes.txt"));
        assert_eq!(second.local_path, item_dir.join("notes_2.txt"));
        assert_eq!(
            tokio::fs::read_to_string(&first.local_path).await.unwrap(),
            "one",
            "the first file should be untouched"
        );
        assert_eq!(
            tokio::fs::read_to_string(&second.local_path).await.unwrap(),
            "two",
            "the second file should hold its own bytes"
        );
    }

    #[tokio::test]
    async fn archiving_companion_item_moves_its_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let inbox_dir = layout.agent_inbox_dir();
        let archive_dir = layout.agent_inbox_archive_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let info = AttachmentInfo {
            filename: "photo.png".to_string(),
            size: 4,
            content_type: Some("image/png".to_string()),
        };
        let item = reserve_attachment_item(&inbox_dir, &info, "Telegram", chrono_tz::UTC).await;
        let saved = save_attachment_bytes(&info, b"png!", item.attachments_dir())
            .await
            .unwrap();
        let mut content = String::new();
        finalize_attachment(&saved, &info, &mut content, "author", item).await;

        let (id, _) = crate::inbox::list_items(&inbox_dir)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("companion item should be saved");
        let json_name = format!("{id}.json");
        let file_name = saved.local_path.file_name().unwrap().to_owned();

        crate::inbox::archive_item(&inbox_dir, &archive_dir, &json_name)
            .await
            .unwrap();
        assert!(
            !saved.local_path.exists(),
            "the attachment should leave the active inbox with its item"
        );
        let archived_file = layout
            .agent_inbox_archive_attachments_dir()
            .join(&id)
            .join(&file_name);
        assert!(
            archived_file.is_file(),
            "the attachment should move to the archive attachments dir"
        );

        crate::inbox::restore_item(&archive_dir, &inbox_dir, &json_name)
            .await
            .unwrap();
        assert!(
            saved.local_path.is_file(),
            "restoring the item should move its attachment back"
        );
        assert!(
            !archived_file.exists(),
            "the archived copy should be gone after restore"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_inline_image() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "photo.jpg".to_string(),
            size: 1024,
            content_type: Some("image/jpeg".to_string()),
        };
        let (item, saved) = reserve_and_save(dir.path(), &info, b"fake image bytes").await;
        let mut content = String::new();

        let result = finalize_attachment(&saved, &info, &mut content, "author", item).await;

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
        assert!(
            content.contains("photo.jpg"),
            "content should contain filename"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_oversized_image_skips_inline() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "large.jpg".to_string(),
            size: MAX_IMAGE_INLINE_SIZE + 1,
            content_type: Some("image/jpeg".to_string()),
        };
        let (item, saved) = reserve_and_save(dir.path(), &info, b"bytes").await;
        let mut content = String::new();

        let result = finalize_attachment(&saved, &info, &mut content, "author", item).await;

        assert!(
            result.is_none(),
            "image exceeding inline size limit should not be encoded"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_non_image_skips_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 1024,
            content_type: Some("application/pdf".to_string()),
        };
        let (item, saved) = reserve_and_save(dir.path(), &info, b"pdf content").await;
        let mut content = String::new();

        let result = finalize_attachment(&saved, &info, &mut content, "author", item).await;

        assert!(
            result.is_none(),
            "non-image content type should not be encoded"
        );
    }

    #[tokio::test]
    async fn finalize_attachment_no_path_filename_skips_inbox_item() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 100,
            content_type: Some("application/pdf".to_string()),
        };
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let item = reserve_attachment_item(&inbox_dir, &info, "Discord", chrono_tz::UTC).await;
        let saved = SavedAttachment {
            local_path: PathBuf::from("/"),
        };
        let mut content = String::new();

        let result = finalize_attachment(&saved, &info, &mut content, "author", item).await;

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

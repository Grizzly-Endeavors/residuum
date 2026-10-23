//! Attachment downloading and metadata formatting for channel messages.
//!
//! Each inbound attachment is filed under a companion agent-inbox item, in that
//! item's own directory: `inbox/agent/attachments/<item id>/<filename>`.
//! [`reserve_attachment_item`] claims that directory on disk (a [`ClaimedItem`])
//! before the download starts, and every save goes through the [`AttachmentItem`],
//! so a file can only land in its item's directory. A caller whose save fails
//! calls [`AttachmentItem::discard`]; an item dropped without being finalized or
//! discarded releases its claim too. Supported image formats are base64-encoded
//! for inline delivery to the model.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::NaiveDateTime;
use tokio::io::AsyncWriteExt;

use residuum_inference::ImageData;

use super::{ClaimedItem, dedupe_filename, sanitize_attachment_filename};

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
    /// File size in bytes, as the sending platform reported it. Zero when the
    /// platform gave no size; the saved file's real size is on [`SavedAttachment`].
    pub size: u32,
    /// MIME content type, if known.
    pub content_type: Option<String>,
}

/// What an attached file is, which sets the word its companion item is titled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// A file sent with a message: titled `"{platform} attachment: {filename}"`.
    Attachment,
    /// A file a remote agent returned for a task it was given: titled
    /// `"{platform} artifact: {filename}"`.
    Artifact,
}

impl AttachmentKind {
    fn title_word(self) -> &'static str {
        match self {
            Self::Attachment => "attachment",
            Self::Artifact => "artifact",
        }
    }
}

/// A file saved into an [`AttachmentItem`]'s directory.
///
/// Only the item's own save methods build one, so a saved file always sits in
/// the directory of the item it is finalized into.
#[derive(Debug)]
pub struct SavedAttachment {
    local_path: PathBuf,
    /// The file's name in the item's directory (the last component of `local_path`).
    file_name: String,
    size: u32,
}

impl SavedAttachment {
    /// Where the file was saved.
    #[must_use]
    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    /// The number of bytes written to the file.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// The companion inbox item an attachment is filed under.
///
/// Reserved by [`reserve_attachment_item`], which claims the item's id and
/// attachment directory on disk. The item's JSON is written only by finalizing
/// it, after the file is saved. A save that fails leaves the directory claimed
/// until the caller calls [`AttachmentItem::discard`] (or drops the item).
#[derive(Debug)]
pub struct AttachmentItem {
    claim: ClaimedItem,
    title: String,
    source: String,
    timestamp: NaiveDateTime,
}

/// A file being streamed into an [`AttachmentItem`]'s directory, for downloads
/// that write as they arrive rather than in one buffer.
pub struct AttachmentWriter {
    file: tokio::fs::File,
    local_path: PathBuf,
    file_name: String,
}

impl AttachmentWriter {
    /// The file to write into.
    pub fn file_mut(&mut self) -> &mut tokio::fs::File {
        &mut self.file
    }

    /// Flush the written bytes to disk and return the saved file.
    ///
    /// # Errors
    /// Returns an error if the flush or the size lookup fails.
    pub async fn finish(mut self) -> Result<SavedAttachment, String> {
        // `tokio::fs::File` queues writes on a blocking thread and dropping it
        // does not wait for them; flush so the file is complete on disk before
        // anything reads it back.
        self.file.flush().await.map_err(|e| {
            format!(
                "failed to save attachment to {}: {e}",
                self.local_path.display()
            )
        })?;
        let len = tokio::fs::metadata(&self.local_path)
            .await
            .map_err(|e| {
                format!(
                    "failed to read saved attachment {}: {e}",
                    self.local_path.display()
                )
            })?
            .len();
        Ok(SavedAttachment {
            local_path: self.local_path,
            file_name: self.file_name,
            size: u32::try_from(len).unwrap_or(u32::MAX),
        })
    }
}

impl AttachmentItem {
    /// Save `bytes` as the item's attachment file, named by the sanitized
    /// `info.filename` (with `_2`, `_3`, ... when the directory already holds that name).
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or the write fails.
    pub async fn save_bytes(
        &self,
        info: &AttachmentInfo,
        bytes: &[u8],
    ) -> Result<SavedAttachment, String> {
        let (file_name, local_path) =
            unique_attachment_path(self.claim.attachments_dir(), &info.filename).await?;

        tokio::fs::write(&local_path, bytes).await.map_err(|e| {
            format!(
                "failed to save attachment '{}' to {}: {e}",
                info.filename,
                local_path.display(),
            )
        })?;

        Ok(SavedAttachment {
            local_path,
            file_name,
            size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        })
    }

    /// Download `url` into the item's directory, named as [`Self::save_bytes`] names it.
    ///
    /// There is no size cap: the file already exists on the sending platform,
    /// which enforced its own upload limit, if any, when it was sent.
    ///
    /// # Errors
    /// Returns an error if the download or the file write fails.
    pub async fn download(
        &self,
        info: &AttachmentInfo,
        url: &str,
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

        self.save_bytes(info, &bytes).await
    }

    /// Create the item's attachment file for streaming, named as [`Self::save_bytes`] names it.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or the file cannot be created.
    pub async fn create_writer(&self, info: &AttachmentInfo) -> Result<AttachmentWriter, String> {
        let (file_name, local_path) =
            unique_attachment_path(self.claim.attachments_dir(), &info.filename).await?;
        let file = tokio::fs::File::create(&local_path).await.map_err(|e| {
            format!(
                "failed to create file '{}' at {}: {e}",
                info.filename,
                local_path.display(),
            )
        })?;
        Ok(AttachmentWriter {
            file,
            local_path,
            file_name,
        })
    }

    /// Release the item's claim: remove its attachment directory and anything
    /// partly written in it, after the attachment failed to save. Nothing is
    /// removed when the directory is already gone. A failed removal is logged,
    /// not returned: the failure that led here is already reported to the user.
    pub async fn discard(self) {
        self.claim.release().await;
    }

    /// Finalize a saved attachment: append its metadata line to `content`, encode
    /// it inline when it is a supported image within the size limit, and save the
    /// companion item that records it. Consumes the item.
    ///
    /// `author` is the sender named in the item body. Returns an `ImageData` if
    /// the attachment is a supported image within the inline size limit.
    pub async fn finalize(
        self,
        saved: &SavedAttachment,
        info: &AttachmentInfo,
        content: &mut String,
        author: &str,
    ) -> Option<ImageData> {
        content.push('\n');
        content.push_str(&format_attachment_line(saved, info));

        let image = match info.content_type.as_deref() {
            Some(ct) if is_supported_image(ct) => {
                if saved.size <= MAX_IMAGE_INLINE_SIZE {
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
                        size = saved.size,
                        max = MAX_IMAGE_INLINE_SIZE,
                        "attachment image exceeds inline size limit, saved but not sent to model"
                    );
                    None
                }
            }
            _ => None,
        };

        self.file_companion_item(saved, info, author).await;

        image
    }

    /// Save the companion inbox item that records the saved file at its path in
    /// this item's directory. Consumes the item. `author` is the sender named in
    /// the item body.
    ///
    /// A failed save is logged, not returned: the file is already on disk and
    /// named in the message, so the claimed directory and the file in it are
    /// kept for the agent.
    pub async fn file_companion_item(
        self,
        saved: &SavedAttachment,
        info: &AttachmentInfo,
        author: &str,
    ) {
        let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
        let companion = crate::inbox::InboxItem {
            title: self.title,
            body: format!(
                "From: {author}\nSize: {} bytes\nContent-Type: {content_type_str}",
                saved.size,
            ),
            source: self.source,
            timestamp: self.timestamp,
            read: false,
            attachments: vec![self.claim.recorded_attachment_path(&saved.file_name)],
            in_reply_to: None,
        };
        let mut claim = self.claim;
        if let Err(e) = claim.save(&companion).await {
            tracing::warn!(
                filename = %info.filename,
                error = %e,
                "failed to create companion inbox item for attachment; keeping the saved file"
            );
            claim.keep();
        }
    }
}

/// Reserve the companion inbox item for an attachment arriving on `platform`,
/// claiming its id and attachment directory on disk.
///
/// The claim is atomic, so reservations that overlap, even for the same title on
/// the same day, get distinct items. The caller must save the file through the
/// returned item, or [`AttachmentItem::discard`] it.
///
/// # Errors
/// Returns an error if the item's attachment directory cannot be claimed.
pub async fn reserve_attachment_item(
    inbox_dir: &Path,
    info: &AttachmentInfo,
    platform: &str,
    kind: AttachmentKind,
    tz: chrono_tz::Tz,
) -> Result<AttachmentItem, String> {
    let title = format!("{platform} {}: {}", kind.title_word(), info.filename);
    let timestamp = residuum_core::time::now_local(tz);
    let claim = ClaimedItem::claim(inbox_dir, &title, timestamp)
        .await
        .map_err(|e| format!("failed to reserve inbox item for '{}': {e}", info.filename))?;
    Ok(AttachmentItem {
        claim,
        title,
        source: platform.to_lowercase(),
        timestamp,
    })
}

/// Choose the name and path a file called `filename` is saved to in `dir`,
/// creating `dir` if needed.
///
/// The name is sanitized, then `_2`, `_3`, ... is appended before the extension
/// while `dir` already holds a file of that name.
///
/// # Errors
/// Returns an error if the directory cannot be created or read.
async fn unique_attachment_path(dir: &Path, filename: &str) -> Result<(String, PathBuf), String> {
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
    let path = dir.join(&name);
    Ok((name, path))
}

/// Format a metadata line for a successfully saved attachment.
#[must_use]
pub fn format_attachment_line(saved: &SavedAttachment, info: &AttachmentInfo) -> String {
    format!(
        "[Attachment: {} ({} bytes) \u{2192} {}]",
        info.filename,
        saved.size,
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
        let item = reserve_attachment_item(
            &inbox_dir,
            info,
            "Discord",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let saved = item.save_bytes(info, bytes).await.unwrap();
        (item, saved)
    }

    /// Reserve a companion item under `root`'s agent inbox without saving anything.
    async fn reserve_only(root: &Path, info: &AttachmentInfo) -> AttachmentItem {
        let inbox_dir = WorkspaceLayout::new(root).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        reserve_attachment_item(
            &inbox_dir,
            info,
            "Discord",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap()
    }

    fn pdf_info() -> AttachmentInfo {
        AttachmentInfo {
            filename: "doc.pdf".to_string(),
            size: 9,
            content_type: Some("application/pdf".to_string()),
        }
    }

    #[test]
    fn format_attachment_line_output() {
        let saved = SavedAttachment {
            local_path: PathBuf::from("/workspace/inbox/20260222_120000_photo.jpg"),
            file_name: "20260222_120000_photo.jpg".to_string(),
            size: 1024,
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
    async fn reserve_twice_with_same_info_claims_distinct_items() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let info = AttachmentInfo {
            filename: "image.png".to_string(),
            size: 4,
            content_type: Some("image/png".to_string()),
        };

        let first = reserve_attachment_item(
            &inbox_dir,
            &info,
            "Discord",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let second = reserve_attachment_item(
            &inbox_dir,
            &info,
            "Discord",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        assert_ne!(
            first.claim.attachments_dir(),
            second.claim.attachments_dir(),
            "two reservations must never share an attachment directory"
        );
        assert!(
            first.claim.attachments_dir().is_dir(),
            "first claim should be on disk"
        );
        assert!(
            second.claim.attachments_dir().is_dir(),
            "second claim should be on disk"
        );

        let first_saved = first.save_bytes(&info, b"one!").await.unwrap();
        let second_saved = second.save_bytes(&info, b"two!").await.unwrap();
        let mut first_content = String::new();
        let mut second_content = String::new();
        first
            .finalize(&first_saved, &info, &mut first_content, "author")
            .await;
        second
            .finalize(&second_saved, &info, &mut second_content, "author")
            .await;

        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 2, "both companion items should survive");
        assert_eq!(
            tokio::fs::read(&first_saved.local_path).await.unwrap(),
            b"one!",
            "the first file should keep its own bytes"
        );
        assert_eq!(
            tokio::fs::read(&second_saved.local_path).await.unwrap(),
            b"two!",
            "the second file should keep its own bytes"
        );
    }

    #[tokio::test]
    async fn discard_removes_claimed_dir_and_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let info = pdf_info();
        let item = reserve_attachment_item(
            &inbox_dir,
            &info,
            "Telegram",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let mut writer = item.create_writer(&info).await.unwrap();
        writer.file_mut().write_all(b"partial").await.unwrap();
        let attachments_dir = item.claim.attachments_dir().to_path_buf();
        assert!(
            attachments_dir.is_dir(),
            "claimed dir should exist before discard"
        );

        item.discard().await;

        assert!(
            !attachments_dir.exists(),
            "discard should remove the claimed dir"
        );
        assert!(
            crate::inbox::list_items(&inbox_dir)
                .await
                .unwrap()
                .is_empty(),
            "no item should be saved for a discarded attachment"
        );
    }

    #[tokio::test]
    async fn discard_is_a_no_op_when_dir_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let item = reserve_attachment_item(
            &inbox_dir,
            &pdf_info(),
            "Discord",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let attachments_dir = item.claim.attachments_dir().to_path_buf();
        tokio::fs::remove_dir_all(&attachments_dir).await.unwrap();

        item.discard().await;

        assert!(!attachments_dir.exists());
    }

    #[tokio::test]
    async fn dropping_an_unfinalized_item_releases_its_claim() {
        let dir = tempfile::tempdir().unwrap();
        let info = pdf_info();
        let item = reserve_only(dir.path(), &info).await;
        let attachments_dir = item.claim.attachments_dir().to_path_buf();
        item.save_bytes(&info, b"half done").await.unwrap();

        drop(item);

        assert!(
            !attachments_dir.exists(),
            "an item that is neither finalized nor discarded must not leave its directory"
        );
    }

    #[tokio::test]
    async fn failed_companion_save_keeps_the_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let info = pdf_info();
        let (item, saved) = reserve_and_save(dir.path(), &info, b"pdf bytes").await;
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        // A directory where the item's JSON belongs makes the companion save fail.
        tokio::fs::create_dir(inbox_dir.join(item.claim.filename()))
            .await
            .unwrap();
        let mut content = String::new();

        item.finalize(&saved, &info, &mut content, "author").await;

        assert!(
            saved.local_path.is_file(),
            "the file named in the message stays on disk when its item cannot be saved"
        );
        assert!(
            content.contains(&saved.local_path.display().to_string()),
            "the message still names the file: {content}"
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
            size: 0,
            content_type: Some("text/plain".to_string()),
        };
        let item = reserve_only(dir.path(), &info).await;

        let saved = item.download(&info, &url).await.unwrap();
        assert!(saved.local_path().exists(), "file should exist on disk");
        assert_eq!(saved.size(), 11, "size should be the downloaded byte count");

        let content = tokio::fs::read_to_string(saved.local_path()).await.unwrap();
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
            size: 0,
            content_type: None,
        };
        let item = reserve_only(dir.path(), &info).await;
        let saved = item
            .download(&info, &url)
            .await
            .expect("a large attachment should be accepted");
        let saved_len = tokio::fs::metadata(saved.local_path()).await.unwrap().len();
        assert_eq!(saved_len, body.len() as u64);
        assert_eq!(
            saved.size(),
            u32::try_from(body.len()).expect("test body fits in u32")
        );
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
        let item = reserve_only(dir.path(), &info).await;

        let result = item.download(&info, &url).await;
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
        let info = pdf_info();
        let (item, saved) = reserve_and_save(dir.path(), &info, b"pdf bytes").await;
        let mut content = String::new();
        let image = item.finalize(&saved, &info, &mut content, "author").await;
        assert!(image.is_none(), "pdf should not be encoded inline");

        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        assert_eq!(items.len(), 1, "one companion item should be saved");
        let (_, companion) = items.first().expect("one companion item should be saved");
        assert_eq!(
            companion.attachments.len(),
            1,
            "item should record its file"
        );

        let recorded = companion
            .attachments
            .first()
            .expect("item should record its file");
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
    async fn companion_item_body_records_the_saved_size() {
        let dir = tempfile::tempdir().unwrap();
        let info = AttachmentInfo {
            filename: "report.bin".to_string(),
            size: 0,
            content_type: None,
        };
        let (item, saved) = reserve_and_save(dir.path(), &info, b"12345").await;
        let mut content = String::new();
        item.finalize(&saved, &info, &mut content, "author").await;

        assert!(
            content.contains("(5 bytes)"),
            "the message line should name the real size: {content}"
        );
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        let items = crate::inbox::list_items(&inbox_dir).await.unwrap();
        let (_, companion) = items.first().expect("one companion item should be saved");
        assert!(
            companion.body.contains("Size: 5 bytes"),
            "the item body should name the real size: {}",
            companion.body
        );
    }

    #[tokio::test]
    async fn attachments_with_same_filename_on_one_item_are_deduped() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = WorkspaceLayout::new(dir.path()).agent_inbox_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let info = AttachmentInfo {
            filename: "notes.txt".to_string(),
            size: 3,
            content_type: None,
        };
        let item = reserve_attachment_item(
            &inbox_dir,
            &info,
            "A2A",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();

        let first = item.save_bytes(&info, b"one").await.unwrap();
        let second = item.save_bytes(&info, b"two").await.unwrap();

        assert_eq!(
            first.local_path,
            item.claim.attachments_dir().join("notes.txt")
        );
        assert_eq!(
            second.local_path,
            item.claim.attachments_dir().join("notes_2.txt")
        );
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
        let item = reserve_attachment_item(
            &inbox_dir,
            &info,
            "Telegram",
            AttachmentKind::Attachment,
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let saved = item.save_bytes(&info, b"png!").await.unwrap();
        let mut content = String::new();
        item.finalize(&saved, &info, &mut content, "author").await;

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

        let result = item.finalize(&saved, &info, &mut content, "author").await;

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
            size: 0,
            content_type: Some("image/jpeg".to_string()),
        };
        // The inline limit applies to the saved file's real size, not the
        // platform-reported one, so the oversized bytes must actually be written.
        let oversized = vec![0_u8; MAX_IMAGE_INLINE_SIZE as usize + 1];
        let (item, saved) = reserve_and_save(dir.path(), &info, &oversized).await;
        let mut content = String::new();

        let result = item.finalize(&saved, &info, &mut content, "author").await;

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

        let result = item.finalize(&saved, &info, &mut content, "author").await;

        assert!(
            result.is_none(),
            "non-image content type should not be encoded"
        );
    }
}

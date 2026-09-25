//! File reading tool for the agent.

use std::path::Path;

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::file_tracker::SharedFileTracker;
use super::{Tool, ToolError, ToolResult};
use crate::inference::{ImageData, ToolDefinition};
use crate::interfaces::attachment::MAX_IMAGE_INLINE_SIZE;

/// Default maximum lines returned when no explicit offset/limit is given.
const DEFAULT_MAX_LINES: usize = 2000;

/// Maximum characters per output line before truncation.
const MAX_CHARS_PER_LINE: usize = 2000;

/// Tool that reads file contents with numbered lines.
pub struct ReadTool {
    tracker: SharedFileTracker,
}

impl ReadTool {
    /// Create a new `ReadTool` with shared file tracker.
    #[must_use]
    pub fn new(tracker: SharedFileTracker) -> Self {
        Self { tracker }
    }

    /// Read an image file, base64-encode it, and return as a tool result with inline image data.
    #[expect(clippy::cast_precision_loss, reason = "file size in KB display only")]
    async fn read_image(&self, path: &str, size: u64, mime: &str) -> Result<ToolResult, ToolError> {
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };

        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let filename = Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        let size_kb = size as f64 / 1024.0;
        let summary = format!("[Image: {filename}, {size_kb:.1} KB]");

        self.tracker.lock().await.record_read(path);

        Ok(ToolResult::success_with_images(
            summary,
            vec![ImageData {
                media_type: mime.to_string(),
                data: encoded,
            }],
        ))
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Read the contents of a file. Each output line is prefixed with its \
                          line number and a tab (e.g. `   1\\thello`); the prefix is not part of \
                          the file, so leave it out of edit_file's old_string. \
                          By default returns the first 2000 lines; use offset/limit to page through \
                          the rest — there is no file size limit, the output header reports the \
                          file's total size and line count either way. \
                          Lines longer than 2000 characters are truncated. \
                          Image files (JPEG, PNG, GIF, WebP) are returned as inline images \
                          for visual inspection instead of raw bytes, capped by the model API's \
                          inline image size limit."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-based, default: 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (default: 2000)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'path' argument".to_string())
            })?;

        let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let explicit_limit = arguments.get("limit").and_then(Value::as_u64);

        let metadata = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };
        let total_size = metadata.len();

        // Check if this is a supported image file — return inline image data.
        // The size cap here is a model API fact (the maximum image size the
        // provider accepts inline), not a residuum-imposed limit.
        if let Some(mime) = image_mime_type(Path::new(path)) {
            if total_size > u64::from(MAX_IMAGE_INLINE_SIZE) {
                return Ok(ToolResult::error(format!(
                    "image {path} is too large to send inline ({total_size} bytes, max \
                     {MAX_IMAGE_INLINE_SIZE} bytes — this is a model API limit)"
                )));
            }
            return self.read_image(path, total_size, mime).await;
        }

        // Text files have no size limit: page through by offset/limit rather
        // than loading the whole file into memory. `start` and `effective_limit`
        // are resolved against line numbers as we stream, not against a
        // pre-counted total, so a file of any size can be paged with bounded
        // memory use.
        // `try_from` rather than `as`: a fallible conversion clamped to
        // `usize::MAX` on overflow, so no truncating cast is needed here.
        let start = usize::try_from(offset).unwrap_or(usize::MAX);

        // Apply the default limit only when no explicit limit/offset given.
        let effective_limit: Option<usize> = explicit_limit.map_or_else(
            || (offset == 0).then_some(DEFAULT_MAX_LINES),
            |l| Some(usize::try_from(l).unwrap_or(usize::MAX)),
        );

        let file = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };
        let mut lines_stream = BufReader::new(file).lines();

        let mut selected: Vec<String> = Vec::new();
        let mut truncated_count: usize = 0;
        let mut total_lines: usize = 0;
        let mut first_shown_line: Option<usize> = None;
        let mut last_shown_line: usize = 0;

        loop {
            let line = match lines_stream.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
            };
            total_lines += 1;
            let line_no = total_lines;

            if line_no <= start {
                continue;
            }
            // Keep streaming (without collecting) past the limit so total_lines stays accurate.
            if effective_limit.is_some_and(|limit| selected.len() >= limit) {
                continue;
            }

            let (formatted, was_truncated) = format_numbered_line(line_no, &line);
            if was_truncated {
                truncated_count += 1;
            }
            selected.push(formatted);
            first_shown_line.get_or_insert(line_no);
            last_shown_line = line_no;
        }

        // Build the info/warnings header. Total size and line count are
        // always reported so a paged or truncated view never hides how much
        // more there is to see.
        let mut warnings: Vec<String> = vec![format!(
            "file: {total_size} bytes, {total_lines} line(s) total"
        )];
        match first_shown_line {
            Some(first) if first > 1 || last_shown_line < total_lines => {
                warnings.push(format!(
                    "showing lines {first}-{last_shown_line} of {total_lines}; \
                     use offset/limit to see more, or exec with grep to find specific content"
                ));
            }
            None if total_lines > 0 => {
                warnings.push(format!(
                    "offset {offset} is at or beyond the file's {total_lines} line(s); nothing to show"
                ));
            }
            Some(_) | None => {}
        }
        if truncated_count > 0 {
            warnings.push(format!(
                "warning: {truncated_count} line(s) exceeded {MAX_CHARS_PER_LINE} characters and were truncated"
            ));
        }

        // Record read in tracker
        self.tracker.lock().await.record_read(path);

        let header = warnings.join("\n");
        let body = selected.join("\n");
        if body.is_empty() {
            Ok(ToolResult::success(header))
        } else {
            Ok(ToolResult::success(format!("{header}\n\n{body}")))
        }
    }
}

/// Format one line the way `read_file` shows it: right-aligned line number, a tab, the text.
///
/// Lines longer than `MAX_CHARS_PER_LINE` are cut at a UTF-8 boundary; the returned flag
/// reports whether that happened.
pub(super) fn format_numbered_line(line_num: usize, line: &str) -> (String, bool) {
    if line.len() > MAX_CHARS_PER_LINE {
        let boundary = line.floor_char_boundary(MAX_CHARS_PER_LINE);
        let truncated = line.get(..boundary).unwrap_or_default();
        (format!("{line_num:>4}\t{truncated} ... (truncated)"), true)
    } else {
        (format!("{line_num:>4}\t{line}"), false)
    }
}

/// Return the MIME type for a supported image extension, or `None`.
fn image_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::file_tracker::FileTracker;

    fn make_tool() -> ReadTool {
        ReadTool::new(FileTracker::new_shared())
    }

    #[tokio::test]
    async fn read_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("line 1"),
            "output should contain file content"
        );
        assert!(
            result.output.contains("line 3"),
            "output should contain all lines"
        );
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "offset": 1,
                "limit": 2
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(result.output.contains("line 2"), "should start from offset");
        assert!(
            result.output.contains("line 3"),
            "should include limited lines"
        );
        assert!(
            !result.output.contains("line 1"),
            "should not include lines before offset"
        );
        assert!(
            !result.output.contains("line 4"),
            "should not include lines beyond limit"
        );
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": "/nonexistent/file.txt" }))
            .await
            .unwrap();

        assert!(result.is_error, "missing file should be an error result");
    }

    #[tokio::test]
    async fn read_file_missing_path() {
        let tool = make_tool();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing path should return ToolError");
    }

    #[test]
    fn read_tool_definition() {
        let tool = make_tool();
        assert_eq!(tool.name(), "read_file", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "read_file", "definition name should match");
    }

    #[tokio::test]
    async fn output_prefixes_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("numbered.txt");
        tokio::fs::write(&file_path, "hello\nworld\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            result.output.ends_with("\n\n   1\thello\n   2\tworld"),
            "each line should be its number, a tab, then the text: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn default_line_limit_caps_at_2000() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big.txt");
        let mut big_content = String::new();
        for i in 1..=3000 {
            _ = writeln!(big_content, "line {i}");
        }
        tokio::fs::write(&file_path, &big_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("3000 line(s) total"),
            "should report total line count"
        );
        assert!(
            result.output.contains("showing lines 1-2000 of 3000"),
            "should show the default-limited range"
        );
        // Count actual content lines (skip warning lines)
        let content_lines: Vec<&str> = result.output.lines().filter(|l| l.contains('\t')).collect();
        assert_eq!(
            content_lines.len(),
            2000,
            "should return exactly 2000 content lines"
        );
    }

    #[tokio::test]
    async fn explicit_limit_can_exceed_default() {
        use std::fmt::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("big2.txt");
        let mut big_content = String::new();
        for i in 1..=2500 {
            _ = writeln!(big_content, "line {i}");
        }
        tokio::fs::write(&file_path, &big_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "limit": 2500
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            !result.output.contains("warning:"),
            "no warning when explicit limit is used"
        );
        assert!(
            !result.output.contains("showing lines"),
            "no partial-range notice when the explicit limit covers the whole file"
        );
        let content_lines: Vec<&str> = result.output.lines().filter(|l| l.contains('\t')).collect();
        assert_eq!(
            content_lines.len(),
            2500,
            "should return all 2500 lines with explicit limit"
        );
    }

    #[tokio::test]
    async fn line_char_truncation_and_warning() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("wide.txt");
        let long_line = "x".repeat(3000);
        let short_line = "short";
        let file_content = format!("{long_line}\n{short_line}\n");
        tokio::fs::write(&file_path, &file_content).await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "read should succeed");
        assert!(
            result.output.contains("1 line(s) exceeded 2000 characters"),
            "should warn about truncated lines"
        );
        assert!(
            result.output.contains("(truncated)"),
            "truncated lines should be marked"
        );
    }

    #[tokio::test]
    async fn tracker_records_path_after_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("track.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let tracker = FileTracker::new_shared();
        let tool = ReadTool::new(std::sync::Arc::clone(&tracker));

        tool.execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            tracker
                .lock()
                .await
                .has_been_read(file_path.to_str().unwrap()),
            "tracker should record the read path"
        );
    }

    #[tokio::test]
    async fn read_image_file_returns_inline_image() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        // Write fake JPEG bytes (real image not needed — just testing encoding path)
        tokio::fs::write(&file_path, b"\xFF\xD8\xFF\xE0fake jpeg data")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "image read should succeed");
        assert!(
            result.output.contains("[Image:"),
            "output should contain image summary: {}",
            result.output,
        );
        assert_eq!(result.images.len(), 1, "should have one inline image");
        assert_eq!(
            result.images.first().unwrap().media_type,
            "image/jpeg",
            "media type should be image/jpeg"
        );
        assert!(
            !result.images.first().unwrap().data.is_empty(),
            "base64 data should be non-empty"
        );
    }

    #[tokio::test]
    async fn read_text_file_has_no_images() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.md");
        tokio::fs::write(&file_path, "# Hello\nworld\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "text read should succeed");
        assert!(
            result.images.is_empty(),
            "text files should not return images"
        );
    }

    #[tokio::test]
    async fn read_image_records_in_tracker() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.png");
        tokio::fs::write(&file_path, b"\x89PNG\r\n\x1A\nfake png")
            .await
            .unwrap();

        let tracker = FileTracker::new_shared();
        let tool = ReadTool::new(std::sync::Arc::clone(&tracker));

        tool.execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            tracker
                .lock()
                .await
                .has_been_read(file_path.to_str().unwrap()),
            "tracker should record image file read"
        );
    }

    #[tokio::test]
    async fn read_file_over_old_ten_mb_cap_is_not_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("huge.bin");
        let file = tokio::fs::File::create(&file_path).await.unwrap();
        // Sparse file past the old 10 MB hard cap; content is all NUL bytes,
        // which are valid UTF-8, so this exercises size handling without
        // writing real data to disk.
        file.set_len(11 * 1024 * 1024).await.unwrap();
        drop(file);

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "a file over the old 10 MB cap should no longer be refused: {}",
            result.output
        );
        assert!(
            result.output.contains("11534336 bytes"),
            "should report the true total size: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn read_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        tokio::fs::write(&file_path, "").await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        assert!(!result.is_error, "reading empty file should succeed");
        assert!(result.images.is_empty(), "empty file should have no images");
    }

    #[test]
    fn image_mime_type_detection() {
        assert_eq!(image_mime_type(Path::new("photo.jpg")), Some("image/jpeg"),);
        assert_eq!(image_mime_type(Path::new("photo.JPEG")), Some("image/jpeg"),);
        assert_eq!(image_mime_type(Path::new("icon.png")), Some("image/png"),);
        assert_eq!(image_mime_type(Path::new("anim.gif")), Some("image/gif"),);
        assert_eq!(
            image_mime_type(Path::new("modern.webp")),
            Some("image/webp"),
        );
        assert_eq!(image_mime_type(Path::new("document.txt")), None,);
        assert_eq!(image_mime_type(Path::new("noext")), None,);
    }
}

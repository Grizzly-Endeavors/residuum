//! File reading tool for the agent.

use std::path::Path;

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use super::file_tracker::SharedFileTracker;
use super::line_hash::line_hash;
use super::{Tool, ToolError, ToolResult};
use crate::models::{ImageData, ToolDefinition};

/// Hard cap on file size (10 MB safety net).
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

/// Default maximum lines returned when no explicit offset/limit is given.
const DEFAULT_MAX_LINES: usize = 2000;

/// Maximum characters per output line before truncation.
const MAX_CHARS_PER_LINE: usize = 2000;

/// Tool that reads file contents with hash-tagged line numbers.
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
            name: "read_file".to_string(),
            description: "Read the contents of a file. Each output line is tagged with a \
                          content hash (e.g. `1:f1\\thello`) for use with edit_file. \
                          By default returns the first 2000 lines; use offset/limit for larger files. \
                          Lines longer than 2000 characters are truncated. \
                          Image files (JPEG, PNG, GIF, WebP) are returned as inline images \
                          for visual inspection instead of raw bytes."
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

        if metadata.len() > MAX_READ_BYTES {
            return Ok(ToolResult::error(format!(
                "file {path} is too large ({} bytes, max {MAX_READ_BYTES})",
                metadata.len()
            )));
        }

        // Check if this is a supported image file — return inline image data
        if let Some(mime) = image_mime_type(Path::new(path)) {
            return self.read_image(path, metadata.len(), mime).await;
        }

        let file_contents = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };

        let lines: Vec<&str> = file_contents.lines().collect();
        let total_lines = lines.len();

        #[expect(
            clippy::cast_possible_truncation,
            reason = "offset from JSON u64 capped by line count"
        )]
        let start = (offset as usize).min(total_lines);

        // Apply default limit only when no explicit limit/offset given
        #[expect(
            clippy::cast_possible_truncation,
            reason = "limit from JSON u64 capped by line count"
        )]
        let effective_limit = explicit_limit.map_or_else(
            || {
                if offset == 0 {
                    DEFAULT_MAX_LINES
                } else {
                    total_lines
                }
            },
            |l| l as usize,
        );
        let end = (start + effective_limit).min(total_lines);
        let line_limit_applied = end < total_lines && explicit_limit.is_none() && offset == 0;

        let mut truncated_count: usize = 0;

        #[expect(
            clippy::indexing_slicing,
            reason = "start and end are clamped to total_lines; the slice is always in-bounds"
        )]
        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let hash = line_hash(line);
                let line_num = start + i + 1;

                // Truncate long lines (UTF-8 safe)
                if line.len() > MAX_CHARS_PER_LINE {
                    truncated_count += 1;
                    let boundary = line.floor_char_boundary(MAX_CHARS_PER_LINE);
                    let truncated = line.get(..boundary).unwrap_or_default();
                    format!("{line_num:>4}:{hash}\t{truncated} ... (truncated)")
                } else {
                    format!("{line_num:>4}:{hash}\t{line}")
                }
            })
            .collect();

        // Build warnings header
        let mut warnings: Vec<String> = Vec::new();
        if line_limit_applied {
            warnings.push(format!(
                "warning: file has {total_lines} lines, showing first {DEFAULT_MAX_LINES}; \
                 use offset/limit or exec with grep to find specific content"
            ));
        }
        if truncated_count > 0 {
            warnings.push(format!(
                "warning: {truncated_count} line(s) exceeded {MAX_CHARS_PER_LINE} characters and were truncated"
            ));
        }

        // Record read in tracker
        self.tracker.lock().await.record_read(path);

        let body = selected.join("\n");
        if warnings.is_empty() {
            Ok(ToolResult::success(body))
        } else {
            let header = warnings.join("\n");
            Ok(ToolResult::success(format!("{header}\n\n{body}")))
        }
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
    async fn output_includes_hash_tags() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hash_test.txt");
        tokio::fs::write(&file_path, "hello\nworld\n")
            .await
            .unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": file_path.to_str().unwrap() }))
            .await
            .unwrap();

        // Each line should have format "   N:xx\tcontent"
        for output_line in result.output.lines() {
            assert!(
                output_line.contains(':'),
                "output line should contain hash separator: {output_line}"
            );
            assert!(
                output_line.contains('\t'),
                "output line should contain tab: {output_line}"
            );
        }
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
            result.output.contains("warning: file has 3000 lines"),
            "should warn about line limit"
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

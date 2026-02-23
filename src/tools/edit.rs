//! Hash-line file editing tool for the agent.
//!
//! Provides surgical edits anchored by line number + content hash pairs
//! from `read_file` output. Re-validates hashes before applying changes
//! to detect stale edits.

use async_trait::async_trait;
use serde_json::Value;

use super::file_tracker::SharedFileTracker;
use super::line_hash::line_hash;
use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Tool that performs hash-validated line edits on files.
pub struct EditTool {
    tracker: SharedFileTracker,
}

impl EditTool {
    /// Create a new `EditTool` with shared file tracker.
    #[must_use]
    pub fn new(tracker: SharedFileTracker) -> Self {
        Self { tracker }
    }
}

/// Parsed line anchor: (1-indexed line number, 2-char hex hash).
struct LineAnchor {
    line_num: usize,
    hash: String,
}

/// Parse a `"line:hash"` string (e.g. `"5:a3"`) into its components.
fn parse_anchor(value: &str) -> Result<LineAnchor, ToolError> {
    let Some((num_str, hash_str)) = value.split_once(':') else {
        return Err(ToolError::InvalidArguments(format!(
            "invalid line:hash format '{value}', expected 'N:xx' (e.g. '5:a3')"
        )));
    };

    let line_num: usize = num_str.parse().map_err(|_not_int| {
        ToolError::InvalidArguments(format!("invalid line number '{num_str}' in '{value}'"))
    })?;

    if hash_str.len() != 2 || !hash_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ToolError::InvalidArguments(format!(
            "invalid hash '{hash_str}' in '{value}', expected 2 hex characters"
        )));
    }

    Ok(LineAnchor {
        line_num,
        hash: hash_str.to_string(),
    })
}

/// Validate that a line anchor is in bounds and its hash matches the current file content.
///
/// Returns `Ok(None)` if valid, `Ok(Some(error_message))` for hash/bounds mismatches
/// (soft errors the model can act on).
fn validate_anchor(anchor: &LineAnchor, lines: &[String]) -> Option<String> {
    if anchor.line_num == 0 || anchor.line_num > lines.len() {
        return Some(format!(
            "line {} is out of bounds (file has {} lines)",
            anchor.line_num,
            lines.len()
        ));
    }

    let idx = anchor.line_num - 1;
    let actual_line = lines.get(idx).map_or("", String::as_str);
    let actual_hash = line_hash(actual_line);

    if actual_hash != anchor.hash {
        return Some(format!(
            "hash mismatch at line {}: expected {}, got {actual_hash} \
             (file may have changed since last read; re-read the file)",
            anchor.line_num, anchor.hash
        ));
    }

    None
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".to_string(),
            description: "Edit a file using line:hash anchors from read_file output. \
                          Validates content hashes before applying changes to detect stale edits. \
                          Operations: 'replace' (replace line or range), 'insert_after' (insert \
                          after a line; use start_line '0' to insert at file start), 'delete' \
                          (remove line or range). Use this over write_file when updating existing content."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit"
                    },
                    "operation": {
                        "type": "string",
                        "enum": ["replace", "insert_after", "delete"],
                        "description": "The edit operation to perform"
                    },
                    "start_line": {
                        "type": "string",
                        "description": "Line anchor as 'N:hash' (e.g. '5:a3'). Use '0' for insert_after at file start."
                    },
                    "end_line": {
                        "type": "string",
                        "description": "Optional end line anchor as 'N:hash' for range operations"
                    },
                    "content": {
                        "type": "string",
                        "description": "New content (required for replace and insert_after, omitted for delete)"
                    }
                },
                "required": ["path", "operation", "start_line"]
            }),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "edit logic with validation is inherently sequential; splitting would scatter related checks"
    )]
    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'path' argument".to_string())
            })?;

        let operation = arguments
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'operation' argument".to_string())
            })?;

        let start_line_str = arguments
            .get("start_line")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'start_line' argument".to_string())
            })?;

        let end_line_str = arguments.get("end_line").and_then(Value::as_str);
        let new_content = arguments.get("content").and_then(Value::as_str);

        // Validate operation
        if !matches!(operation, "replace" | "insert_after" | "delete") {
            return Err(ToolError::InvalidArguments(format!(
                "invalid operation '{operation}', must be 'replace', 'insert_after', or 'delete'"
            )));
        }

        // Content required for replace and insert_after
        if matches!(operation, "replace" | "insert_after") && new_content.is_none() {
            return Err(ToolError::InvalidArguments(format!(
                "'{operation}' requires 'content' argument"
            )));
        }

        // Parse start_line — special case: "0" for insert_after at file start
        let insert_at_start = operation == "insert_after" && start_line_str == "0";
        let start_anchor = if insert_at_start {
            None
        } else {
            Some(parse_anchor(start_line_str)?)
        };

        // Parse end_line if provided
        let end_anchor = end_line_str.map(parse_anchor).transpose()?;

        // Validate range ordering
        if let (Some(start), Some(end)) = (&start_anchor, &end_anchor)
            && end.line_num < start.line_num
        {
            return Err(ToolError::InvalidArguments(format!(
                "end_line {} is before start_line {}",
                end.line_num, start.line_num
            )));
        }

        // Check file exists
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Ok(ToolResult::error(format!("file {path} does not exist")));
        }

        // Check tracker — must have been read
        if !self.tracker.lock().await.has_been_read(path) {
            return Ok(ToolResult::error(format!(
                "file {path} has not been read; use read_file before editing"
            )));
        }

        // Read current file
        let file_text = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!("failed to read {path}: {e}")));
            }
        };

        let mut lines: Vec<String> = file_text.lines().map(String::from).collect();
        let ends_with_newline = file_text.ends_with('\n');

        // Validate anchors against current file content
        if let Some(anchor) = &start_anchor
            && let Some(err_msg) = validate_anchor(anchor, &lines)
        {
            return Ok(ToolResult::error(err_msg));
        }
        if let Some(anchor) = &end_anchor
            && let Some(err_msg) = validate_anchor(anchor, &lines)
        {
            return Ok(ToolResult::error(err_msg));
        }

        // Compute effective range (0-indexed)
        let (range_start, range_end) = if let Some(start) = &start_anchor {
            let s = start.line_num - 1;
            let e = end_anchor.as_ref().map_or(s, |end| end.line_num - 1);
            (s, e)
        } else {
            (0, 0)
        };

        // Apply operation
        let description = match operation {
            "replace" => {
                let content_lines: Vec<&str> = new_content.unwrap_or_default().lines().collect();
                let range_desc = if range_start == range_end {
                    format!("{}", range_start + 1)
                } else {
                    format!("{}-{}", range_start + 1, range_end + 1)
                };

                let mut new_lines = Vec::with_capacity(
                    lines.len() - (range_end - range_start + 1) + content_lines.len(),
                );
                new_lines.extend(lines.drain(..range_start));
                new_lines.extend(content_lines.iter().map(|s| (*s).to_string()));
                let skip_count = range_end - range_start + 1;
                let remaining: Vec<String> = lines.into_iter().skip(skip_count).collect();
                new_lines.extend(remaining);
                lines = new_lines;

                format!("replaced line(s) {range_desc}")
            }
            "insert_after" => {
                let content_lines: Vec<&str> = new_content.unwrap_or_default().lines().collect();

                if insert_at_start {
                    let mut new_lines = Vec::with_capacity(lines.len() + content_lines.len());
                    new_lines.extend(content_lines.iter().map(|s| (*s).to_string()));
                    new_lines.extend(lines);
                    lines = new_lines;
                    "inserted at file start".to_string()
                } else {
                    let insert_idx = range_start + 1;
                    let mut new_lines = Vec::with_capacity(lines.len() + content_lines.len());
                    new_lines.extend(lines.drain(..insert_idx));
                    new_lines.extend(content_lines.iter().map(|s| (*s).to_string()));
                    new_lines.extend(lines);
                    lines = new_lines;
                    format!("inserted after line {}", range_start + 1)
                }
            }
            "delete" => {
                let delete_count = range_end - range_start + 1;
                if delete_count >= lines.len() {
                    return Ok(ToolResult::error(format!(
                        "cannot delete all lines from {path}"
                    )));
                }

                let range_desc = if range_start == range_end {
                    format!("{}", range_start + 1)
                } else {
                    format!("{}-{}", range_start + 1, range_end + 1)
                };

                let mut new_lines = Vec::with_capacity(lines.len() - delete_count);
                new_lines.extend(lines.drain(..range_start));
                let remaining: Vec<String> = lines.into_iter().skip(delete_count).collect();
                new_lines.extend(remaining);
                lines = new_lines;

                format!("deleted line(s) {range_desc}")
            }
            _ => {
                return Err(ToolError::InvalidArguments(format!(
                    "unknown operation '{operation}'"
                )));
            }
        };

        // Reconstruct file content
        let mut output = lines.join("\n");
        if ends_with_newline {
            output.push('\n');
        }

        if let Err(e) = tokio::fs::write(path, &output).await {
            return Ok(ToolResult::error(format!("failed to write {path}: {e}")));
        }

        Ok(ToolResult::success(format!("edited {path}: {description}")))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::tools::file_tracker::FileTracker;
    use crate::tools::line_hash::line_hash as compute_hash;

    /// Create an `EditTool` with a pre-registered path in the tracker.
    async fn make_tool_with_file(path: &str) -> EditTool {
        let tracker = FileTracker::new_shared();
        tracker.lock().await.record_read(path);
        EditTool::new(tracker)
    }

    /// Create an `EditTool` with an empty tracker (nothing read).
    fn make_tool_no_reads() -> EditTool {
        EditTool::new(FileTracker::new_shared())
    }

    /// Helper: write a test file and return the tool with path registered.
    async fn setup_file(dir: &tempfile::TempDir, name: &str, content: &str) -> (EditTool, String) {
        let file_path = dir.path().join(name);
        tokio::fs::write(&file_path, content).await.unwrap();
        let path_str = file_path.to_str().unwrap().to_string();
        let tool = make_tool_with_file(&path_str).await;
        (tool, path_str)
    }

    fn anchor(line: usize, content: &str) -> String {
        format!("{line}:{}", compute_hash(content))
    }

    #[tokio::test]
    async fn single_line_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "replace.txt", "aaa\nbbb\nccc\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "replace",
                "start_line": anchor(2, "bbb"),
                "content": "BBB"
            }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "replace should succeed: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "aaa\nBBB\nccc\n", "line 2 should be replaced");
    }

    #[tokio::test]
    async fn range_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "range.txt", "a\nb\nc\nd\ne\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "replace",
                "start_line": anchor(2, "b"),
                "end_line": anchor(4, "d"),
                "content": "X\nY"
            }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "range replace should succeed: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "a\nX\nY\ne\n",
            "lines 2-4 should be replaced with X, Y"
        );
    }

    #[tokio::test]
    async fn insert_after_line() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "insert.txt", "first\nsecond\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "insert_after",
                "start_line": anchor(1, "first"),
                "content": "inserted"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "insert should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "first\ninserted\nsecond\n",
            "line should be inserted after line 1"
        );
    }

    #[tokio::test]
    async fn insert_at_file_start() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "start.txt", "existing\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "insert_after",
                "start_line": "0",
                "content": "header"
            }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "insert at start should succeed: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            updated, "header\nexisting\n",
            "content should be inserted at file start"
        );
    }

    #[tokio::test]
    async fn delete_single_line() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "del.txt", "keep\nremove\nkeep2\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "delete",
                "start_line": anchor(2, "remove"),
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "delete should succeed: {}", result.output);
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "keep\nkeep2\n", "line 2 should be deleted");
    }

    #[tokio::test]
    async fn delete_range() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "delrange.txt", "a\nb\nc\nd\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "delete",
                "start_line": anchor(2, "b"),
                "end_line": anchor(3, "c"),
            }))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "range delete should succeed: {}",
            result.output
        );
        let updated = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(updated, "a\nd\n", "lines 2-3 should be deleted");
    }

    #[tokio::test]
    async fn hash_mismatch_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "mismatch.txt", "hello\nworld\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "replace",
                "start_line": "1:ff",
                "content": "replaced"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "hash mismatch should fail");
        assert!(
            result.output.contains("hash mismatch"),
            "error should mention hash mismatch: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn file_not_found() {
        let tool = make_tool_with_file("/nonexistent/edit_target.txt").await;

        let result = tool
            .execute(serde_json::json!({
                "path": "/nonexistent/edit_target.txt",
                "operation": "replace",
                "start_line": "1:aa",
                "content": "x"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "nonexistent file should fail");
        assert!(
            result.output.contains("does not exist"),
            "error should mention file not found"
        );
    }

    #[tokio::test]
    async fn not_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("unread.txt");
        tokio::fs::write(&file_path, "content\n").await.unwrap();

        let tool = make_tool_no_reads();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "operation": "replace",
                "start_line": "1:aa",
                "content": "x"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "unread file should fail");
        assert!(
            result.output.contains("has not been read"),
            "error should mention read requirement"
        );
    }

    #[tokio::test]
    async fn invalid_operation() {
        let result = make_tool_no_reads()
            .execute(serde_json::json!({
                "path": "/tmp/x.txt",
                "operation": "bogus",
                "start_line": "1:aa"
            }))
            .await;

        assert!(result.is_err(), "invalid operation should return ToolError");
    }

    #[tokio::test]
    async fn missing_required_params() {
        let tool = make_tool_no_reads();

        // Missing path
        let no_path = tool
            .execute(serde_json::json!({"operation": "replace", "start_line": "1:aa"}))
            .await;
        assert!(no_path.is_err(), "missing path should error");

        // Missing operation
        let no_op = tool
            .execute(serde_json::json!({"path": "/tmp/x", "start_line": "1:aa"}))
            .await;
        assert!(no_op.is_err(), "missing operation should error");

        // Missing start_line
        let no_start = tool
            .execute(serde_json::json!({"path": "/tmp/x", "operation": "replace"}))
            .await;
        assert!(no_start.is_err(), "missing start_line should error");
    }

    #[tokio::test]
    async fn malformed_line_hash_string() {
        let tool = make_tool_no_reads();

        // No colon
        let no_colon = tool
            .execute(serde_json::json!({
                "path": "/tmp/x", "operation": "replace",
                "start_line": "5aa", "content": "x"
            }))
            .await;
        assert!(no_colon.is_err(), "missing colon should error");

        // Non-numeric line
        let bad_num = tool
            .execute(serde_json::json!({
                "path": "/tmp/x", "operation": "replace",
                "start_line": "abc:aa", "content": "x"
            }))
            .await;
        assert!(bad_num.is_err(), "non-numeric line should error");

        // Hash too long
        let long_hash = tool
            .execute(serde_json::json!({
                "path": "/tmp/x", "operation": "replace",
                "start_line": "1:aabb", "content": "x"
            }))
            .await;
        assert!(long_hash.is_err(), "hash too long should error");
    }

    #[test]
    fn tool_definition_check() {
        let tool = make_tool_no_reads();
        assert_eq!(tool.name(), "edit_file", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "edit_file", "definition name should match");
    }

    #[tokio::test]
    async fn cannot_delete_all_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (tool, path) = setup_file(&dir, "single.txt", "only line\n").await;

        let result = tool
            .execute(serde_json::json!({
                "path": path,
                "operation": "delete",
                "start_line": anchor(1, "only line"),
            }))
            .await
            .unwrap();

        assert!(result.is_error, "deleting all lines should fail");
        assert!(
            result.output.contains("cannot delete all lines"),
            "error should mention cannot delete all"
        );
    }
}

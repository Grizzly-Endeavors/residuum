//! File reading tool for the agent.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Maximum file size to read (100KB).
const MAX_READ_BYTES: u64 = 100 * 1024;

/// Tool that reads file contents.
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file. Supports optional offset and limit \
                          parameters for reading specific line ranges."
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
                        "description": "Maximum number of lines to read (default: all)"
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

        let limit = arguments.get("limit").and_then(Value::as_u64);

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

        let contents = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("failed to read {path}: {e}"))),
        };

        let lines: Vec<&str> = contents.lines().collect();

        #[expect(
            clippy::cast_possible_truncation,
            reason = "offset from JSON u64 capped by line count"
        )]
        let start = (offset as usize).min(lines.len());
        let end = limit.map_or(lines.len(), |l| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "limit from JSON u64 capped by line count"
            )]
            let l_usize = l as usize;
            (start + l_usize).min(lines.len())
        });

        let selected: Vec<String> = lines
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4}\t{line}", start + i + 1))
            .collect();

        Ok(ToolResult::success(selected.join("\n")))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\n")
            .await
            .unwrap();

        let tool = ReadTool;
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

        let tool = ReadTool;
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
        let tool = ReadTool;
        let result = tool
            .execute(serde_json::json!({ "path": "/nonexistent/file.txt" }))
            .await
            .unwrap();

        assert!(result.is_error, "missing file should be an error result");
    }

    #[tokio::test]
    async fn read_file_missing_path() {
        let tool = ReadTool;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing path should return ToolError");
    }

    #[test]
    fn read_tool_definition() {
        let tool = ReadTool;
        assert_eq!(tool.name(), "read_file", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "read_file", "definition name should match");
    }
}

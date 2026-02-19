//! File writing tool for the agent.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Tool that writes content to files.
pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".to_string(),
            description:
                "Write content to a file. Creates parent directories if they don't exist. \
                          Overwrites the file if it already exists."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["path", "content"]
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

        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'content' argument".to_string())
            })?;

        let file_path = std::path::Path::new(path);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return Ok(ToolResult::error(format!(
                "failed to create directory {}: {e}",
                parent.display()
            )));
        }

        match tokio::fs::write(path, content).await {
            Ok(()) => Ok(ToolResult::success(format!(
                "wrote {} bytes to {path}",
                content.len()
            ))),
            Err(e) => Ok(ToolResult::error(format!("failed to write {path}: {e}"))),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        let tool = WriteTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "hello world"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "write should succeed");
        let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(contents, "hello world", "file content should match");
    }

    #[tokio::test]
    async fn write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("sub/dir/test.txt");

        let tool = WriteTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "write with nested dirs should succeed");
        let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(contents, "nested", "nested file content should match");
    }

    #[tokio::test]
    async fn write_file_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "old content").await.unwrap();

        let tool = WriteTool;
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "overwrite should succeed");
        let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(contents, "new content", "should contain new content");
    }

    #[tokio::test]
    async fn write_file_missing_path() {
        let tool = WriteTool;
        let result = tool.execute(serde_json::json!({ "content": "data" })).await;
        assert!(result.is_err(), "missing path should return ToolError");
    }

    #[tokio::test]
    async fn write_file_missing_content() {
        let tool = WriteTool;
        let result = tool
            .execute(serde_json::json!({ "path": "/tmp/test.txt" }))
            .await;
        assert!(result.is_err(), "missing content should return ToolError");
    }

    #[test]
    fn write_tool_definition() {
        let tool = WriteTool;
        assert_eq!(tool.name(), "write_file", "tool name should match");
    }
}

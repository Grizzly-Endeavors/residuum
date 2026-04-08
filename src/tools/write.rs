//! File writing tool for the agent.

use async_trait::async_trait;
use serde_json::Value;

use super::file_tracker::SharedFileTracker;
use super::path_policy::SharedPathPolicy;
use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Tool that writes content to files, enforcing read-before-overwrite.
pub struct WriteTool {
    tracker: SharedFileTracker,
    policy: SharedPathPolicy,
}

impl WriteTool {
    /// Create a new `WriteTool` with shared file tracker and path policy.
    #[must_use]
    pub fn new(tracker: SharedFileTracker, policy: SharedPathPolicy) -> Self {
        Self { tracker, policy }
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description:
                "Write content to a file. Creates parent directories if they don't exist. \
                 Overwrites the file if it already exists. Existing files must be read \
                 with read_file before overwriting."
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

        let file_content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'content' argument".to_string())
            })?;

        let file_path = std::path::Path::new(path);

        // Enforce write-scoping policy
        if let Err(reason) = self.policy.read().await.check_write(file_path) {
            return Ok(ToolResult::error(reason));
        }

        // Enforce read-before-overwrite for existing files
        if file_path.exists() && !self.tracker.lock().await.has_been_read(path) {
            return Ok(ToolResult::error(format!(
                "file {path} already exists but has not been read; use read_file before overwriting"
            )));
        }

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

        match tokio::fs::write(path, file_content).await {
            Ok(()) => {
                // Record in tracker — the agent knows the content since it just wrote it
                self.tracker.lock().await.record_read(path);
                Ok(ToolResult::success(format!(
                    "wrote {} bytes to {path}",
                    file_content.len()
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("failed to write {path}: {e}"))),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tools::file_tracker::FileTracker;
    use crate::tools::path_policy::PathPolicy;

    /// Create a permissive policy rooted at `/tmp` (allows all test writes).
    fn permissive_policy() -> SharedPathPolicy {
        PathPolicy::new_shared(std::path::PathBuf::from("/tmp"))
    }

    fn make_tool() -> WriteTool {
        WriteTool::new(FileTracker::new_shared(), permissive_policy())
    }

    fn make_tool_with_tracker() -> (WriteTool, SharedFileTracker) {
        let tracker = FileTracker::new_shared();
        let policy = permissive_policy();
        let tool = WriteTool::new(Arc::clone(&tracker), policy);
        (tool, tracker)
    }

    #[tokio::test]
    async fn write_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        let tool = make_tool();
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

        let tool = make_tool();
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
    async fn write_file_overwrites_after_read() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "old content").await.unwrap();

        let (tool, tracker) = make_tool_with_tracker();
        // Pre-record the read
        tracker
            .lock()
            .await
            .record_read(file_path.to_str().unwrap());

        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "overwrite after read should succeed");
        let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(contents, "new content", "should contain new content");
    }

    #[tokio::test]
    async fn overwrite_without_read_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        tokio::fs::write(&file_path, "original").await.unwrap();

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "overwrite attempt"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "overwrite without read should fail");
        assert!(
            result.output.contains("has not been read"),
            "error should mention read requirement"
        );
        // Verify file unchanged
        let contents = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(contents, "original", "file should be unchanged");
    }

    #[tokio::test]
    async fn new_file_without_read_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("brand_new.txt");

        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "fresh content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "new file should succeed without read");
    }

    #[tokio::test]
    async fn write_records_path_in_tracker() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tracked.txt");

        let (tool, tracker) = make_tool_with_tracker();
        tool.execute(serde_json::json!({
            "path": file_path.to_str().unwrap(),
            "content": "data"
        }))
        .await
        .unwrap();

        assert!(
            tracker
                .lock()
                .await
                .has_been_read(file_path.to_str().unwrap()),
            "write should record path in tracker"
        );
    }

    #[tokio::test]
    async fn write_file_missing_path() {
        let tool = make_tool();
        let result = tool.execute(serde_json::json!({ "content": "data" })).await;
        assert!(result.is_err(), "missing path should return ToolError");
    }

    #[tokio::test]
    async fn write_file_missing_content() {
        let tool = make_tool();
        let result = tool
            .execute(serde_json::json!({ "path": "/tmp/test.txt" }))
            .await;
        assert!(result.is_err(), "missing content should return ToolError");
    }

    #[test]
    fn write_tool_definition() {
        let tool = make_tool();
        assert_eq!(tool.name(), "write_file", "tool name should match");
    }
}

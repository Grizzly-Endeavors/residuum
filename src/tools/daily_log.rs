//! Daily log tool for explicit note-taking.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Tool that appends timestamped notes to daily log files.
pub struct DailyLogTool {
    memory_dir: PathBuf,
}

impl DailyLogTool {
    /// Create a new daily log tool targeting the given memory directory.
    #[must_use]
    pub fn new(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }
}

#[async_trait]
impl Tool for DailyLogTool {
    fn name(&self) -> &'static str {
        "daily_log"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "daily_log".to_string(),
            description: "Append a timestamped note to today's daily log file. Use this to \
                          record important observations, decisions, or context that should \
                          persist across sessions."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note": {
                        "type": "string",
                        "description": "The note to append to the daily log"
                    }
                },
                "required": ["note"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let note = arguments
            .get("note")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'note' argument".to_string())
            })?;

        if note.trim().is_empty() {
            return Ok(ToolResult::error("note cannot be empty"));
        }

        match crate::memory::daily_log::append_daily_note(&self.memory_dir, note).await {
            Ok(msg) => Ok(ToolResult::success(msg)),
            Err(e) => Ok(ToolResult::error(format!("failed to write daily log: {e}"))),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn daily_log_tool_success() {
        let dir = tempfile::tempdir().unwrap();
        let tool = DailyLogTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(serde_json::json!({"note": "test observation"}))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed");
        assert!(
            result.output.contains("note added"),
            "should confirm addition"
        );
    }

    #[tokio::test]
    async fn daily_log_tool_missing_note() {
        let dir = tempfile::tempdir().unwrap();
        let tool = DailyLogTool::new(dir.path().to_path_buf());

        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing note should be ToolError");
    }

    #[tokio::test]
    async fn daily_log_tool_empty_note() {
        let dir = tempfile::tempdir().unwrap();
        let tool = DailyLogTool::new(dir.path().to_path_buf());

        let result = tool
            .execute(serde_json::json!({"note": "  "}))
            .await
            .unwrap();

        assert!(result.is_error, "empty note should be an error result");
    }

    #[test]
    fn daily_log_tool_definition() {
        let tool = DailyLogTool::new(PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "daily_log", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "daily_log", "definition name should match");
    }
}

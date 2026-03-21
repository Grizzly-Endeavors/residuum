//! Memory get tool for retrieving episode transcripts by ID.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::memory::episode_store::{find_episode_path, read_episode_lines};
use crate::models::ToolDefinition;

/// Tool that retrieves a raw episode transcript by ID with optional line offset.
pub struct MemoryGetTool {
    episodes_dir: PathBuf,
}

impl MemoryGetTool {
    /// Create a new memory get tool with the given episodes directory.
    #[must_use]
    pub fn new(episodes_dir: PathBuf) -> Self {
        Self { episodes_dir }
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &'static str {
        "memory_get"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_get".to_string(),
            description: "Retrieve a raw episode transcript by ID. Use after memory_search to \
                          drill into the full conversation transcript of a specific episode. \
                          Returns formatted message lines with role labels and line numbers."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "episode_id": {
                        "type": "string",
                        "description": "The episode ID to retrieve (e.g., \"ep-001\")"
                    },
                    "from_line": {
                        "type": "integer",
                        "description": "Start reading from this line offset (1-indexed, default: start)"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "Number of message lines to return (default: 50, max: 200)"
                    }
                },
                "required": ["episode_id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let episode_id = arguments
            .get("episode_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'episode_id' argument".to_string())
            })?;

        if episode_id.trim().is_empty() {
            return Ok(ToolResult::error("episode_id cannot be empty"));
        }

        if episode_id.contains('/') || episode_id.contains('\\') || episode_id.contains("..") {
            return Ok(ToolResult::error(
                "episode_id contains invalid characters (path traversal rejected)",
            ));
        }

        let from_line = arguments
            .get("from_line")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok());

        let lines = arguments
            .get("lines")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok());

        let path = match find_episode_path(&self.episodes_dir, episode_id) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return Ok(ToolResult::error(format!(
                    "episode '{episode_id}' not found"
                )));
            }
            Err(e) => {
                tracing::error!(error = %e, episode_id = %episode_id, "failed to search for episode");
                return Ok(ToolResult::error(format!(
                    "failed to search for episode: {e}"
                )));
            }
        };

        match read_episode_lines(&path, from_line, lines).await {
            Ok(output) => Ok(ToolResult::success(output)),
            Err(e) => {
                tracing::error!(error = %e, episode_id = %episode_id, "failed to read episode transcript");
                Ok(ToolResult::error(format!(
                    "failed to read episode transcript: {e}"
                )))
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::episode_store::write_episode_transcript;
    use crate::memory::types::Episode;
    use crate::models::Message;
    use chrono::NaiveDate;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            start: "user asked about files".to_string(),
            end: "listed directory contents".to_string(),
            context: "general".to_string(),
            observations: vec!["user prefers concise output".to_string()],
            source_episodes: vec![],
        }
    }

    async fn setup_tool() -> (tempfile::TempDir, MemoryGetTool) {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![
            Message::user("hello"),
            Message::assistant("world", None),
            Message::user("thanks"),
        ];
        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();

        let tool = MemoryGetTool::new(dir.path().to_path_buf());
        (dir, tool)
    }

    #[test]
    fn tool_definition_correctness() {
        let tool = MemoryGetTool::new(PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "memory_get", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "memory_get", "definition name should match");
        assert!(
            def.description.contains("episode transcript"),
            "description should mention transcripts"
        );
    }

    #[tokio::test]
    async fn successful_retrieval() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"episode_id": "ep-001"}))
            .await
            .unwrap();

        assert!(!result.is_error, "retrieval should succeed");
        assert!(
            result.output.contains("Episode: ep-001"),
            "should have header"
        );
        assert!(
            result.output.contains("[line 2] User: hello"),
            "should show messages"
        );
    }

    #[tokio::test]
    async fn retrieval_with_offset() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({
                "episode_id": "ep-001",
                "from_line": 2,
                "lines": 1
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "retrieval with offset should succeed");
        assert!(
            result.output.contains("Episode: ep-001"),
            "header always shown"
        );
        assert!(
            result.output.contains("showing lines"),
            "should have footer"
        );
    }

    #[tokio::test]
    async fn episode_not_found() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"episode_id": "ep-999"}))
            .await
            .unwrap();

        assert!(result.is_error, "missing episode should be error result");
        assert!(
            result.output.contains("not found"),
            "should report not found"
        );
    }

    #[tokio::test]
    async fn missing_episode_id_argument() {
        let (_dir, tool) = setup_tool().await;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing episode_id should be ToolError");
    }

    #[tokio::test]
    async fn path_traversal_rejection() {
        let (_dir, tool) = setup_tool().await;

        for bad_id in ["../etc/passwd", "ep-001/../../etc", "ep\\001"] {
            let result = tool
                .execute(serde_json::json!({"episode_id": bad_id}))
                .await
                .unwrap();
            assert!(
                result.is_error,
                "path traversal should be rejected: {bad_id}"
            );
            assert!(
                result.output.contains("path traversal"),
                "error should mention path traversal: {bad_id}"
            );
        }
    }

    #[tokio::test]
    async fn empty_episode_id_rejection() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"episode_id": "  "}))
            .await
            .unwrap();
        assert!(result.is_error, "empty episode_id should be error result");
        assert!(
            result.output.contains("cannot be empty"),
            "should report empty"
        );
    }
}

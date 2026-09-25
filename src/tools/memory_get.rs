//! Memory get tool for retrieving episode or session-run transcripts by ID.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolError, ToolResult};
use crate::background::store::{RunRecord, SessionStore};
use crate::inference::{Message, ToolDefinition};
use crate::memory::episode_store::{
    DEFAULT_LINES, MAX_LINES, find_episode_path, format_message_line, read_episode_lines,
};

/// Tool that retrieves a raw transcript by episode ID or session run ID, with
/// an optional line offset.
pub struct MemoryGetTool {
    episodes_dir: PathBuf,
    session_store: SessionStore,
}

/// Reject a path-traversal-shaped identifier before it reaches the filesystem.
fn rejects_path_traversal(id: &str) -> bool {
    id.contains('/') || id.contains('\\') || id.contains("..")
}

/// Format a run's transcript for LLM consumption, in the same bounded,
/// line-numbered style as [`read_episode_lines`].
fn format_run_transcript(
    record: &RunRecord,
    messages: &[Message],
    from_line: Option<usize>,
    request_limit: Option<usize>,
) -> String {
    let total = messages.len();
    let limit = request_limit.map_or(DEFAULT_LINES, |l| l.clamp(1, MAX_LINES));
    // from_line is 1-indexed; message lines start at index 0.
    let start_idx = from_line.map_or(0, |f| f.saturating_sub(1));
    let end_idx = total.min(start_idx + limit);

    let mut parts: Vec<String> = Vec::new();

    let episode_suffix = record
        .episode_id
        .as_deref()
        .map_or_else(String::new, |id| format!(" | episode: {id}"));
    parts.push(format!(
        "Run: {} | address: {} | category: {} | state: {}{episode_suffix}",
        record.run_id, record.address, record.category, record.state
    ));
    parts.push(String::new());

    for (idx, msg) in messages.iter().enumerate().take(end_idx).skip(start_idx) {
        format_message_line(&mut parts, idx + 1, msg);
    }

    if start_idx >= total && start_idx > 0 {
        parts.push(format!(
            "--- line {} is past the end; the transcript has {total} lines ---",
            start_idx + 1,
        ));
    } else if start_idx > 0 || end_idx < total {
        parts.push(format!(
            "--- showing lines {}-{end_idx} of {total} total ---",
            start_idx + 1,
        ));
    }

    parts.join("\n")
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &'static str {
        "memory_get"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Retrieve a raw transcript by episode ID or session run ID — provide \
                          exactly one of the two. Use episode_id after memory_search to drill \
                          into a merged episode's full conversation. Use run_id to read a \
                          session run's transcript directly from the session store — e.g. to \
                          follow a resume pointer to a run that produced no episode, or to check \
                          on a run that's still in progress. Returns formatted message lines \
                          with role labels and line numbers."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "episode_id": {
                        "type": "string",
                        "description": "The episode ID to retrieve (e.g., \"ep-001\")"
                    },
                    "run_id": {
                        "type": "string",
                        "description": "The session run ID to retrieve (e.g., \"run-1234567890-abcd1234\")"
                    },
                    "from_line": {
                        "type": "integer",
                        "description": "Start reading from this line offset (1-indexed, default: start)"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "Number of message lines to return (default: 50, max: 200)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let episode_id = arguments.get("episode_id").and_then(Value::as_str);
        let run_id = arguments.get("run_id").and_then(Value::as_str);

        let from_line = arguments
            .get("from_line")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok());

        let lines = arguments
            .get("lines")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok());

        match (episode_id, run_id) {
            (Some(_), Some(_)) => Err(ToolError::InvalidArguments(
                "provide exactly one of 'episode_id' or 'run_id', not both".to_string(),
            )),
            (None, None) => Err(ToolError::InvalidArguments(
                "missing required 'episode_id' or 'run_id' argument".to_string(),
            )),
            (Some(episode_id), None) => self.get_episode(episode_id, from_line, lines).await,
            (None, Some(run_id)) => self.get_run(run_id, from_line, lines).await,
        }
    }
}

impl MemoryGetTool {
    /// Create a new memory get tool with the given episodes and sessions
    /// directories.
    #[must_use]
    pub fn new(episodes_dir: PathBuf, sessions_dir: PathBuf) -> Self {
        Self {
            episodes_dir,
            session_store: SessionStore::new(sessions_dir),
        }
    }

    async fn get_episode(
        &self,
        episode_id: &str,
        from_line: Option<usize>,
        lines: Option<usize>,
    ) -> Result<ToolResult, ToolError> {
        if episode_id.trim().is_empty() {
            return Ok(ToolResult::error("episode_id cannot be empty"));
        }
        if rejects_path_traversal(episode_id) {
            return Ok(ToolResult::error(
                "episode_id contains invalid characters (path traversal rejected)",
            ));
        }

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

    async fn get_run(
        &self,
        run_id: &str,
        from_line: Option<usize>,
        lines: Option<usize>,
    ) -> Result<ToolResult, ToolError> {
        if run_id.trim().is_empty() {
            return Ok(ToolResult::error("run_id cannot be empty"));
        }
        if rejects_path_traversal(run_id) {
            return Ok(ToolResult::error(
                "run_id contains invalid characters (path traversal rejected)",
            ));
        }

        match self.session_store.read_run(run_id).await {
            Ok(Some((record, messages))) => Ok(ToolResult::success(format_run_transcript(
                &record, &messages, from_line, lines,
            ))),
            Ok(None) => Ok(ToolResult::error(format!(
                "run '{run_id}' not found; use list_agents to find live session addresses, or \
                 memory_search for merged episodes"
            ))),
            Err(e) => {
                tracing::error!(error = %e, run_id = %run_id, "failed to read session run transcript");
                Ok(ToolResult::error(format!(
                    "failed to read run transcript: {e}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::{SessionCategory, SessionState};
    use crate::background::store::SessionStore;
    use crate::bus::{EventTrigger, SessionAddress};
    use crate::memory::episode_store::write_episode_transcript;
    use crate::memory::types::Episode;
    use chrono::NaiveDate;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            observations: vec!["user prefers concise output".to_string()],
        }
    }

    fn sample_session_info(run_id: &str) -> crate::background::registry::SessionInfo {
        crate::background::registry::SessionInfo {
            address: SessionAddress::from("spawned-researcher-0001"),
            run_id: run_id.to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from("main")),
            depth: 1,
            purpose: "research the thing".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: None,
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
            overlap: None,
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

        let tool = MemoryGetTool::new(dir.path().to_path_buf(), dir.path().join("sessions"));
        (dir, tool)
    }

    #[test]
    fn tool_definition_correctness() {
        let tool = MemoryGetTool::new(
            PathBuf::from("/tmp/episodes"),
            PathBuf::from("/tmp/sessions"),
        );
        assert_eq!(tool.name(), "memory_get", "tool name should match");
        let def = tool.definition();
        assert_eq!(def.name, "memory_get", "definition name should match");
        assert!(
            def.description.contains("episode ID"),
            "description should mention episode ID"
        );
        assert!(
            def.description.contains("run_id"),
            "description should mention run_id"
        );
    }

    #[test]
    fn episode_id_and_run_id_are_mutually_exclusive_and_optional_in_schema() {
        let tool = MemoryGetTool::new(
            PathBuf::from("/tmp/episodes"),
            PathBuf::from("/tmp/sessions"),
        );
        let params = tool.definition().parameters;
        let required = params.get("required");
        assert!(
            required.is_none() || required.unwrap().as_array().unwrap().is_empty(),
            "neither episode_id nor run_id should be unconditionally required"
        );
    }

    #[tokio::test]
    async fn missing_both_ids_is_an_error() {
        let (_dir, tool) = setup_tool().await;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "should error when neither id is given");
    }

    #[tokio::test]
    async fn providing_both_ids_is_an_error() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"episode_id": "ep-001", "run_id": "run-1"}))
            .await;
        assert!(result.is_err(), "should error when both ids are given");
    }

    #[tokio::test]
    async fn run_id_retrieves_a_completed_runs_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        let store = SessionStore::new(sessions_dir.clone());
        let info = sample_session_info("run-test-001");
        store.begin_run(&info).await;
        store
            .complete_run(
                &info,
                "completed",
                vec![
                    Message::user("investigate"),
                    Message::assistant("found it", None),
                ],
                Some("ep-042".to_string()),
            )
            .await;

        let tool = MemoryGetTool::new(dir.path().join("episodes"), sessions_dir);
        let result = tool
            .execute(serde_json::json!({"run_id": "run-test-001"}))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "retrieval should succeed: {}",
            result.output
        );
        assert!(result.output.contains("run-test-001"));
        assert!(result.output.contains("ep-042"));
        assert!(result.output.contains("[line 1] User: investigate"));
        assert!(result.output.contains("[line 2] Assistant: found it"));
    }

    #[tokio::test]
    async fn run_id_from_line_past_the_end_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        let store = SessionStore::new(sessions_dir.clone());
        let info = sample_session_info("run-test-past-end");
        store.begin_run(&info).await;
        store
            .complete_run(
                &info,
                "completed",
                vec![Message::user("one"), Message::assistant("two", None)],
                None,
            )
            .await;

        let tool = MemoryGetTool::new(dir.path().join("episodes"), sessions_dir);
        let result = tool
            .execute(serde_json::json!({"run_id": "run-test-past-end", "from_line": 50}))
            .await
            .unwrap();

        assert!(!result.is_error, "{}", result.output);
        assert!(
            result
                .output
                .contains("line 50 is past the end; the transcript has 2 lines"),
            "{}",
            result.output
        );
        assert!(
            !result.output.contains("showing lines"),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn run_id_retrieves_a_live_runs_incremental_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        let store = SessionStore::new(sessions_dir.clone());
        let info = sample_session_info("run-test-002");
        store.begin_run(&info).await;
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[Message::user("still going")],
            )
            .await;

        let tool = MemoryGetTool::new(dir.path().join("episodes"), sessions_dir);
        let result = tool
            .execute(serde_json::json!({"run_id": "run-test-002"}))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "retrieval should succeed: {}",
            result.output
        );
        assert!(result.output.contains("state: running"));
        assert!(result.output.contains("still going"));
    }

    #[tokio::test]
    async fn unknown_run_id_gives_an_actionable_error() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"run_id": "run-does-not-exist"}))
            .await
            .unwrap();

        assert!(result.is_error, "unknown run should be an error result");
        assert!(result.output.contains("not found"), "{}", result.output);
        assert!(
            result.output.contains("list_agents"),
            "error should point toward discovery, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn empty_run_id_rejection() {
        let (_dir, tool) = setup_tool().await;
        let result = tool
            .execute(serde_json::json!({"run_id": "  "}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("cannot be empty"));
    }

    #[tokio::test]
    async fn run_id_path_traversal_rejection() {
        let (_dir, tool) = setup_tool().await;
        for bad_id in ["../etc/passwd", "run-1/../../etc", "run\\1"] {
            let result = tool
                .execute(serde_json::json!({"run_id": bad_id}))
                .await
                .unwrap();
            assert!(
                result.is_error,
                "path traversal should be rejected: {bad_id}"
            );
            assert!(result.output.contains("path traversal"));
        }
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
        // from_line=2 skips line 2 (user "hello") and shows line 3 (assistant "world")
        assert!(
            result.output.contains("[line 3] Assistant: world"),
            "should contain line 3 content: {}",
            result.output
        );
        assert!(
            !result.output.contains("[line 2]"),
            "should not contain line 2 content: {}",
            result.output
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

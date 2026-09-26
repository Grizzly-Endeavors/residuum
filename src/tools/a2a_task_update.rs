//! `a2a_task_update`: lets a session started from the `a2a` endpoint report
//! its delegated task's outcome to the A2A caller waiting on it. Registered
//! only for those sessions — see `docs/systems-usage/a2a.md`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::bus::{A2aTaskSignalEvent, A2aTaskSignalState, Publisher, SessionAddress, topics};
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Hard cap on one artifact file's size (matches the plan's ~20 MB budget).
const MAX_ARTIFACT_BYTES: u64 = 20 * 1024 * 1024;

/// Reports an A2A task's outcome (completed, needs input, or failed) to the
/// caller that delegated it, publishing an [`A2aTaskSignalEvent`] the
/// session's [`crate::a2a::executor::SessionExecutor`] is waiting on.
pub struct A2aTaskUpdateTool {
    address: SessionAddress,
    publisher: Publisher,
    workspace_dir: PathBuf,
}

impl A2aTaskUpdateTool {
    /// `address` is the session's own address — the same one the executor
    /// filters incoming signals by.
    #[must_use]
    pub fn new(address: SessionAddress, publisher: Publisher, workspace_dir: PathBuf) -> Self {
        Self {
            address,
            publisher,
            workspace_dir,
        }
    }
}

/// Resolve `relative` against `workspace_dir`, refusing a path that escapes
/// the workspace or doesn't exist.
async fn resolve_artifact_path(workspace_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.trim().is_empty() {
        return Err("artifact path must not be empty".to_string());
    }
    let canonical_root = tokio::fs::canonicalize(workspace_dir)
        .await
        .map_err(|e| format!("failed to resolve the workspace root: {e}"))?;
    let candidate = workspace_dir.join(relative);
    let canonical_target = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|e| format!("'{relative}' does not exist in the workspace: {e}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(format!("'{relative}' is outside the workspace"));
    }
    Ok(canonical_target)
}

/// Read `relative` (validated against `workspace_dir`) into an [`a2a::Part`],
/// as UTF-8 text when possible and raw bytes with a detected media type
/// otherwise, capped at [`MAX_ARTIFACT_BYTES`].
async fn build_artifact(workspace_dir: &Path, relative: &str) -> Result<a2a::Artifact, String> {
    let resolved = resolve_artifact_path(workspace_dir, relative).await?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|e| format!("failed to read '{relative}': {e}"))?;
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "'{relative}' is {} bytes, over the {} MB limit",
            metadata.len(),
            MAX_ARTIFACT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(|e| format!("failed to read '{relative}': {e}"))?;
    let filename = resolved.file_name().map_or_else(
        || relative.to_string(),
        |name| name.to_string_lossy().to_string(),
    );

    let part = if let Ok(text) = String::from_utf8(bytes.clone()) {
        a2a::Part::text(text).with_filename(filename.clone())
    } else {
        let media_type = crate::interfaces::attachment::detect_mime_type(&resolved);
        a2a::Part::raw(bytes)
            .with_filename(filename.clone())
            .with_media_type(media_type)
    };

    Ok(a2a::Artifact {
        artifact_id: a2a::new_artifact_id(),
        name: Some(filename),
        description: None,
        parts: vec![part],
        metadata: None,
        extensions: None,
    })
}

#[async_trait]
impl Tool for A2aTaskUpdateTool {
    fn name(&self) -> &'static str {
        "a2a_task_update"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Report this A2A task's outcome to the caller that delegated it. Call \
                this when the delegated task is done, when you need more input from the caller \
                before you can continue, or when the task cannot be completed. Your final answer \
                for the caller goes in `message` — the caller only ever sees what you put there, \
                not the rest of your turn output."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["completed", "input_required", "failed"],
                        "description": "\"completed\" when the work is done, \"input_required\" \
                            when you need more information from the caller before continuing, \
                            \"failed\" when the task cannot be completed."
                    },
                    "message": {
                        "type": "string",
                        "description": "The message the caller sees: your final answer for \
                            \"completed\", the question for \"input_required\", or an explanation \
                            for \"failed\"."
                    },
                    "artifacts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace-relative paths of files to attach as artifacts, \
                            if any."
                    }
                },
                "required": ["state", "message"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let state = match super::require_str(&arguments, "state")? {
            "completed" => A2aTaskSignalState::Completed,
            "input_required" => A2aTaskSignalState::InputRequired,
            "failed" => A2aTaskSignalState::Failed,
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "unknown state '{other}'; expected completed, input_required, or failed"
                )));
            }
        };
        let message = super::require_str(&arguments, "message")?.to_string();
        if message.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "message must not be empty".to_string(),
            ));
        }
        let artifact_paths: Vec<String> = arguments
            .get("artifacts")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut artifacts = Vec::with_capacity(artifact_paths.len());
        for relative in &artifact_paths {
            match build_artifact(&self.workspace_dir, relative).await {
                Ok(artifact) => artifacts.push(artifact),
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "failed to attach artifact '{relative}': {e}"
                    )));
                }
            }
        }

        let event = A2aTaskSignalEvent {
            address: self.address.clone(),
            state,
            message,
            artifacts,
        };
        if let Err(e) = self.publisher.publish(topics::A2aTaskSignal, event).await {
            return Ok(ToolResult::error(format!(
                "failed to report the task outcome: {e}"
            )));
        }

        let verb = match state {
            A2aTaskSignalState::Completed => "completed",
            A2aTaskSignalState::InputRequired => "marked as needing more input",
            A2aTaskSignalState::Failed => "marked failed",
        };
        Ok(ToolResult::success(format!(
            "Task marked {verb}; the caller has been notified."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Subscriber, spawn_broker};

    fn tool(address: &str, workspace_dir: PathBuf, publisher: Publisher) -> A2aTaskUpdateTool {
        A2aTaskUpdateTool::new(SessionAddress::from(address), publisher, workspace_dir)
    }

    #[tokio::test]
    async fn publishes_a_completed_signal_with_no_artifacts() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let mut sub: Subscriber<A2aTaskSignalEvent> =
            bus.subscribe(topics::A2aTaskSignal).await.unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );

        let result = tool
            .execute(serde_json::json!({ "state": "completed", "message": "all done" }))
            .await
            .unwrap();
        assert!(!result.is_error, "got: {}", result.output);
        assert!(result.output.contains("Task marked completed"));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address.as_ref(), "external-a2a-0001");
        assert_eq!(event.state, A2aTaskSignalState::Completed);
        assert_eq!(event.message, "all done");
        assert!(event.artifacts.is_empty());
    }

    #[tokio::test]
    async fn rejects_an_unknown_state() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );
        let err = tool
            .execute(serde_json::json!({ "state": "done", "message": "x" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn rejects_an_empty_message() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );
        let err = tool
            .execute(serde_json::json!({ "state": "completed", "message": "  " }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn attaches_a_text_artifact_with_its_filename() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.md"), "# Report\ndone").unwrap();
        let mut sub: Subscriber<A2aTaskSignalEvent> =
            bus.subscribe(topics::A2aTaskSignal).await.unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );

        let result = tool
            .execute(serde_json::json!({
                "state": "completed",
                "message": "see report",
                "artifacts": ["report.md"]
            }))
            .await
            .unwrap();
        assert!(!result.is_error, "got: {}", result.output);

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.artifacts.len(), 1);
        assert_eq!(
            event.artifacts.first().unwrap().name.as_deref(),
            Some("report.md")
        );
        assert_eq!(
            event
                .artifacts
                .first()
                .unwrap()
                .parts
                .first()
                .unwrap()
                .as_text(),
            Some("# Report\ndone")
        );
    }

    #[tokio::test]
    async fn refuses_a_path_outside_the_workspace() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "nope").unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );

        let result = tool
            .execute(serde_json::json!({
                "state": "completed",
                "message": "see report",
                "artifacts": ["../secret.txt"]
            }))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("failed to attach artifact"));
    }

    #[tokio::test]
    async fn refuses_a_missing_artifact() {
        let bus = spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let tool = tool(
            "external-a2a-0001",
            dir.path().to_path_buf(),
            bus.publisher(),
        );

        let result = tool
            .execute(serde_json::json!({
                "state": "completed",
                "message": "see report",
                "artifacts": ["nope.txt"]
            }))
            .await
            .unwrap();
        assert!(result.is_error);
    }
}

//! Background task management tools: `stop_agent` and `list_agents`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::background::BackgroundTaskSpawner;
use crate::models::ToolDefinition;
use crate::notify::types::TaskSource;

use super::{Tool, ToolError, ToolResult};

// ─── StopAgentTool ───────────────────────────────────────────────────────────

/// Tool for cancelling a running background task by ID.
pub struct StopAgentTool {
    spawner: Arc<BackgroundTaskSpawner>,
}

impl StopAgentTool {
    /// Create a new `StopAgentTool`.
    #[must_use]
    pub fn new(spawner: Arc<BackgroundTaskSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for StopAgentTool {
    fn name(&self) -> &'static str {
        "stop_agent"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "stop_agent".to_string(),
            description: "Cancel a running background task by ID. Returns an error if no task with that ID is active. Use list_agents to find active task IDs.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the background task to cancel"
                    }
                },
                "required": ["task_id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let task_id = arguments
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("task_id is required".to_string()))?;

        if self.spawner.cancel(task_id).await {
            Ok(ToolResult::success(format!("Cancelled task {task_id}.")))
        } else {
            Ok(ToolResult::error(format!(
                "No active task with id {task_id}."
            )))
        }
    }
}

// ─── ListAgentsTool ──────────────────────────────────────────────────────────

/// Tool for listing all currently running background tasks.
pub struct ListAgentsTool {
    spawner: Arc<BackgroundTaskSpawner>,
}

impl ListAgentsTool {
    /// Create a new `ListAgentsTool`.
    #[must_use]
    pub fn new(spawner: Arc<BackgroundTaskSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &'static str {
        "list_agents"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_agents".to_string(),
            description: "List all currently running background tasks with their IDs, types, sources, prompt previews, and elapsed time.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let tasks = self.spawner.list_active_tasks().await;

        if tasks.is_empty() {
            return Ok(ToolResult::success("No active background tasks."));
        }

        let now = Utc::now();
        let mut lines = vec![format!("{} active task(s):", tasks.len())];

        for (id, info) in &tasks {
            let elapsed_secs = (now - info.started_at).num_seconds().max(0);
            let source_label = match info.source {
                TaskSource::Pulse => "pulse",
                TaskSource::Cron => "cron",
                TaskSource::Agent => "agent",
            };
            let preview_suffix = if info.prompt_preview.is_empty() {
                String::new()
            } else {
                format!("\n    preview: {}", info.prompt_preview)
            };
            lines.push(format!(
                "  [{id}] {task} — type: {etype} — source: {src} — running {elapsed}s{sfx}",
                task = info.task_name,
                etype = info.execution_type,
                src = source_label,
                elapsed = elapsed_secs,
                sfx = preview_suffix,
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

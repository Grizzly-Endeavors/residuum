//! Background task management tools: `stop_agent`, `list_agents`, and `subagent_spawn`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::background::BackgroundTaskSpawner;
use crate::config::BackgroundModelTier;
use crate::inference::ToolDefinition;
use crate::skills::SharedSkillState;

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
            name: self.name().to_string(),
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
        let task_id = super::require_str(&arguments, "task_id")?;

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
            name: self.name().to_string(),
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
            let source_kind = info.source.as_str();
            let preview_suffix = if info.prompt_preview.is_empty() {
                String::new()
            } else {
                format!("\n    preview: {}", info.prompt_preview)
            };
            lines.push(format!(
                "  [{id}] {task} — type: sub_agent — source: {src} — running {elapsed}s{sfx}",
                task = info.source_label,
                src = source_kind,
                elapsed = elapsed_secs,
                sfx = preview_suffix,
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── SubagentSpawnTool ──────────────────────────────────────────────────────

/// Tool for spawning background sub-agents on demand.
pub struct SubagentSpawnTool {
    publisher: crate::bus::Publisher,
    /// Main agent skill state — read to validate a requested skill name.
    skill_state: SharedSkillState,
}

impl SubagentSpawnTool {
    /// Create a new `SubagentSpawnTool`.
    #[must_use]
    pub(crate) fn new(publisher: crate::bus::Publisher, skill_state: SharedSkillState) -> Self {
        Self {
            publisher,
            skill_state,
        }
    }
}

#[async_trait]
impl Tool for SubagentSpawnTool {
    fn name(&self) -> &'static str {
        "subagent_spawn"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Spawn a background sub-agent to handle a task. Optionally name a skill to give the sub-agent a role — its instructions become the sub-agent's brief. Runs asynchronously; the result is relayed back to you when the sub-agent finishes. A sub-agent's result is its own self-report, not verified fact — for verifiable work, ask the sub-agent to return concrete handles (file paths, IDs, URLs) and verify them yourself before relying on the result.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The prompt/instructions for the sub-agent"
                    },
                    "skill": {
                        "type": "string",
                        "description": "Name of a skill to activate for the sub-agent, giving it a role. Omit to run a plain sub-agent on the task prompt alone."
                    },
                    "model": {
                        "type": "string",
                        "enum": ["small", "medium", "large"],
                        "description": "Model tier for the sub-agent (default: \"medium\")."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let task_prompt = super::require_str(&arguments, "task")?;

        if task_prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task must not be empty".to_string(),
            ));
        }

        let skill_name = arguments.get("skill").and_then(Value::as_str);

        if let Some(name) = skill_name {
            if name.eq_ignore_ascii_case("main") {
                return Err(ToolError::InvalidArguments(
                    "\"main\" is reserved for scheduled tasks (pulse/actions). Name a skill instead."
                        .to_string(),
                ));
            }

            // Validate against the in-memory skill index so an unknown name fails
            // here, rather than surfacing later as a failed background result.
            let state = self.skill_state.lock().await;
            if state.index().find_by_name(name).is_none() {
                let available: Vec<&str> = state
                    .index()
                    .entries()
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect();
                return Ok(ToolResult::error(format!(
                    "unknown skill '{name}'. Available: {}",
                    available.join(", ")
                )));
            }
        }

        let model_tier = match arguments.get("model").and_then(Value::as_str) {
            Some(s) => parse_model_tier(s)?,
            None => BackgroundModelTier::Medium,
        };

        let spawn_event = crate::bus::SpawnRequestEvent {
            skill: skill_name.map(crate::bus::SkillName::from),
            source_label: format!("agent:{}", skill_name.unwrap_or("subagent")),
            prompt: task_prompt.to_string(),
            context: None,
            source: crate::bus::EventTrigger::Agent,
            model_tier,
            include_identity: false,
        };

        self.publisher
            .publish(crate::bus::topics::Background, spawn_event)
            .await
            .map_err(|err| {
                tracing::error!(error = %err, skill = skill_name.unwrap_or("none"), "failed to publish spawn request");
                ToolError::Execution(format!("failed to publish spawn request: {err}"))
            })?;

        Ok(ToolResult::success(match skill_name {
            Some(name) => format!("Sub-agent spawned with skill '{name}'."),
            None => "Sub-agent spawned.".to_string(),
        }))
    }
}

fn parse_model_tier(s: &str) -> Result<BackgroundModelTier, ToolError> {
    s.parse::<BackgroundModelTier>()
        .map_err(ToolError::InvalidArguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillIndex, SkillState};

    fn make_tool() -> SubagentSpawnTool {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        SubagentSpawnTool::new(publisher, skill_state)
    }

    #[test]
    fn model_tier_parsing_valid() {
        assert!(matches!(
            parse_model_tier("small"),
            Ok(BackgroundModelTier::Small)
        ));
        assert!(matches!(
            parse_model_tier("medium"),
            Ok(BackgroundModelTier::Medium)
        ));
        assert!(matches!(
            parse_model_tier("large"),
            Ok(BackgroundModelTier::Large)
        ));
    }

    #[test]
    fn model_tier_parsing_invalid() {
        assert!(parse_model_tier("invalid").is_err());
        assert!(parse_model_tier("SMALL").is_err());
    }

    #[tokio::test]
    async fn task_required() {
        let tool = make_tool();

        // Missing task
        let missing_result = tool.execute(serde_json::json!({})).await;
        assert!(missing_result.is_err(), "should error on missing task");

        // Empty task
        let empty_result = tool.execute(serde_json::json!({"task": "  "})).await;
        assert!(empty_result.is_err(), "should error on empty task");
    }

    #[tokio::test]
    async fn main_skill_name_rejected() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "main"
            }))
            .await;

        assert!(result.is_err(), "\"main\" should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("reserved"),
            "error should mention 'reserved', got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn main_skill_name_rejected_case_insensitive() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "MAIN"
            }))
            .await;

        assert!(result.is_err(), "\"MAIN\" should also be rejected");
    }

    #[tokio::test]
    async fn unknown_skill_name_returns_error() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "skill": "definitely-not-a-real-skill"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "unknown skill should return a tool error");
        assert!(
            result.output.contains("unknown skill"),
            "error should mention unknown skill, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn spawns_without_a_skill() {
        // Omitting `skill` spawns a plain sub-agent — no index lookup, no error.
        let tool = make_tool();

        let res = tool
            .execute(serde_json::json!({
                "task": "do something"
            }))
            .await
            .unwrap();

        assert!(
            !res.is_error,
            "spawning without a skill should succeed, got: {}",
            res.output
        );
    }
}

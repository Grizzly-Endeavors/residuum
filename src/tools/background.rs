//! Background task management tools: `stop_agent`, `list_agents`, and `subagent_spawn`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use crate::background::BackgroundTaskSpawner;
use crate::config::BackgroundModelTier;
use crate::models::ToolDefinition;
use crate::subagents::SubagentPresetIndex;

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
            let source_kind = info.source.as_str();
            let preview_suffix = if info.prompt_preview.is_empty() {
                String::new()
            } else {
                format!("\n    preview: {}", info.prompt_preview)
            };
            lines.push(format!(
                "  [{id}] {task} — type: {etype} — source: {src} — running {elapsed}s{sfx}",
                task = info.source_label,
                etype = info.execution_type,
                src = source_kind,
                elapsed = elapsed_secs,
                sfx = preview_suffix,
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── SubAgentSpawnTool ──────────────────────────────────────────────────────

/// Tool for spawning background sub-agents on demand.
pub struct SubAgentSpawnTool {
    publisher: crate::bus::Publisher,
    subagents_dir: PathBuf,
}

impl SubAgentSpawnTool {
    /// Create a new `SubAgentSpawnTool`.
    #[must_use]
    pub(crate) fn new(publisher: crate::bus::Publisher, subagents_dir: PathBuf) -> Self {
        Self {
            publisher,
            subagents_dir,
        }
    }
}

#[async_trait]
impl Tool for SubAgentSpawnTool {
    fn name(&self) -> &'static str {
        "subagent_spawn"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "subagent_spawn".to_string(),
            description: "Spawn a background sub-agent to handle a task. The agent_name selects a preset that configures the sub-agent's instructions, model tier, and tool restrictions. Unknown preset names fail immediately with a list of available presets. Runs asynchronously; results are routed by the notification router based on content and ALERTS.md policy.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The prompt/instructions for the sub-agent"
                    },
                    "agent_name": {
                        "type": "string",
                        "description": "Preset name to use (default: \"general-purpose\"). Must match a known preset or the call fails."
                    },
                    "model_override": {
                        "type": "string",
                        "enum": ["small", "medium", "large"],
                        "description": "Override the preset's model tier. If omitted, the preset's tier is used (default: \"medium\")."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let task_prompt = arguments
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("task is required".to_string()))?;

        if task_prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task must not be empty".to_string(),
            ));
        }

        let preset_name = arguments
            .get("agent_name")
            .and_then(Value::as_str)
            .unwrap_or("general-purpose");

        if preset_name.eq_ignore_ascii_case("main") {
            return Err(ToolError::InvalidArguments(
                "\"main\" is reserved for scheduled tasks (pulse/actions). Use a named preset instead."
                    .to_string(),
            ));
        }
        let explicit_model_override = arguments.get("model_override").and_then(Value::as_str);

        let resolved =
            match resolve_spawn_params(&self.subagents_dir, preset_name, explicit_model_override)
                .await
            {
                Ok(r) => r,
                Err(tool_result) => return Ok(tool_result),
            };

        let spawn_event = crate::bus::SpawnRequestEvent {
            source_label: format!("agent:{}", resolved.preset_name),
            prompt: task_prompt.to_string(),
            context: None,
            source: crate::bus::EventTrigger::Agent,
            model_tier_override: Some(resolved.tier),
        };

        let topic = crate::bus::TopicId::AgentPreset(crate::bus::PresetName::from(preset_name));

        self.publisher
            .publish(topic, crate::bus::BusEvent::SpawnRequest(spawn_event))
            .await
            .map_err(|err| {
                ToolError::Execution(format!("failed to publish spawn request: {err}"))
            })?;

        Ok(ToolResult::success(format!(
            "Subagent '{preset_name}' spawned with task delegated to registry."
        )))
    }
}

/// Resolved spawn parameters after loading a preset.
struct ResolvedSpawn {
    preset_name: String,
    tier: BackgroundModelTier,
}

/// Load a preset and resolve tier defaults from arguments + preset.
async fn resolve_spawn_params(
    subagents_dir: &std::path::Path,
    preset_name: &str,
    explicit_model_override: Option<&str>,
) -> Result<ResolvedSpawn, ToolResult> {
    let index = SubagentPresetIndex::scan(subagents_dir)
        .await
        .map_err(|e| ToolResult::error(format!("failed to load subagent presets: {e}")))?;

    let (preset_fm, _preset_body) = index
        .load_preset(preset_name)
        .await
        .map_err(|e| ToolResult::error(e.to_string()))?;

    let tier = if let Some(s) = explicit_model_override {
        parse_model_tier(s).map_err(|e| ToolResult::error(e.to_string()))?
    } else if let Some(tier_str) = preset_fm.model_tier.as_deref() {
        match parse_model_tier(tier_str) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(preset = %preset_name, tier = %tier_str, error = %e, "invalid model_tier in preset, falling back to medium");
                BackgroundModelTier::Medium
            }
        }
    } else {
        BackgroundModelTier::Medium
    };

    Ok(ResolvedSpawn {
        preset_name: preset_name.to_string(),
        tier,
    })
}

fn parse_model_tier(s: &str) -> Result<BackgroundModelTier, ToolError> {
    s.parse::<BackgroundModelTier>()
        .map_err(ToolError::InvalidArguments)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_tool() -> SubAgentSpawnTool {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        SubAgentSpawnTool::new(publisher, PathBuf::from("/tmp"))
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
    async fn definition_has_required_task() {
        let tool = make_tool();

        let def = tool.definition();
        assert_eq!(def.name, "subagent_spawn");
        let required = def.parameters.get("required").unwrap();
        assert!(
            required
                .as_array()
                .unwrap()
                .contains(&Value::String("task".to_string()))
        );
    }

    #[tokio::test]
    async fn main_preset_name_rejected() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "agent_name": "main"
            }))
            .await;

        assert!(result.is_err(), "\"main\" preset should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("reserved"),
            "error should mention 'reserved', got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn main_preset_name_rejected_case_insensitive() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "agent_name": "MAIN"
            }))
            .await;

        assert!(result.is_err(), "\"MAIN\" should also be rejected");
    }

    #[tokio::test]
    async fn unknown_preset_name_returns_error() {
        let tool = make_tool();

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "agent_name": "definitely-not-a-real-preset"
            }))
            .await
            .unwrap();

        assert!(result.is_error, "unknown preset should return a tool error");
        assert!(
            result.output.contains("unknown preset"),
            "error should mention unknown preset, got: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn default_preset_general_purpose_used_when_no_agent_name() {
        // When no agent_name is provided, the general-purpose preset is used.
        // We test this indirectly — the call should not fail with an unknown-preset error.
        let tool = make_tool();

        // No agent_name → should not fail with "unknown preset"
        let result = tool
            .execute(serde_json::json!({
                "task": "do something"
            }))
            .await;

        // May fail with provider error (no valid API key in test), but not with preset error
        match result {
            Ok(res) if res.is_error => {
                assert!(
                    !res.output.contains("unknown preset"),
                    "should not fail with unknown-preset error, got: {}",
                    res.output
                );
            }
            Ok(_) | Err(_) => {} // any other outcome is acceptable
        }
    }
}

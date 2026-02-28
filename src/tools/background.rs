//! Background task management tools: `stop_agent`, `list_agents`, and `subagent_spawn`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rand::Rng;
use serde_json::Value;

use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::types::{BackgroundTask, Execution, ResultRouting, SubAgentConfig};
use crate::config::BackgroundModelTier;
use crate::mcp::SharedMcpRegistry;
use crate::models::ToolDefinition;
use crate::notify::types::TaskSource;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
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
            let source_label = info.source.as_str();
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

// ─── SubAgentSpawnTool ──────────────────────────────────────────────────────

/// Tool for spawning background sub-agents on demand.
pub struct SubAgentSpawnTool {
    spawner: Arc<BackgroundTaskSpawner>,
    spawn_context: Arc<SpawnContext>,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    valid_external_channels: HashSet<String>,
    subagents_dir: PathBuf,
}

impl SubAgentSpawnTool {
    /// Create a new `SubAgentSpawnTool`.
    ///
    /// The `subagents_dir` for preset loading is derived from `spawn_context.layout`.
    #[must_use]
    pub(crate) fn new(
        spawner: Arc<BackgroundTaskSpawner>,
        spawn_context: Arc<SpawnContext>,
        project_state: SharedProjectState,
        skill_state: SharedSkillState,
        mcp_registry: SharedMcpRegistry,
        valid_external_channels: HashSet<String>,
    ) -> Self {
        let subagents_dir = spawn_context.layout.subagents_dir();
        Self {
            spawner,
            spawn_context,
            project_state,
            skill_state,
            mcp_registry,
            valid_external_channels,
            subagents_dir,
        }
    }
}

#[async_trait]
#[expect(
    clippy::too_many_lines,
    reason = "execute() handles preset validation, channel validation, and resource construction in a single flow"
)]
impl Tool for SubAgentSpawnTool {
    fn name(&self) -> &'static str {
        "subagent_spawn"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "subagent_spawn".to_string(),
            description: "Spawn a background sub-agent to handle a task. The agent_name selects a preset that configures the sub-agent's instructions, model tier, and tool restrictions. Unknown preset names fail immediately with a list of available presets. Runs asynchronously and delivers the result to the specified channels.".to_string(),
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
                    },
                    "channels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Result delivery channels. If omitted, uses the preset's default channels (fallback: [\"agent_feed\"])."
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
        let explicit_channels: Option<Vec<String>> = arguments
            .get("channels")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            });

        let resolved = match resolve_spawn_params(
            &self.subagents_dir,
            preset_name,
            explicit_model_override,
            explicit_channels,
        )
        .await
        {
            Ok(r) => r,
            Err(tool_result) => return Ok(tool_result),
        };

        for ch in &resolved.channels {
            if !is_valid_channel(ch, &self.valid_external_channels) {
                return Ok(ToolResult::error(format!(
                    "unknown channel '{ch}'. Valid: agent_wake, agent_feed, inbox, or configured external channels."
                )));
            }
        }

        let task_id = generate_agent_task_id();
        let config = SubAgentConfig {
            prompt: task_prompt.to_string(),
            context: None,
            model_tier: resolved.tier,
        };
        let preset_arg = Some((&resolved.preset_fm, resolved.preset_body.clone()));

        let resources = build_spawn_resources(
            &self.spawn_context,
            &resolved.tier,
            &self.project_state,
            &self.skill_state,
            Arc::clone(&self.mcp_registry),
            preset_arg,
        )
        .await
        .map_err(|err| {
            ToolError::Execution(format!("failed to build sub-agent resources: {err}"))
        })?;

        let task = BackgroundTask {
            id: task_id.clone(),
            task_name: resolved.preset_name,
            source: TaskSource::Agent,
            execution: Execution::SubAgent(config),
            routing: ResultRouting::Direct(resolved.channels),
        };

        self.spawner
            .spawn(task, Some(resources))
            .await
            .map_err(|err| ToolError::Execution(format!("failed to spawn sub-agent: {err}")))?;

        Ok(ToolResult::success(format!("Subagent spawned: {task_id}")))
    }
}

/// Resolved spawn parameters after loading a preset.
struct ResolvedSpawn {
    preset_name: String,
    tier: BackgroundModelTier,
    channels: Vec<String>,
    preset_fm: crate::subagents::types::SubagentPresetFrontmatter,
    preset_body: String,
}

/// Load a preset and resolve tier/channel defaults from arguments + preset.
async fn resolve_spawn_params(
    subagents_dir: &std::path::Path,
    preset_name: &str,
    explicit_model_override: Option<&str>,
    explicit_channels: Option<Vec<String>>,
) -> Result<ResolvedSpawn, ToolResult> {
    let index = SubagentPresetIndex::scan(subagents_dir)
        .await
        .map_err(|e| ToolResult::error(format!("failed to load subagent presets: {e}")))?;

    let (preset_fm, preset_body) = index
        .load_preset(preset_name)
        .await
        .map_err(|e| ToolResult::error(e.to_string()))?;

    let tier = if let Some(s) = explicit_model_override {
        parse_model_tier(s).map_err(|e| ToolResult::error(e.to_string()))?
    } else if let Some(tier_str) = preset_fm.model_tier.as_deref() {
        parse_model_tier(tier_str).unwrap_or(BackgroundModelTier::Medium)
    } else {
        BackgroundModelTier::Medium
    };

    let channels = explicit_channels
        .or_else(|| preset_fm.channels.clone())
        .unwrap_or_else(|| vec!["agent_feed".to_string()]);

    Ok(ResolvedSpawn {
        preset_name: preset_name.to_string(),
        tier,
        channels,
        preset_fm,
        preset_body,
    })
}

fn parse_model_tier(s: &str) -> Result<BackgroundModelTier, ToolError> {
    s.parse::<BackgroundModelTier>()
        .map_err(ToolError::InvalidArguments)
}

fn generate_agent_task_id() -> String {
    let rand_part: u32 = rand::thread_rng().r#gen();
    let timestamp_ms = Utc::now().timestamp_millis();
    format!("agent-{rand_part:08x}-{timestamp_ms}")
}

pub(crate) fn is_valid_channel(name: &str, external: &HashSet<String>) -> bool {
    matches!(name, "agent_wake" | "agent_feed" | "inbox") || external.contains(name)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::mcp::McpRegistry;
    use crate::models::CompletionOptions;
    use crate::projects::activation::ProjectState;
    use crate::projects::scanner::ProjectIndex;
    use crate::skills::{SkillIndex, SkillState};
    use crate::workspace::identity::IdentityFiles;
    use crate::workspace::layout::WorkspaceLayout;

    use tokio::sync::mpsc;

    fn make_test_spawn_context() -> Arc<SpawnContext> {
        Arc::new(SpawnContext {
            background_config: crate::config::BackgroundConfig::default(),
            main_provider_spec: crate::config::ProviderSpec {
                name: "test".to_string(),
                model: crate::config::ModelSpec {
                    kind: crate::config::ProviderKind::Ollama,
                    model: "test-model".to_string(),
                },
                provider_url: "http://localhost:11434".to_string(),
                api_key: None,
            },
            http_client: crate::models::SharedHttpClient::new(
                &crate::models::HttpClientConfig::with_timeout(30),
            )
            .unwrap(),
            max_tokens: 4096,
            retry_config: crate::models::retry::RetryConfig::no_retry(),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            layout: WorkspaceLayout::new(PathBuf::from("/tmp")),
            tz: chrono_tz::UTC,
        })
    }

    fn make_spawner() -> (
        Arc<BackgroundTaskSpawner>,
        mpsc::Receiver<crate::background::types::BackgroundResult>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(32);
        let spawner = Arc::new(BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
        ));
        (spawner, rx)
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

    #[test]
    fn valid_channels() {
        let external = HashSet::from(["ntfy_phone".to_string()]);
        assert!(is_valid_channel("agent_wake", &external));
        assert!(is_valid_channel("agent_feed", &external));
        assert!(is_valid_channel("inbox", &external));
        assert!(is_valid_channel("ntfy_phone", &external));
        assert!(!is_valid_channel("unknown", &external));
    }

    #[test]
    fn task_id_format() {
        let id = generate_agent_task_id();
        assert!(id.starts_with("agent-"), "should start with agent-");
        // Format: agent-XXXXXXXX-TIMESTAMP
        let parts: Vec<&str> = id.splitn(3, '-').collect();
        assert_eq!(parts.len(), 3, "should have 3 parts");
    }

    #[tokio::test]
    async fn invalid_channel_rejected() {
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "channels": ["nonexistent_channel"]
            }))
            .await
            .unwrap();

        assert!(result.is_error, "should reject unknown channel");
        assert!(result.output.contains("unknown channel"));
    }

    #[tokio::test]
    async fn task_required() {
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

        // Missing task
        let missing_result = tool.execute(serde_json::json!({})).await;
        assert!(missing_result.is_err(), "should error on missing task");

        // Empty task
        let empty_result = tool.execute(serde_json::json!({"task": "  "})).await;
        assert!(empty_result.is_err(), "should error on empty task");
    }

    #[test]
    fn definition_has_required_task() {
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

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
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

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
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

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
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

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
        let (spawner, _rx) = make_spawner();
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();

        let tool = SubAgentSpawnTool::new(
            spawner,
            make_test_spawn_context(),
            project_state,
            skill_state,
            mcp_registry,
            HashSet::new(),
        );

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

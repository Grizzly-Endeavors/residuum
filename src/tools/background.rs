//! Background task management tools: `stop_agent`, `list_agents`, and `subagent_spawn`.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use rand::Rng;
use serde_json::Value;

use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::subagent::execute_subagent;
use crate::background::types::{BackgroundTask, Execution, ResultRouting, SubAgentConfig};
use crate::config::BackgroundModelTier;
use crate::mcp::SharedMcpRegistry;
use crate::models::ToolDefinition;
use crate::notify::types::TaskSource;
use crate::projects::activation::SharedProjectState;
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
}

impl SubAgentSpawnTool {
    /// Create a new `SubAgentSpawnTool`.
    #[must_use]
    pub(crate) fn new(
        spawner: Arc<BackgroundTaskSpawner>,
        spawn_context: Arc<SpawnContext>,
        project_state: SharedProjectState,
        skill_state: SharedSkillState,
        mcp_registry: SharedMcpRegistry,
        valid_external_channels: HashSet<String>,
    ) -> Self {
        Self {
            spawner,
            spawn_context,
            project_state,
            skill_state,
            mcp_registry,
            valid_external_channels,
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
            description: "Spawn a background sub-agent to handle a task. By default runs asynchronously and delivers the result to the specified channels. Set wait=true to block until the sub-agent finishes and return its output directly.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The prompt/instructions for the sub-agent"
                    },
                    "agent_name": {
                        "type": "string",
                        "description": "Human-readable name for the task (default: \"subagent\")"
                    },
                    "model_override": {
                        "type": "string",
                        "enum": ["small", "medium", "large"],
                        "description": "Model tier to use (default: \"medium\")"
                    },
                    "channels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Result delivery channels (default: [\"agent_feed\"]). Only used in async mode."
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "If true, block until the sub-agent finishes and return its output (default: false)"
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        // Parse required task param
        let task_prompt = arguments
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("task is required".to_string()))?;

        if task_prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "task must not be empty".to_string(),
            ));
        }

        // Parse optional params
        let agent_name = arguments
            .get("agent_name")
            .and_then(Value::as_str)
            .unwrap_or("subagent");

        let tier = arguments
            .get("model_override")
            .and_then(Value::as_str)
            .map_or(Ok(BackgroundModelTier::Medium), parse_model_tier)?;

        let wait = arguments
            .get("wait")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let channels: Vec<String> = arguments
            .get("channels")
            .and_then(Value::as_array)
            .map_or_else(
                || vec!["agent_feed".to_string()],
                |arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                },
            );

        // Validate channels in async mode
        if !wait {
            for ch in &channels {
                if !is_valid_channel(ch, &self.valid_external_channels) {
                    return Ok(ToolResult::error(format!(
                        "unknown channel '{ch}'. Valid: agent_wake, agent_feed, inbox, or configured external channels."
                    )));
                }
            }
        }

        let task_id = generate_agent_task_id();
        let config = SubAgentConfig {
            prompt: task_prompt.to_string(),
            context: None,
            model_tier: tier,
        };

        if wait {
            // Sync mode: run directly, no spawner
            let resources = build_spawn_resources(
                &self.spawn_context,
                &tier,
                &self.project_state,
                &self.skill_state,
                Arc::clone(&self.mcp_registry),
            )
            .await
            .map_err(|err| {
                ToolError::Execution(format!("failed to build sub-agent resources: {err}"))
            })?;

            match execute_subagent(&task_id, &config, &resources).await {
                Ok(output) => Ok(ToolResult::success(output)),
                Err(err) => Ok(ToolResult::error(format!("sub-agent failed: {err}"))),
            }
        } else {
            // Async mode: spawn via BackgroundTaskSpawner
            let resources = build_spawn_resources(
                &self.spawn_context,
                &tier,
                &self.project_state,
                &self.skill_state,
                Arc::clone(&self.mcp_registry),
            )
            .await
            .map_err(|err| {
                ToolError::Execution(format!("failed to build sub-agent resources: {err}"))
            })?;

            let task = BackgroundTask {
                id: task_id.clone(),
                task_name: agent_name.to_string(),
                source: TaskSource::Agent,
                execution: Execution::SubAgent(config),
                routing: ResultRouting::Direct(channels),
            };

            self.spawner
                .spawn(task, Some(resources))
                .await
                .map_err(|err| ToolError::Execution(format!("failed to spawn sub-agent: {err}")))?;

            Ok(ToolResult::success(format!("Subagent spawned: {task_id}")))
        }
    }
}

fn parse_model_tier(s: &str) -> Result<BackgroundModelTier, ToolError> {
    match s {
        "small" => Ok(BackgroundModelTier::Small),
        "medium" => Ok(BackgroundModelTier::Medium),
        "large" => Ok(BackgroundModelTier::Large),
        other => Err(ToolError::InvalidArguments(format!(
            "invalid model_override '{other}': must be small, medium, or large"
        ))),
    }
}

fn generate_agent_task_id() -> String {
    let rand_part: u32 = rand::thread_rng().r#gen();
    let timestamp_ms = Utc::now().timestamp_millis();
    format!("agent-{rand_part:08x}-{timestamp_ms}")
}

fn is_valid_channel(name: &str, external: &HashSet<String>) -> bool {
    matches!(name, "agent_wake" | "agent_feed" | "inbox") || external.contains(name)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::background::subagent::SubAgentResources;
    use crate::mcp::McpRegistry;
    use crate::models::{CompletionOptions, Message, ModelError, ModelProvider, ModelResponse};
    use crate::projects::activation::ProjectState;
    use crate::projects::scanner::ProjectIndex;
    use crate::skills::{SkillIndex, SkillState};
    use crate::tools::path_policy::PathPolicy;
    use crate::tools::{ToolFilter, ToolRegistry};
    use crate::workspace::identity::IdentityFiles;
    use crate::workspace::layout::WorkspaceLayout;

    use async_trait::async_trait;
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

    struct MockSpawnProvider {
        response: String,
    }

    #[async_trait]
    impl ModelProvider for MockSpawnProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-spawn"
        }
    }

    fn make_test_resources(response: &str) -> SubAgentResources {
        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let path_policy = PathPolicy::new_shared(PathBuf::from("/tmp"));
        let tool_filter = ToolFilter::new_shared(HashSet::new());
        let mcp_registry = McpRegistry::new_shared();
        SubAgentResources {
            provider: Box::new(MockSpawnProvider {
                response: response.to_string(),
            }),
            tools: ToolRegistry::new(),
            tool_filter,
            mcp_registry,
            project_state,
            skill_state,
            path_policy,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            projects_ctx_index: None,
            skills_index: None,
            preset_instructions: None,
        }
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
    async fn spawn_sync_returns_output() {
        let resources = make_test_resources("analysis complete");
        let config = SubAgentConfig {
            prompt: "analyze logs".to_string(),
            context: None,
            model_tier: BackgroundModelTier::Medium,
        };

        let result = execute_subagent("test-sync-1", &config, &resources)
            .await
            .unwrap();
        assert_eq!(result, "analysis complete");
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
    async fn channels_ignored_in_sync_mode() {
        // In sync/wait mode, invalid channels should not cause errors
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

        // This would fail in async mode (invalid channel), but sync mode skips validation
        // NOTE: this test will fail with a provider error since we can't construct a real
        // provider from a default ProviderSpec, but the important thing is it does NOT fail
        // with a channel validation error
        let result = tool
            .execute(serde_json::json!({
                "task": "do something",
                "channels": ["nonexistent_channel"],
                "wait": true
            }))
            .await;

        // The error should NOT be about channel validation
        match result {
            Ok(res) => {
                // If it's an error it should be about provider, not channels
                if res.is_error {
                    assert!(
                        !res.output.contains("unknown channel"),
                        "sync mode should not validate channels"
                    );
                }
            }
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    !msg.contains("unknown channel"),
                    "sync mode should not validate channels"
                );
            }
        }
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
}

//! Background task spawner: bounded concurrency with cancellation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::subagent::{
    SubAgentOutput, SubAgentResources, execute_subagent, force_deactivate_project,
};
use super::types::{ActiveTaskInfo, BackgroundResult, BackgroundTask, execution_info};
use crate::bus::AgentResultStatus;
use crate::mcp::SharedMcpRegistry;
use crate::models::Message;
use crate::projects::activation::SharedProjectState;
use crate::tools::SharedToolFilter;
use crate::tools::path_policy::SharedPathPolicy;

struct CleanupHandles {
    project_state: SharedProjectState,
    mcp_registry: SharedMcpRegistry,
    path_policy: SharedPathPolicy,
    tool_filter: SharedToolFilter,
}

/// Spawns and manages background tasks with bounded concurrency.
pub struct BackgroundTaskSpawner {
    result_tx: mpsc::Sender<BackgroundResult>,
    semaphore: Arc<Semaphore>,
    active_tasks: Arc<Mutex<HashMap<String, (CancellationToken, ActiveTaskInfo)>>>,
    background_dir: PathBuf,
}

impl BackgroundTaskSpawner {
    /// Create a new spawner with the given concurrency limit and result channel.
    #[must_use]
    pub fn new(
        result_tx: mpsc::Sender<BackgroundResult>,
        max_concurrent: usize,
        background_dir: PathBuf,
    ) -> Self {
        Self {
            result_tx,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            background_dir,
        }
    }

    /// Spawn a background task. Returns the task ID immediately.
    ///
    /// The task runs on the tokio runtime, bounded by the concurrency semaphore.
    /// When complete, a `BackgroundResult` is sent to the result channel.
    ///
    /// # Errors
    /// Returns an error if the task cannot be registered (e.g. duplicate ID).
    #[tracing::instrument(skip_all, fields(task.id = %task.id))]
    pub(crate) async fn spawn(
        &self,
        task: BackgroundTask,
        resources: Option<SubAgentResources>,
    ) -> Result<String, anyhow::Error> {
        let task_id = task.id.clone();
        let token = CancellationToken::new();

        let semaphore = Arc::clone(&self.semaphore);
        let active_tasks = Arc::clone(&self.active_tasks);
        let result_tx = self.result_tx.clone();
        let background_dir = self.background_dir.clone();
        let child_token = token.clone();
        let spawn_task_id = task_id.clone();

        let prompt_preview = execution_info(&task.subagent_config);
        let active_info = ActiveTaskInfo {
            source_label: task.source_label.clone(),
            source: task.source.clone(),
            prompt_preview,
            started_at: Utc::now(),
        };

        // Register the task before spawning
        active_tasks
            .lock()
            .await
            .insert(task_id.clone(), (token, active_info));

        tracing::info!(task_id = %task_id, source_label = %task.source_label, "spawning background task");

        tokio::spawn(async move {
            // Acquire semaphore permit (waits if at capacity)
            let _permit = match semaphore.acquire().await {
                Ok(permit) => permit,
                Err(_closed) => {
                    tracing::warn!(task_id = %spawn_task_id, "semaphore closed before task could acquire permit; task dropped");
                    active_tasks.lock().await.remove(&spawn_task_id);
                    return;
                }
            };

            // Extract Arc clones for cancellation cleanup (cheap; no data copied)
            let cleanup_handles = resources.as_ref().map(|r| CleanupHandles {
                project_state: Arc::clone(&r.project_state),
                mcp_registry: Arc::clone(&r.mcp_registry),
                path_policy: Arc::clone(&r.path_policy),
                tool_filter: Arc::clone(&r.tool_filter),
            });

            let result = tokio::select! {
                biased;
                () = child_token.cancelled() => {
                    build_cancelled_result(&task, &spawn_task_id, cleanup_handles).await
                }
                outcome = async {
                    let res = resources.as_ref().ok_or_else(|| anyhow::anyhow!("sub-agent task requires SubAgentResources"))?;
                    execute_subagent(&task.id, &task.subagent_config, res).await
                } => {
                    build_completed_result(&task, outcome, &background_dir).await
                }
            };

            // Remove from active tasks
            active_tasks.lock().await.remove(&result.id);

            // Send result to gateway channel
            if let Err(e) = result_tx.send(result).await {
                tracing::warn!(
                    task_id = %e.0.id,
                    source_label = %e.0.source_label,
                    "failed to send background task result; result lost"
                );
            }
        });

        Ok(task_id)
    }

    /// Send a pre-built result directly through the result channel.
    ///
    /// Used for pre-built results that need no LLM call — they produce
    /// a `BackgroundResult` immediately and inject it into the normal result
    /// pipeline.
    ///
    /// # Errors
    /// Returns an error if the result channel is closed.
    pub async fn send_result(&self, result: BackgroundResult) -> Result<(), anyhow::Error> {
        self.result_tx
            .send(result)
            .await
            .map_err(|send_err| anyhow::anyhow!("failed to send direct result: {send_err}"))
    }

    /// Cancel a running task. Returns `true` if the task was found and cancelled.
    #[tracing::instrument(skip_all, fields(task.id = %task_id))]
    pub async fn cancel(&self, task_id: &str) -> bool {
        let guard = self.active_tasks.lock().await;
        if let Some((token, _info)) = guard.get(task_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Snapshot of active task metadata for display.
    pub async fn list_active_tasks(&self) -> Vec<(String, ActiveTaskInfo)> {
        self.active_tasks
            .lock()
            .await
            .iter()
            .map(|(id, (_token, info))| (id.clone(), info.clone()))
            .collect()
    }
}

/// Build a `BackgroundResult` for a cancelled task, cleaning up any active project.
async fn build_cancelled_result(
    task: &BackgroundTask,
    spawn_task_id: &str,
    cleanup_handles: Option<CleanupHandles>,
) -> BackgroundResult {
    tracing::info!(task_id = %spawn_task_id, "background task cancelled");
    if let Some(handles) = cleanup_handles {
        let active_name = handles
            .project_state
            .lock()
            .await
            .active_project_name()
            .map(str::to_string);
        if let Some(name) = active_name {
            force_deactivate_project(
                &name,
                &handles.mcp_registry,
                &handles.path_policy,
                &handles.tool_filter,
            )
            .await;
        }
    }

    BackgroundResult {
        id: task.id.clone(),
        source_label: task.source_label.clone(),
        source: task.source.clone(),
        summary: String::new(),
        transcript_path: None,
        status: AgentResultStatus::Cancelled,
        timestamp: Utc::now(),
        agent_preset: task.agent_preset.clone(),
    }
}

/// Build a `BackgroundResult` from a completed (or failed) task execution.
async fn build_completed_result(
    task: &BackgroundTask,
    outcome: Result<SubAgentOutput, anyhow::Error>,
    background_dir: &std::path::Path,
) -> BackgroundResult {
    let (status, summary, messages) = match outcome {
        Ok(SubAgentOutput { summary, messages }) => {
            (AgentResultStatus::Completed, summary, Some(messages))
        }
        Err(e) => {
            let error_msg = e.to_string();
            tracing::warn!(task_id = %task.id, source_label = %task.source_label, error = %e, "background task failed");
            (
                AgentResultStatus::Failed { error: error_msg },
                String::new(),
                None,
            )
        }
    };

    if matches!(status, AgentResultStatus::Completed) {
        tracing::info!(task_id = %task.id, source_label = %task.source_label, "background task completed");
    }

    let transcript_summary = match &status {
        AgentResultStatus::Failed { error } => error.as_str(),
        AgentResultStatus::Completed => &summary,
        AgentResultStatus::Cancelled => {
            unreachable!("Cancelled is only produced by build_cancelled_result")
        }
    };
    let transcript_path = write_transcript(
        background_dir,
        &task.id,
        transcript_summary,
        messages.as_deref(),
    )
    .await;

    BackgroundResult {
        id: task.id.clone(),
        source_label: task.source_label.clone(),
        source: task.source.clone(),
        summary,
        transcript_path,
        status,
        timestamp: Utc::now(),
        agent_preset: task.agent_preset.clone(),
    }
}

/// Write a transcript file for the task. Returns the path if successful.
///
/// When `messages` is provided, the transcript is serialized as JSON with both
/// a summary and the full message history. Falls back to plain summary on
/// serialization error.
async fn write_transcript(
    background_dir: &std::path::Path,
    task_id: &str,
    summary: &str,
    messages: Option<&[Message]>,
) -> Option<PathBuf> {
    let now = Utc::now();
    let month_dir = background_dir.join(now.format("%Y-%m").to_string());
    let day_dir = month_dir.join(now.format("%d").to_string());

    if let Err(e) = tokio::fs::create_dir_all(&day_dir).await {
        tracing::warn!(error = %e, "failed to create background transcript directory");
        return None;
    }

    let filename = format!("bg-{task_id}.log");
    let path = day_dir.join(&filename);

    let content = match messages {
        Some(msgs) => {
            let transcript = serde_json::json!({
                "summary": summary,
                "messages": msgs,
            });
            serde_json::to_string_pretty(&transcript).unwrap_or_else(|err| {
                tracing::warn!(error = %err, "failed to serialize transcript, falling back to plain summary");
                summary.to_string()
            })
        }
        None => summary.to_string(),
    };

    match tokio::fs::write(&path, content).await {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to write transcript");
            None
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::background::types::SubAgentConfig;
    use crate::bus::{EventTrigger, PresetName};
    use crate::config::BackgroundModelTier;

    #[tokio::test]
    async fn send_result_delivers_to_channel() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(tx, 3, dir.path().to_path_buf());

        let result = BackgroundResult {
            id: "direct-1".to_string(),
            source_label: "action:action_event".to_string(),
            source: EventTrigger::Action,
            summary: "system alert".to_string(),
            transcript_path: None,
            status: super::AgentResultStatus::Completed,
            timestamp: chrono::Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        spawner.send_result(result).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "direct-1");
        assert_eq!(received.source_label, "action:action_event");
        assert_eq!(received.summary, "system alert");
        assert!(matches!(received.source, EventTrigger::Action));
    }

    #[tokio::test]
    async fn failed_task_transcript_contains_error() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(tx, 3, dir.path().to_path_buf());

        // SubAgent without resources → guaranteed failure
        let task = BackgroundTask {
            id: "fail-transcript-1".to_string(),
            source_label: "agent:failing_agent".to_string(),
            source: EventTrigger::Agent,
            subagent_config: SubAgentConfig {
                prompt: "do something".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            },

            agent_preset: PresetName::from("general-purpose"),
        };

        spawner.spawn(task, None).await.unwrap();

        let result = rx.recv().await.unwrap();
        assert!(
            matches!(result.status, AgentResultStatus::Failed { .. }),
            "should be failed"
        );
        assert!(
            result.summary.is_empty(),
            "summary should be empty on failure (error is in status)"
        );
        assert!(
            result.transcript_path.is_some(),
            "failed task should still write transcript"
        );

        // Failed tasks write the error from status (no messages), so content is the raw error
        let transcript_content =
            tokio::fs::read_to_string(result.transcript_path.as_ref().unwrap())
                .await
                .unwrap();
        if let AgentResultStatus::Failed { error } = &result.status {
            assert!(transcript_content.contains(error.as_str()));
        }
    }

    #[tokio::test]
    async fn cancel_returns_true_for_known_false_for_unknown() {
        let (tx, _rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        // Zero concurrency: tasks block waiting for the semaphore, so they
        // remain registered in active_tasks long enough to cancel.
        let spawner = BackgroundTaskSpawner::new(tx, 0, dir.path().to_path_buf());

        let task = BackgroundTask {
            id: "cancel-check".to_string(),
            source_label: "agent:cancel_test".to_string(),
            source: EventTrigger::Agent,
            subagent_config: SubAgentConfig {
                prompt: "do work".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            },
            agent_preset: PresetName::from("general-purpose"),
        };

        let task_id = spawner.spawn(task, None).await.unwrap();

        assert!(
            spawner.cancel(&task_id).await,
            "should return true for known task"
        );
        assert!(
            !spawner.cancel("nonexistent-id").await,
            "should return false for unknown task"
        );
    }

    #[tokio::test]
    async fn list_active_tasks_returns_registered_task_info() {
        let (tx, _rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        // Zero concurrency keeps the task registered.
        let spawner = BackgroundTaskSpawner::new(tx, 0, dir.path().to_path_buf());

        let task = BackgroundTask {
            id: "list-test".to_string(),
            source_label: "agent:list_test".to_string(),
            source: EventTrigger::Agent,
            subagent_config: SubAgentConfig {
                prompt: "do work".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            },
            agent_preset: PresetName::from("general-purpose"),
        };

        spawner.spawn(task, None).await.unwrap();

        let active = spawner.list_active_tasks().await;
        assert_eq!(active.len(), 1);
        let (id, info) = active.first().unwrap();
        assert_eq!(id, "list-test");
        assert_eq!(info.source_label, "agent:list_test");
    }

    #[tokio::test]
    async fn cancelled_task_produces_cancelled_status() {
        use crate::mcp::McpRegistry;
        use crate::models::{
            CompletionOptions, Message, ModelError, ModelResponse, ToolDefinition,
        };
        use crate::projects::activation::ProjectState;
        use crate::projects::scanner::ProjectIndex;
        use crate::skills::{SkillIndex, SkillState};
        use crate::tools::path_policy::PathPolicy;
        use crate::tools::{ToolFilter, ToolRegistry};
        use crate::workspace::identity::IdentityFiles;
        use crate::workspace::layout::WorkspaceLayout;
        use async_trait::async_trait;
        use std::collections::HashSet;
        use std::path::PathBuf;

        struct BlockingProvider;

        #[async_trait]
        impl crate::models::ModelProvider for BlockingProvider {
            async fn complete(
                &self,
                _messages: &[Message],
                _tools: &[ToolDefinition],
                _options: &CompletionOptions,
            ) -> Result<ModelResponse, ModelError> {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Err(ModelError::Api("cancelled".into()))
            }

            fn model_name(&self) -> &'static str {
                "blocking"
            }
        }

        let project_state = ProjectState::new_shared(
            ProjectIndex::default(),
            WorkspaceLayout::new(PathBuf::from("/tmp")),
        );
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let path_policy = PathPolicy::new_shared(PathBuf::from("/tmp"));
        let tool_filter = ToolFilter::new_shared(HashSet::new());
        let mcp_registry = McpRegistry::new_shared();
        let resources = SubAgentResources {
            provider: Box::new(BlockingProvider),
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
        };

        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(tx, 1, dir.path().to_path_buf());

        let task = BackgroundTask {
            id: "cancel-status-test".to_string(),
            source_label: "agent:cancel_status".to_string(),
            source: EventTrigger::Agent,
            subagent_config: SubAgentConfig {
                prompt: "block forever".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            },
            agent_preset: PresetName::from("general-purpose"),
        };

        let task_id = spawner.spawn(task, Some(resources)).await.unwrap();

        // Yield to let the task acquire the semaphore and enter the select!
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        spawner.cancel(&task_id).await;

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(
            matches!(result.status, AgentResultStatus::Cancelled),
            "cancelled task should produce Cancelled status, got {:?}",
            result.status
        );
    }
}

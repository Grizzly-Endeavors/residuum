//! Background task spawner: bounded concurrency with cancellation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::subagent::{SubAgentOutput, SubAgentResources, execute_subagent};
use super::types::{
    ActiveTaskInfo, BackgroundResult, BackgroundTask, Execution, TaskStatus, execution_info,
};
use crate::models::Message;

/// Spawns and manages background tasks with bounded concurrency.
pub struct BackgroundTaskSpawner {
    result_tx: mpsc::Sender<BackgroundResult>,
    semaphore: Arc<Semaphore>,
    active_tasks: Arc<Mutex<HashMap<String, (CancellationToken, ActiveTaskInfo)>>>,
    workspace_root: PathBuf,
    background_dir: PathBuf,
}

impl BackgroundTaskSpawner {
    /// Create a new spawner with the given concurrency limit and result channel.
    #[must_use]
    pub fn new(
        result_tx: mpsc::Sender<BackgroundResult>,
        max_concurrent: usize,
        workspace_root: PathBuf,
        background_dir: PathBuf,
    ) -> Self {
        Self {
            result_tx,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            workspace_root,
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
    pub async fn spawn(
        &self,
        task: BackgroundTask,
        resources: Option<SubAgentResources>,
    ) -> Result<String, anyhow::Error> {
        let task_id = task.id.clone();
        let token = CancellationToken::new();

        let semaphore = Arc::clone(&self.semaphore);
        let active_tasks = Arc::clone(&self.active_tasks);
        let result_tx = self.result_tx.clone();
        let workspace_root = self.workspace_root.clone();
        let background_dir = self.background_dir.clone();
        let child_token = token.clone();
        let spawn_task_id = task_id.clone();

        let (exec_type, prompt_preview) = execution_info(&task.execution);
        let active_info = ActiveTaskInfo {
            task_name: task.task_name.clone(),
            source: task.source,
            execution_type: exec_type,
            prompt_preview,
            started_at: Utc::now(),
        };

        // Register the task before spawning
        active_tasks
            .lock()
            .await
            .insert(task_id.clone(), (token, active_info));

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
            let cleanup_handles = resources.as_ref().map(|r| {
                (
                    Arc::clone(&r.project_state),
                    Arc::clone(&r.mcp_registry),
                    Arc::clone(&r.path_policy),
                    Arc::clone(&r.tool_filter),
                )
            });

            let result = tokio::select! {
                biased;
                () = child_token.cancelled() => {
                    build_cancelled_result(&task, &spawn_task_id, cleanup_handles).await
                }
                outcome = execute_task(&task, resources.as_ref(), &workspace_root) => {
                    build_completed_result(&task, outcome, &background_dir).await
                }
            };

            // Remove from active tasks
            active_tasks.lock().await.remove(&result.id);

            // Send result to gateway channel
            if let Err(e) = result_tx.send(result).await {
                tracing::warn!(error = %e, "failed to send background task result");
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

    /// List IDs of currently active tasks.
    pub async fn active_task_ids(&self) -> Vec<String> {
        let guard = self.active_tasks.lock().await;
        guard.keys().cloned().collect()
    }
}

/// Build a `BackgroundResult` for a cancelled task, cleaning up any active project.
async fn build_cancelled_result(
    task: &BackgroundTask,
    spawn_task_id: &str,
    cleanup_handles: Option<(
        crate::projects::activation::SharedProjectState,
        crate::mcp::registry::SharedMcpRegistry,
        crate::tools::path_policy::SharedPathPolicy,
        crate::tools::SharedToolFilter,
    )>,
) -> BackgroundResult {
    if let Some((project_state, mcp_registry, path_policy, tool_filter)) = cleanup_handles {
        let active_name = project_state
            .lock()
            .await
            .active_project_name()
            .map(str::to_string);
        if let Some(name) = active_name {
            tracing::info!(
                task_id = %spawn_task_id,
                project = %name,
                "[cancelled] SubAgent {spawn_task_id} was stopped. Work may be incomplete."
            );
            mcp_registry.write().await.deactivate_project(&name).await;
            path_policy.write().await.set_active_project(None);
            tool_filter.write().await.clear_enabled();
        }
    }

    BackgroundResult {
        id: task.id.clone(),
        task_name: task.task_name.clone(),
        source: task.source,
        summary: String::new(),
        transcript_path: None,
        status: TaskStatus::Cancelled,
        timestamp: Utc::now(),
        routing: task.routing.clone(),
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
            (TaskStatus::Completed, summary, Some(messages))
        }
        Err(e) => {
            let error_msg = e.to_string();
            tracing::warn!(task_id = %task.id, task_name = %task.task_name, error = %e, "background task failed");
            (
                TaskStatus::Failed {
                    error: error_msg.clone(),
                },
                format!("[FAILED] {error_msg}"),
                None,
            )
        }
    };

    let transcript_path =
        write_transcript(background_dir, &task.id, &summary, messages.as_deref()).await;

    BackgroundResult {
        id: task.id.clone(),
        task_name: task.task_name.clone(),
        source: task.source,
        summary,
        transcript_path,
        status,
        timestamp: Utc::now(),
        routing: task.routing.clone(),
    }
}

/// Execute a task based on its execution type.
async fn execute_task(
    task: &BackgroundTask,
    resources: Option<&SubAgentResources>,
    _workspace_root: &std::path::Path,
) -> Result<SubAgentOutput, anyhow::Error> {
    let Execution::SubAgent(config) = &task.execution;
    let res =
        resources.ok_or_else(|| anyhow::anyhow!("sub-agent task requires SubAgentResources"))?;
    execute_subagent(&task.id, config, res).await
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
    use crate::background::types::{Execution, ResultRouting, SubAgentConfig};
    use crate::config::BackgroundModelTier;
    use crate::notify::types::TaskSource;

    #[tokio::test]
    async fn send_result_delivers_to_channel() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        let result = BackgroundResult {
            id: "direct-1".to_string(),
            task_name: "action_event".to_string(),
            source: TaskSource::Action,
            summary: "system alert".to_string(),
            transcript_path: None,
            status: super::TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Direct(vec!["agent_feed".to_string()]),
        };

        spawner.send_result(result).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "direct-1");
        assert_eq!(received.task_name, "action_event");
        assert_eq!(received.summary, "system alert");
        assert!(matches!(received.source, TaskSource::Action));
    }

    #[tokio::test]
    async fn failed_task_transcript_contains_error() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        // SubAgent without resources → guaranteed failure
        let task = BackgroundTask {
            id: "fail-transcript-1".to_string(),
            task_name: "failing_agent".to_string(),
            source: TaskSource::Agent,
            execution: Execution::SubAgent(SubAgentConfig {
                prompt: "do something".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            }),
            routing: ResultRouting::Direct(vec!["agent_feed".to_string()]),
        };

        spawner.spawn(task, None).await.unwrap();

        let result = rx.recv().await.unwrap();
        assert!(
            matches!(result.status, TaskStatus::Failed { .. }),
            "should be failed"
        );
        assert!(
            result.summary.contains("[FAILED]"),
            "summary should contain [FAILED] prefix, got: {}",
            result.summary
        );
        assert!(
            !result.summary.is_empty(),
            "summary should not be empty on failure"
        );
        assert!(
            result.transcript_path.is_some(),
            "failed task should still write transcript"
        );

        // Failed tasks write plain summary (no messages), so content is the raw error string
        let transcript_content =
            tokio::fs::read_to_string(result.transcript_path.as_ref().unwrap())
                .await
                .unwrap();
        assert!(
            transcript_content.contains("[FAILED]"),
            "transcript should contain the failure summary"
        );
    }
}

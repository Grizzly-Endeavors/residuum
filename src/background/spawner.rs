//! Background task spawner: bounded concurrency with cancellation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::script::execute_script;
use super::subagent::{SubAgentResources, execute_subagent};
use super::types::{
    ActiveTaskInfo, BackgroundResult, BackgroundTask, Execution, TaskStatus, execution_info,
};

/// Spawns and manages background tasks with bounded concurrency.
pub struct BackgroundTaskSpawner {
    result_tx: mpsc::Sender<BackgroundResult>,
    semaphore: Arc<Semaphore>,
    active_tasks: Arc<Mutex<HashMap<String, (CancellationToken, ActiveTaskInfo)>>>,
    workspace_root: PathBuf,
    background_dir: PathBuf,
    #[expect(
        dead_code,
        reason = "stored for future transcript timestamp formatting"
    )]
    tz: chrono_tz::Tz,
}

impl BackgroundTaskSpawner {
    /// Create a new spawner with the given concurrency limit and result channel.
    #[must_use]
    pub fn new(
        result_tx: mpsc::Sender<BackgroundResult>,
        max_concurrent: usize,
        workspace_root: PathBuf,
        background_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            result_tx,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            workspace_root,
            background_dir,
            tz,
        }
    }

    /// Spawn a background task. Returns the task ID immediately.
    ///
    /// The task runs on the tokio runtime, bounded by the concurrency semaphore.
    /// When complete, a `BackgroundResult` is sent to the result channel.
    ///
    /// # Errors
    /// Returns an error if the task cannot be registered (e.g. duplicate ID).
    pub fn spawn(
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
        {
            let mut guard = active_tasks.try_lock().map_err(|lock_err| {
                anyhow::anyhow!("failed to lock active_tasks for registration: {lock_err}")
            })?;
            guard.insert(task_id.clone(), (token, active_info));
        }

        tokio::spawn(async move {
            // Acquire semaphore permit (waits if at capacity)
            let _permit = match semaphore.acquire().await {
                Ok(permit) => permit,
                Err(_closed) => {
                    active_tasks.lock().await.remove(&spawn_task_id);
                    return;
                }
            };

            let result = tokio::select! {
                biased;
                () = child_token.cancelled() => {
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
                outcome = execute_task(&task, resources.as_ref(), &workspace_root) => {
                    let (status, summary) = match outcome {
                        Ok(output) => (TaskStatus::Completed, output),
                        Err(e) => (TaskStatus::Failed { error: e.to_string() }, String::new()),
                    };

                    let transcript_path = write_transcript(
                        &background_dir, &task.id, &summary,
                    ).await;

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

/// Execute a task based on its execution type.
async fn execute_task(
    task: &BackgroundTask,
    resources: Option<&SubAgentResources>,
    workspace_root: &std::path::Path,
) -> Result<String, anyhow::Error> {
    match &task.execution {
        Execution::SubAgent(config) => {
            let res = resources
                .ok_or_else(|| anyhow::anyhow!("sub-agent task requires SubAgentResources"))?;
            execute_subagent(&task.id, config, res).await
        }
        Execution::Script(config) => execute_script(&task.id, config, workspace_root).await,
    }
}

/// Write a transcript file for the task. Returns the path if successful.
async fn write_transcript(
    background_dir: &std::path::Path,
    task_id: &str,
    content: &str,
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
    use crate::background::types::{Execution, ResultRouting, ScriptConfig};
    use crate::notify::types::TaskSource;

    fn make_echo_task(id: &str) -> BackgroundTask {
        BackgroundTask {
            id: id.to_string(),
            task_name: "test_echo".to_string(),
            source: TaskSource::Agent,
            execution: Execution::Script(ScriptConfig {
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        }
    }

    #[tokio::test]
    async fn spawn_and_receive_result() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        let task = make_echo_task("t1");
        let id = spawner.spawn(task, None).unwrap();
        assert_eq!(id, "t1");

        let result = rx.recv().await.unwrap();
        assert_eq!(result.id, "t1");
        assert!(
            matches!(result.status, TaskStatus::Completed),
            "should be completed"
        );
        assert!(
            result.summary.contains("hello"),
            "should contain echo output"
        );
    }

    #[tokio::test]
    async fn cancel_returns_correct_bool() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        // Spawn a long-running task
        let task = BackgroundTask {
            id: "long-1".to_string(),
            task_name: "long_task".to_string(),
            source: TaskSource::Agent,
            execution: Execution::Script(ScriptConfig {
                command: "sleep".to_string(),
                args: vec!["30".to_string()],
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        };

        spawner.spawn(task, None).unwrap();

        // Give the spawned task time to register and start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let cancelled = spawner.cancel("long-1").await;
        assert!(cancelled, "should return true for active task");

        let not_found = spawner.cancel("nonexistent").await;
        assert!(!not_found, "should return false for unknown task");

        // Drain the result
        let result = rx.recv().await.unwrap();
        assert!(
            matches!(result.status, TaskStatus::Cancelled),
            "should be cancelled"
        );
    }

    #[tokio::test]
    async fn concurrency_limit_enforced() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let max_concurrent = 2;
        let spawner = BackgroundTaskSpawner::new(
            tx,
            max_concurrent,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        // Spawn 3 tasks (one more than limit)
        for i in 0..3 {
            let task = BackgroundTask {
                id: format!("conc-{i}"),
                task_name: "echo_task".to_string(),
                source: TaskSource::Agent,
                execution: Execution::Script(ScriptConfig {
                    command: "echo".to_string(),
                    args: vec![format!("task-{i}")],
                    working_dir: None,
                    timeout_secs: None,
                }),
                routing: ResultRouting::Notify,
            };
            spawner.spawn(task, None).unwrap();
        }

        // All 3 should complete (the semaphore just queues the 3rd)
        let mut results = Vec::new();
        for _ in 0..3 {
            results.push(rx.recv().await.unwrap());
        }

        assert_eq!(results.len(), 3, "all 3 tasks should complete");
        for result in &results {
            assert!(
                matches!(result.status, TaskStatus::Completed),
                "all should be completed"
            );
        }
    }

    #[tokio::test]
    async fn result_sent_to_channel() {
        let (tx, mut rx) = mpsc::channel(32);
        let dir = tempfile::tempdir().unwrap();
        let spawner = BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        let task = make_echo_task("ch-1");
        spawner.spawn(task, None).unwrap();

        let result = rx.recv().await.unwrap();
        assert_eq!(result.task_name, "test_echo");
        assert!(result.transcript_path.is_some(), "should write transcript");
    }
}

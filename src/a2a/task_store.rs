//! Persistent, file-backed [`a2a_server::TaskStore`]: each task is a JSON
//! file at `{workspace}/a2a/tasks/{task_id}.json`, written atomically, with
//! an in-memory index for reads. See `docs/systems-usage/a2a.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2a::{A2AError, ListTasksRequest, ListTasksResponse, Task};
use a2a_server::task_store::TaskVersion;
use anyhow::Context as _;
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::util::fs::atomic_write;

/// Task metadata key recording the caller (`key:<name>` | `sibling:<slug>`)
/// a task belongs to, checked by [`super::handler::ResiduumA2aHandler`] on
/// every task-addressed request.
pub const CALLER_METADATA_KEY: &str = "residuum.caller";
/// Task metadata key recording the conversation-session address the
/// executor delivers this task's messages into.
pub const ADDRESS_METADATA_KEY: &str = "residuum.address";
/// Message metadata key marking a restart-continuation message as synthetic
/// (Residuum's own, not something the caller sent).
pub const SYNTHETIC_METADATA_KEY: &str = "residuum.synthetic";

/// How long a terminal task is kept before being pruned at startup.
const TASK_RETENTION: chrono::Duration = chrono::Duration::days(30);

struct StoredEntry {
    task: Task,
    version: TaskVersion,
}

/// File-backed [`a2a_server::TaskStore`].
pub struct FileTaskStore {
    dir: PathBuf,
    tasks: RwLock<HashMap<String, StoredEntry>>,
}

/// Shared handle to the task store.
pub type SharedTaskStore = Arc<FileTaskStore>;

impl FileTaskStore {
    /// Load every task file under `dir` (creating it if missing) into an
    /// in-memory index, pruning terminal tasks whose status timestamp is
    /// older than 30 days.
    ///
    /// # Errors
    /// Returns an error if `dir` cannot be created or listed.
    pub async fn load(dir: &Path) -> anyhow::Result<SharedTaskStore> {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("failed to create a2a task directory at {}", dir.display()))?;

        let mut tasks = HashMap::new();
        let mut read_dir = tokio::fs::read_dir(dir)
            .await
            .with_context(|| format!("failed to read a2a task directory at {}", dir.display()))?;
        let cutoff = chrono::Utc::now() - TASK_RETENTION;
        let mut pruned = 0_usize;
        loop {
            let entry = read_dir
                .next_entry()
                .await
                .with_context(|| format!("failed to read entry in {}", dir.display()))?;
            let Some(entry) = entry else { break };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = match tokio::fs::read(&path).await {
                Ok(raw) => raw,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to read a2a task file; skipping");
                    continue;
                }
            };
            let task: Task = match serde_json::from_slice(&raw) {
                Ok(task) => task,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to parse a2a task file; skipping");
                    continue;
                }
            };
            let is_old_terminal = task.status.state.is_terminal()
                && task.status.timestamp.is_some_and(|ts| ts < cutoff);
            if is_old_terminal {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!(path = %path.display(), error = %e, "failed to prune old a2a task file");
                } else {
                    pruned += 1;
                }
                continue;
            }
            tasks.insert(task.id.clone(), StoredEntry { task, version: 1 });
        }
        if pruned > 0 {
            tracing::info!(
                count = pruned,
                "pruned terminal a2a tasks older than 30 days"
            );
        }
        tracing::info!(count = tasks.len(), "loaded a2a tasks from disk");

        Ok(Arc::new(Self {
            dir: dir.to_path_buf(),
            tasks: RwLock::new(tasks),
        }))
    }

    fn path_for(&self, task_id: &str) -> PathBuf {
        self.dir.join(format!("{task_id}.json"))
    }

    async fn persist(&self, task: &Task) -> Result<(), A2AError> {
        let json = serde_json::to_vec_pretty(task)
            .map_err(|e| A2AError::internal(format!("failed to serialize a2a task: {e}")))?;
        atomic_write(&self.path_for(&task.id), json)
            .await
            .map_err(|e| A2AError::internal(format!("failed to persist a2a task {}: {e}", task.id)))
    }

    /// The caller a task belongs to, from its `residuum.caller` metadata.
    #[must_use]
    pub fn caller_of(task: &Task) -> Option<&str> {
        task.metadata.as_ref()?.get(CALLER_METADATA_KEY)?.as_str()
    }

    /// The non-terminal task in `context_id`, if one exists — used to
    /// enforce one open task per context.
    pub async fn open_task_in_context(&self, context_id: &str) -> Option<Task> {
        self.tasks
            .read()
            .await
            .values()
            .map(|e| &e.task)
            .find(|task| task.context_id == context_id && !task.status.state.is_terminal())
            .cloned()
    }

    /// Every task belonging to `caller` (its `residuum.caller` metadata),
    /// for [`super::handler::ResiduumA2aHandler::list_tasks`] scoping.
    pub async fn tasks_for_caller(&self, caller: &str) -> Vec<Task> {
        self.tasks
            .read()
            .await
            .values()
            .map(|e| &e.task)
            .filter(|task| Self::caller_of(task) == Some(caller))
            .cloned()
            .collect()
    }

    /// Every task left `Submitted` or `Working`, for the restart
    /// continuation sweep.
    pub async fn in_progress_tasks(&self) -> Vec<Task> {
        self.tasks
            .read()
            .await
            .values()
            .map(|e| &e.task)
            .filter(|task| {
                matches!(
                    task.status.state,
                    a2a::TaskState::Submitted | a2a::TaskState::Working
                )
            })
            .cloned()
            .collect()
    }
}

/// Saturating `usize -> i32`, for the `ListTasksResponse` count fields a
/// page or store size never realistically approaches `i32::MAX` anyway.
#[must_use]
pub(crate) fn saturating_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Mirrors `a2a_server::task_store::apply_history_length`, which is
/// crate-private in the SDK — duplicated here rather than widened upstream.
/// Shared with [`super::handler::ResiduumA2aHandler::list_tasks`], which
/// bypasses the SDK's own list path to filter by caller first.
pub(crate) fn apply_history_length(task: &mut Task, history_length: Option<i32>) {
    let Some(requested) = history_length else {
        return;
    };
    let keep = usize::try_from(requested).unwrap_or(0);
    let Some(history) = task.history.as_mut() else {
        return;
    };
    if keep == 0 {
        history.clear();
        return;
    }
    if history.len() > keep {
        history.drain(..history.len() - keep);
    }
}

#[async_trait]
impl a2a_server::TaskStore for FileTaskStore {
    async fn create(&self, task: Task) -> Result<TaskVersion, A2AError> {
        let mut tasks = self.tasks.write().await;
        if tasks.contains_key(&task.id) {
            return Err(A2AError::internal("task already exists"));
        }
        self.persist(&task).await?;
        let id = task.id.clone();
        tasks.insert(id, StoredEntry { task, version: 1 });
        Ok(1)
    }

    async fn update(&self, task: Task) -> Result<TaskVersion, A2AError> {
        let mut tasks = self.tasks.write().await;
        let Some(entry) = tasks.get(&task.id) else {
            return Err(A2AError::task_not_found(&task.id));
        };
        let version = entry.version + 1;
        self.persist(&task).await?;
        let id = task.id.clone();
        tasks.insert(id, StoredEntry { task, version });
        Ok(version)
    }

    async fn get(&self, task_id: &str) -> Result<Option<Task>, A2AError> {
        Ok(self.tasks.read().await.get(task_id).map(|e| e.task.clone()))
    }

    async fn list(&self, req: &ListTasksRequest) -> Result<ListTasksResponse, A2AError> {
        let tasks = self.tasks.read().await;
        let mut matching: Vec<Task> = tasks
            .values()
            .map(|e| &e.task)
            .filter(|task| {
                if let Some(ctx) = &req.context_id
                    && task.context_id != *ctx
                {
                    return false;
                }
                if let Some(status) = &req.status
                    && task.status.state != *status
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        matching.sort_by(|a, b| a.id.cmp(&b.id));

        let page_size = a2a_server::pagination::resolve_page_size(req.page_size);
        let start = if let Some(token) = &req.page_token {
            token
                .parse::<usize>()
                .map_err(|e| A2AError::invalid_params(format!("invalid page token: {e}")))?
        } else {
            0
        };
        let total_size = matching.len();
        let start = start.min(total_size);
        let end = start.saturating_add(page_size).min(total_size);
        let mut page: Vec<Task> = matching.get(start..end).unwrap_or_default().to_vec();
        for task in &mut page {
            apply_history_length(task, req.history_length);
        }
        let next_page_token = if end < total_size {
            end.to_string()
        } else {
            String::new()
        };

        Ok(ListTasksResponse {
            tasks: page,
            next_page_token,
            page_size: saturating_i32(page_size),
            total_size: saturating_i32(total_size),
        })
    }
}

/// Wraps a [`SharedTaskStore`] so it can be handed to
/// `a2a_server::DefaultRequestHandler::new` (which takes `impl TaskStore` by
/// value and wraps it in its own `Arc`) while
/// [`super::handler::ResiduumA2aHandler`] keeps its own clone of the same
/// `Arc<FileTaskStore>` — both then read and write the one underlying store,
/// rather than the handler silently getting a disconnected copy. A local
/// newtype rather than `impl TaskStore for SharedTaskStore` directly: `Arc`
/// isn't `#[fundamental]`, so the orphan rules refuse a foreign trait on a
/// foreign `Arc<Local>` even though the pointee is local.
pub(crate) struct DelegatingTaskStore(pub(crate) SharedTaskStore);

#[async_trait]
impl a2a_server::TaskStore for DelegatingTaskStore {
    async fn create(&self, task: Task) -> Result<TaskVersion, A2AError> {
        self.0.create(task).await
    }

    async fn update(&self, task: Task) -> Result<TaskVersion, A2AError> {
        self.0.update(task).await
    }

    async fn get(&self, task_id: &str) -> Result<Option<Task>, A2AError> {
        self.0.get(task_id).await
    }

    async fn list(&self, req: &ListTasksRequest) -> Result<ListTasksResponse, A2AError> {
        self.0.list(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a::{Message, Part, Role, TaskState, TaskStatus};
    use a2a_server::TaskStore as _;

    fn task(id: &str, context_id: &str, state: TaskState) -> Task {
        Task {
            id: id.to_string(),
            context_id: context_id.to_string(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: Some(chrono::Utc::now()),
            },
            artifacts: None,
            history: Some(vec![Message::new(Role::User, vec![Part::text("hi")])]),
            metadata: None,
        }
    }

    fn task_with_caller(id: &str, context_id: &str, state: TaskState, caller: &str) -> Task {
        let mut task = task(id, context_id, state);
        let mut metadata = HashMap::new();
        metadata.insert(
            CALLER_METADATA_KEY.to_string(),
            serde_json::Value::String(caller.to_string()),
        );
        task.metadata = Some(metadata);
        task
    }

    #[tokio::test]
    async fn create_persists_to_disk_and_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        let t = task("t1", "c1", TaskState::Submitted);
        store.create(t.clone()).await.unwrap();

        assert!(dir.path().join("t1.json").exists());
        let got = store.get("t1").await.unwrap().unwrap();
        assert_eq!(got.id, "t1");
    }

    #[tokio::test]
    async fn load_recovers_tasks_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = FileTaskStore::load(dir.path()).await.unwrap();
            store
                .create(task("t1", "c1", TaskState::Working))
                .await
                .unwrap();
        }
        let reloaded = FileTaskStore::load(dir.path()).await.unwrap();
        let got = reloaded.get("t1").await.unwrap().unwrap();
        assert_eq!(got.status.state, TaskState::Working);
    }

    #[tokio::test]
    async fn load_prunes_old_terminal_tasks_but_keeps_recent_ones() {
        let dir = tempfile::tempdir().unwrap();
        let mut old_task = task("old", "c1", TaskState::Completed);
        old_task.status.timestamp = Some(chrono::Utc::now() - chrono::Duration::days(31));
        let recent_task = task("recent", "c1", TaskState::Completed);

        std::fs::write(
            dir.path().join("old.json"),
            serde_json::to_vec(&old_task).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("recent.json"),
            serde_json::to_vec(&recent_task).unwrap(),
        )
        .unwrap();

        let store = FileTaskStore::load(dir.path()).await.unwrap();
        assert!(store.get("old").await.unwrap().is_none());
        assert!(store.get("recent").await.unwrap().is_some());
        assert!(!dir.path().join("old.json").exists());
    }

    #[tokio::test]
    async fn update_requires_an_existing_task() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        let err = store
            .update(task("missing", "c1", TaskState::Working))
            .await
            .unwrap_err();
        assert_eq!(err.code, a2a::error_code::TASK_NOT_FOUND);
    }

    #[tokio::test]
    async fn open_task_in_context_finds_only_non_terminal_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        store
            .create(task("t1", "c1", TaskState::Completed))
            .await
            .unwrap();
        assert!(store.open_task_in_context("c1").await.is_none());

        store
            .create(task("t2", "c1", TaskState::Working))
            .await
            .unwrap();
        let open = store.open_task_in_context("c1").await.unwrap();
        assert_eq!(open.id, "t2");
    }

    #[tokio::test]
    async fn tasks_for_caller_filters_by_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        store
            .create(task_with_caller(
                "t1",
                "c1",
                TaskState::Working,
                "key:alice",
            ))
            .await
            .unwrap();
        store
            .create(task_with_caller("t2", "c2", TaskState::Working, "key:bob"))
            .await
            .unwrap();

        let alice_tasks = store.tasks_for_caller("key:alice").await;
        assert_eq!(alice_tasks.len(), 1);
        assert_eq!(alice_tasks.first().unwrap().id, "t1");
    }

    #[tokio::test]
    async fn in_progress_tasks_excludes_terminal_and_input_required() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        store
            .create(task("t1", "c1", TaskState::Working))
            .await
            .unwrap();
        store
            .create(task("t2", "c2", TaskState::Submitted))
            .await
            .unwrap();
        store
            .create(task("t3", "c3", TaskState::Completed))
            .await
            .unwrap();
        store
            .create(task("t4", "c4", TaskState::InputRequired))
            .await
            .unwrap();

        let mut ids: Vec<String> = store
            .in_progress_tasks()
            .await
            .into_iter()
            .map(|t| t.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["t1".to_string(), "t2".to_string()]);
    }

    #[tokio::test]
    async fn list_paginates_and_filters_by_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTaskStore::load(dir.path()).await.unwrap();
        for i in 0..5 {
            store
                .create(task(&format!("t{i}"), "c1", TaskState::Working))
                .await
                .unwrap();
        }
        store
            .create(task("other", "c2", TaskState::Working))
            .await
            .unwrap();

        let req = ListTasksRequest {
            context_id: Some("c1".to_string()),
            status: None,
            page_size: Some(2),
            page_token: None,
            history_length: None,
            status_timestamp_after: None,
            include_artifacts: None,
            tenant: None,
        };
        let resp = store.list(&req).await.unwrap();
        assert_eq!(resp.tasks.len(), 2);
        assert_eq!(resp.total_size, 5);
        assert!(!resp.next_page_token.is_empty());
    }
}

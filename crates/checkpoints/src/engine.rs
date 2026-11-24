use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;

use residuum_bus::Publisher;

use super::backend::{GitRepo, SnapshotCommit, SnapshotFile};
use super::types::{
    ChangedPath, CheckpointContext, CheckpointDetail, CheckpointError, CheckpointPage,
    CheckpointSummary, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
};
use super::{exclude, notice};
use residuum_config::paths::TeamPaths;
use residuum_workspace::team_files::{TeamPathGuard, TeamWriteCoordinator, TeamWriter};

const HUB_TRACKED_FILES: &[&str] = &[
    "config.toml",
    "secrets.toml.enc",
    "agent-keys.toml.enc",
    "a2a-keys.toml",
];

const AGENT_CONFIG_TRACKED_FILES: &[&str] = &["config.toml", "providers.toml"];

const DEFAULT_PAGE_LIMIT: usize = 50;

struct TreeRepo {
    root: PathBuf,
    repo: Arc<Mutex<GitRepo>>,
    git_dir: PathBuf,
}

impl TreeRepo {
    fn open(root: PathBuf, git_dir: PathBuf) -> Result<Self, CheckpointError> {
        let repo = GitRepo::open_or_init(&git_dir)?;
        Ok(Self {
            root,
            repo: Arc::new(Mutex::new(repo)),
            git_dir,
        })
    }
}

pub struct SharedCheckpointRepos {
    team: TreeRepo,
    hub: TreeRepo,
}

impl SharedCheckpointRepos {
    pub fn open(
        hub_dir: &Path,
        team: &TeamPaths,
        checkpoints_dir: &Path,
    ) -> Result<Arc<Self>, CheckpointError> {
        Ok(Arc::new(Self {
            team: TreeRepo::open(
                team.root().to_path_buf(),
                checkpoints_dir.join(RepoKind::Team.dir_name()),
            )?,
            hub: TreeRepo::open(
                hub_dir.to_path_buf(),
                checkpoints_dir.join(RepoKind::Hub.dir_name()),
            )?,
        }))
    }
}

const AGENT_REPOS_DIR: &str = "agents";

#[must_use]
pub fn agent_repos_dir(checkpoints_dir: &Path, name: &str) -> PathBuf {
    checkpoints_dir.join(AGENT_REPOS_DIR).join(name)
}

pub fn agents_with_history(checkpoints_dir: &Path) -> Result<Vec<String>, CheckpointError> {
    let dir = checkpoints_dir.join(AGENT_REPOS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CheckpointError::Io(format!(
                "failed to read {}: {e}",
                dir.display()
            )));
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join(RepoKind::Workspace.dir_name()).is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

pub struct CheckpointEngine {
    workspace: TreeRepo,
    agent_config: TreeRepo,
    shared: Arc<SharedCheckpointRepos>,
    publisher: Option<Publisher>,
    team_coordinator: Option<TeamWriteCoordinator>,
    scratch_guard: Option<tempfile::TempDir>,
    pending: Arc<tokio::sync::watch::Sender<usize>>,
}

impl CheckpointEngine {
    pub fn new(
        agent_name: &str,
        workspace_root: PathBuf,
        team: &TeamPaths,
        agent_config_dir: PathBuf,
        hub_dir: impl AsRef<Path>,
        checkpoints_dir: &Path,
        publisher: Option<Publisher>,
    ) -> Result<Self, CheckpointError> {
        let shared = SharedCheckpointRepos::open(hub_dir.as_ref(), team, checkpoints_dir)?;
        Self::with_shared_repos(
            shared,
            agent_name,
            workspace_root,
            agent_config_dir,
            checkpoints_dir,
            publisher,
        )
    }

    pub fn with_shared_repos(
        shared: Arc<SharedCheckpointRepos>,
        agent_name: &str,
        workspace_root: PathBuf,
        agent_config_dir: PathBuf,
        checkpoints_dir: &Path,
        publisher: Option<Publisher>,
    ) -> Result<Self, CheckpointError> {
        let agent_repos_dir = agent_repos_dir(checkpoints_dir, agent_name);
        Ok(Self {
            workspace: TreeRepo::open(
                workspace_root,
                agent_repos_dir.join(RepoKind::Workspace.dir_name()),
            )?,
            agent_config: TreeRepo::open(
                agent_config_dir,
                agent_repos_dir.join(RepoKind::AgentConfig.dir_name()),
            )?,
            shared,
            publisher,
            team_coordinator: None,
            scratch_guard: None,
            pending: Arc::new(tokio::sync::watch::channel(0).0),
        })
    }

    #[must_use]
    pub fn with_team_coordinator(mut self, coordinator: TeamWriteCoordinator) -> Self {
        self.team_coordinator = Some(coordinator);
        self
    }

    #[must_use]
    pub(crate) fn with_tempdir_guard(mut self, dir: tempfile::TempDir) -> Self {
        self.scratch_guard = Some(dir);
        self
    }

    pub fn hub_scoped(
        shared: Arc<SharedCheckpointRepos>,
        hub_dir: &Path,
    ) -> Result<Self, CheckpointError> {
        let scratch = tempfile::tempdir()
            .map_err(|e| CheckpointError::Io(format!("failed to create scratch directory: {e}")))?;
        let engine = Self::with_shared_repos(
            shared,
            "hub",
            hub_dir.join("_unused-workspace"),
            hub_dir.join("_unused-agent-config"),
            scratch.path(),
            None,
        )?;
        Ok(engine.with_tempdir_guard(scratch))
    }

    #[must_use]
    pub fn open_for_cli(hub_dir: &Path) -> Option<Self> {
        let checkpoints_dir = residuum_config::HubPaths::new(hub_dir).checkpoints_dir();
        let placeholder_team = TeamPaths::new(hub_dir.join("_unused-team"));
        let opened = SharedCheckpointRepos::open(hub_dir, &placeholder_team, &checkpoints_dir)
            .and_then(|shared| Self::hub_scoped(shared, hub_dir));
        match opened {
            Ok(engine) => Some(engine),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    checkpoints_dir = %checkpoints_dir.display(),
                    "couldn't open checkpoint repositories; continuing without checkpointing this command"
                );
                None
            }
        }
    }

    fn tree(&self, kind: RepoKind) -> &TreeRepo {
        match kind {
            RepoKind::Workspace => &self.workspace,
            RepoKind::Team => &self.shared.team,
            RepoKind::AgentConfig => &self.agent_config,
            RepoKind::Hub => &self.shared.hub,
        }
    }

    fn repo(&self, kind: RepoKind) -> Arc<Mutex<GitRepo>> {
        Arc::clone(&self.tree(kind).repo)
    }

    fn dest_root(&self, kind: RepoKind) -> PathBuf {
        self.tree(kind).root.clone()
    }

    fn git_dir(&self, kind: RepoKind) -> PathBuf {
        self.tree(kind).git_dir.clone()
    }

    #[must_use]
    pub fn pending_checkpoints(&self) -> usize {
        *self.pending.borrow()
    }

    pub async fn settled(&self) {
        let mut pending = self.pending.subscribe();
        if pending.wait_for(|count| *count == 0).await.is_err() {
            tracing::debug!("the checkpoint engine went away while its checkpoints settled");
        }
    }

    pub fn spawn_turn_start_checkpoint(&self, ctx: CheckpointContext) {
        self.spawn_workspace_checkpoint(ctx);
    }

    pub fn spawn_turn_end_checkpoint(&self, ctx: CheckpointContext) {
        self.spawn_workspace_checkpoint(ctx);
    }

    fn spawn_workspace_checkpoint(&self, ctx: CheckpointContext) {
        self.spawn_tree_checkpoint(RepoKind::Workspace, ctx.clone());
        self.spawn_tree_checkpoint(RepoKind::Team, ctx);
    }

    fn spawn_tree_checkpoint(&self, kind: RepoKind, ctx: CheckpointContext) {
        let repo = self.repo(kind);
        let root = self.dest_root(kind);
        let publisher = self.publisher.clone();
        let in_flight = PendingCheckpoint::start(&self.pending);
        residuum_core::util::spawn_in_span(async move {
            let _in_flight = in_flight;
            let task_ctx = ctx.clone();
            let result = residuum_core::util::spawn_blocking_in_span(move || {
                commit_tree(kind, &repo, &root, &task_ctx).map(|commit| commit.recorded_id())
            })
            .await
            .unwrap_or_else(|e| {
                Err(CheckpointError::Git(format!(
                    "checkpoint task panicked: {e}"
                )))
            });
            report_outcome(publisher.as_ref(), kind.dir_name(), &ctx, &result).await;
        });
    }

    pub async fn checkpoint_workspace_before_action(&self, ctx: CheckpointContext) {
        let _checkpoint_id = self.checkpoint_workspace_id_before_action(ctx).await;
    }

    #[must_use]
    pub async fn checkpoint_workspace_id_before_action(
        &self,
        ctx: CheckpointContext,
    ) -> Option<String> {
        self.checkpoint_tree_id_before_action(RepoKind::Workspace, ctx)
            .await
    }

    pub async fn checkpoint_team_before_action(&self, ctx: CheckpointContext) {
        let _checkpoint_id = self.checkpoint_team_id_before_action(ctx).await;
    }

    #[must_use]
    pub async fn checkpoint_team_id_before_action(&self, ctx: CheckpointContext) -> Option<String> {
        self.checkpoint_tree_id_before_action(RepoKind::Team, ctx)
            .await
    }

    async fn checkpoint_tree_id_before_action(
        &self,
        kind: RepoKind,
        ctx: CheckpointContext,
    ) -> Option<String> {
        let repo = self.repo(kind);
        let root = self.dest_root(kind);
        let task_ctx = ctx.clone();
        let outcome = residuum_core::util::spawn_blocking_in_span(move || {
            commit_tree(kind, &repo, &root, &task_ctx)
        })
        .await
        .unwrap_or_else(|e| {
            Err(CheckpointError::Git(format!(
                "checkpoint task panicked: {e}"
            )))
        });
        let (report, id) = id_and_report(outcome);
        report_outcome(self.publisher.as_ref(), kind.dir_name(), &ctx, &report).await;
        id
    }

    pub fn checkpoint_config_kind_now(
        &self,
        kind: RepoKind,
        ctx: &CheckpointContext,
    ) -> Result<Option<String>, CheckpointError> {
        let guard = self.repo(kind);
        let guard = guard.lock().unwrap_or_else(PoisonError::into_inner);
        let files = collect_config_files(kind, &self.dest_root(kind))?;
        guard.commit_snapshot(&files, Utc::now(), ctx)
    }

    pub async fn checkpoint_config_before_write(&self, ctx: CheckpointContext) {
        let _checkpoint_id = self.checkpoint_config_id_before_write(ctx).await;
    }

    #[must_use]
    pub async fn checkpoint_config_id_before_write(
        &self,
        ctx: CheckpointContext,
    ) -> Option<String> {
        self.checkpoint_config_kind_id_before_write(RepoKind::Hub, ctx)
            .await
    }

    #[must_use]
    pub async fn checkpoint_config_kind_id_before_write(
        &self,
        kind: RepoKind,
        ctx: CheckpointContext,
    ) -> Option<String> {
        let (report, id) = id_and_report(self.config_snapshot(kind, &ctx));
        report_outcome(self.publisher.as_ref(), kind.dir_name(), &ctx, &report).await;
        id
    }

    fn config_snapshot(
        &self,
        kind: RepoKind,
        ctx: &CheckpointContext,
    ) -> Result<SnapshotCommit, CheckpointError> {
        let guard = self.repo(kind);
        let guard = guard.lock().unwrap_or_else(PoisonError::into_inner);
        let files = collect_config_files(kind, &self.dest_root(kind))?;
        guard.commit_snapshot_outcome(&files, Utc::now(), ctx)
    }

    pub async fn list_checkpoints(
        &self,
        kind: RepoKind,
        path_filter: Option<String>,
        turn_filter: Option<String>,
        before: Option<String>,
        limit: Option<usize>,
    ) -> Result<CheckpointPage, CheckpointError> {
        let repo = self.repo(kind);
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).max(1);
        residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let before_id = before
                .map(|id| guard.resolve_commit(&id))
                .transpose()
                .map_err(|e| {
                    tracing::debug!(error = %e, "checkpoint list cursor did not resolve");
                    CheckpointError::InvalidCursor
                })?;
            let (rows, next) = guard.log(
                before_id,
                limit,
                path_filter.as_deref(),
                turn_filter.as_deref(),
            )?;
            let items = rows
                .into_iter()
                .map(|(id, fields, changed_path_count)| CheckpointSummary {
                    id: id.to_hex().to_string(),
                    timestamp: fields.timestamp,
                    address: fields.address,
                    run_id: fields.run_id,
                    turn_id: fields.turn_id,
                    trigger: fields.trigger,
                    summary: fields.summary,
                    changed_path_count,
                })
                .collect();
            Ok(CheckpointPage {
                items,
                next_cursor: next.map(|id| id.to_hex().to_string()),
            })
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("list task panicked: {e}"))))
    }

    pub async fn show_checkpoint(
        &self,
        kind: RepoKind,
        id: String,
    ) -> Result<CheckpointDetail, CheckpointError> {
        let repo = self.repo(kind);
        residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            let fields = guard.commit_fields(oid)?;
            let changed_paths = guard.changed_paths(oid)?;
            Ok(CheckpointDetail {
                summary: CheckpointSummary {
                    id,
                    timestamp: fields.timestamp,
                    address: fields.address,
                    run_id: fields.run_id,
                    turn_id: fields.turn_id,
                    trigger: fields.trigger,
                    summary: fields.summary,
                    changed_path_count: changed_paths.len(),
                },
                changed_paths,
            })
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("show task panicked: {e}"))))
    }

    /// Unified-diff text for `path` at checkpoint `id`, relative to the
    /// checkpoint before it. `None` if `path` didn't change there.
    ///
    /// # Errors
    /// Returns [`CheckpointError::NotFound`] if `id` doesn't name a
    /// checkpoint in `kind`.
    pub async fn file_diff(
        &self,
        kind: RepoKind,
        id: String,
        path: String,
    ) -> Result<Option<String>, CheckpointError> {
        let repo = self.repo(kind);
        residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            guard.file_diff(oid, &path)
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("diff task panicked: {e}"))))
    }

    pub async fn file_content_at(
        &self,
        kind: RepoKind,
        id: String,
        path: String,
    ) -> Result<Option<Vec<u8>>, CheckpointError> {
        let repo = self.repo(kind);
        residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            guard.file_content_at(oid, &path)
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("read task panicked: {e}"))))
    }

    pub async fn stats(&self, kind: RepoKind) -> Result<RepoStats, CheckpointError> {
        let repo = self.repo(kind);
        let git_dir = self.git_dir(kind);
        residuum_core::util::spawn_blocking_in_span(move || {
            let (checkpoint_count, oldest) = {
                let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
                guard.stats()?
            };
            Ok(RepoStats {
                on_disk_bytes: dir_size(&git_dir),
                checkpoint_count,
                oldest,
            })
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("stats task panicked: {e}"))))
    }

    pub async fn restore_path(
        &self,
        kind: RepoKind,
        id: String,
        path: String,
        ctx: CheckpointContext,
        writer: &TeamWriter,
    ) -> Result<RestoreOutcome, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let team_locks = self
            .lock_restore_targets(kind, &id, std::slice::from_ref(&path))
            .await?;
        let restored_paths = {
            let repo = Arc::clone(&repo);
            let id = id.clone();
            let path = path.clone();
            residuum_core::util::spawn_blocking_in_span(move || {
                let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
                let oid = guard.resolve_commit(&id)?;
                guard.restore_paths(oid, &path, &dest_root)
            })
            .await
            .unwrap_or_else(|e| Err(CheckpointError::Git(format!("restore task panicked: {e}"))))?
        };
        record_team_writes(team_locks, writer, None).await;

        let checkpoint_id = self.checkpoint_after_mutation(kind, ctx).await;
        notice::publish(
            self.publisher.as_ref(),
            format!(
                "Restored {path} ({} path(s)) from checkpoint {}.",
                restored_paths.len(),
                short_id(&id)
            ),
        )
        .await;

        Ok(RestoreOutcome {
            checkpoint_id,
            restored_paths,
        })
    }

    pub async fn restore_tree(
        &self,
        kind: RepoKind,
        id: String,
        ctx: CheckpointContext,
        writer: &TeamWriter,
    ) -> Result<RestoreOutcome, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let team_locks = if self.team_coordinator_for(kind).is_some() {
            let top_level = {
                let repo = Arc::clone(&repo);
                let id = id.clone();
                residuum_core::util::spawn_blocking_in_span(move || {
                    let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
                    let oid = guard.resolve_commit(&id)?;
                    guard.root_entry_names(oid)
                })
                .await
                .unwrap_or_else(|e| {
                    Err(CheckpointError::Git(format!("restore task panicked: {e}")))
                })?
            };
            self.lock_restore_targets(kind, &id, &top_level).await?
        } else {
            Vec::new()
        };
        let restored_paths = {
            let repo = Arc::clone(&repo);
            let id = id.clone();
            residuum_core::util::spawn_blocking_in_span(move || {
                let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
                let oid = guard.resolve_commit(&id)?;
                guard.restore_root(oid, &dest_root)
            })
            .await
            .unwrap_or_else(|e| Err(CheckpointError::Git(format!("restore task panicked: {e}"))))?
        };
        record_team_writes(team_locks, writer, None).await;

        let checkpoint_id = self.checkpoint_after_mutation(kind, ctx).await;
        notice::publish(
            self.publisher.as_ref(),
            format!(
                "Restored {} path(s) from checkpoint {}.",
                restored_paths.len(),
                short_id(&id)
            ),
        )
        .await;

        Ok(RestoreOutcome {
            checkpoint_id,
            restored_paths,
        })
    }

    pub async fn undo_checkpoint(
        &self,
        kind: RepoKind,
        id: String,
        ctx: CheckpointContext,
        writer: &TeamWriter,
    ) -> Result<UndoOutcome, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let team_locks = self.lock_undo_targets(kind, &id).await?;
        let (reverted_paths, skipped_paths) = {
            let repo = Arc::clone(&repo);
            let id = id.clone();
            residuum_core::util::spawn_blocking_in_span(move || {
                undo_checkpoint_paths(&repo, &id, &dest_root)
            })
            .await
            .unwrap_or_else(|e| Err(CheckpointError::Git(format!("undo task panicked: {e}"))))?
        };
        record_team_writes(team_locks, writer, Some(&reverted_paths)).await;

        let checkpoint_id = self.checkpoint_after_mutation(kind, ctx).await;
        notice::publish(
            self.publisher.as_ref(),
            undo_notice_message(&id, &reverted_paths, &skipped_paths),
        )
        .await;

        Ok(UndoOutcome {
            checkpoint_id,
            reverted_paths,
            skipped_paths,
        })
    }

    fn team_coordinator_for(&self, kind: RepoKind) -> Option<&TeamWriteCoordinator> {
        match kind {
            RepoKind::Team => self.team_coordinator.as_ref(),
            RepoKind::Workspace | RepoKind::AgentConfig | RepoKind::Hub => None,
        }
    }

    async fn lock_restore_targets(
        &self,
        kind: RepoKind,
        id: &str,
        paths: &[String],
    ) -> Result<Vec<(String, TeamPathGuard)>, CheckpointError> {
        let Some(coordinator) = self.team_coordinator_for(kind) else {
            return Ok(Vec::new());
        };
        let mut wanted: BTreeSet<String> = BTreeSet::new();
        let mut held: Vec<(String, TeamPathGuard)> = Vec::new();
        for _ in 0..RESTORE_LOCK_ATTEMPTS {
            let planned = self.plan_restore(kind, id, paths).await?;
            if planned.is_empty() {
                return Ok(Vec::new());
            }
            if !held.is_empty() && planned.is_subset(&wanted) {
                return Ok(held);
            }
            drop(held);
            wanted.extend(planned);
            held = self.lock_relative(coordinator, kind, &wanted).await;
        }
        Err(CheckpointError::Io(format!(
            "team files under {} kept changing while the restore waited for them; try again",
            paths.join(", ")
        )))
    }

    async fn lock_undo_targets(
        &self,
        kind: RepoKind,
        id: &str,
    ) -> Result<Vec<(String, TeamPathGuard)>, CheckpointError> {
        let Some(coordinator) = self.team_coordinator_for(kind) else {
            return Ok(Vec::new());
        };
        let repo = self.repo(kind);
        let id = id.to_string();
        let changed: BTreeSet<String> = residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            Ok::<_, CheckpointError>(
                guard
                    .changed_paths(oid)?
                    .into_iter()
                    .map(|change| change.path)
                    .collect(),
            )
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("undo task panicked: {e}"))))?;
        Ok(self.lock_relative(coordinator, kind, &changed).await)
    }

    async fn lock_relative(
        &self,
        coordinator: &TeamWriteCoordinator,
        kind: RepoKind,
        relative: &BTreeSet<String>,
    ) -> Vec<(String, TeamPathGuard)> {
        let root = self.dest_root(kind);
        let absolute: Vec<PathBuf> = relative.iter().map(|rel| root.join(rel)).collect();
        let mut guards = coordinator.lock_all(&absolute).await;
        let mut held = Vec::with_capacity(guards.len());
        for rel in relative {
            let target = root.join(rel);
            if let Some(index) = guards.iter().position(|guard| guard.path() == target) {
                held.push((rel.clone(), guards.swap_remove(index)));
            }
        }
        held
    }

    async fn plan_restore(
        &self,
        kind: RepoKind,
        id: &str,
        paths: &[String],
    ) -> Result<BTreeSet<String>, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let id = id.to_string();
        let paths = paths.to_vec();
        residuum_core::util::spawn_blocking_in_span(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            let mut files = BTreeSet::new();
            for path in &paths {
                files.extend(guard.files_under(oid, path)?);
                if super::is_root_relative(path) {
                    collect_disk_files(&dest_root, path, &mut files);
                }
            }
            Ok(files)
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("restore task panicked: {e}"))))
    }

    async fn checkpoint_after_mutation(&self, kind: RepoKind, ctx: CheckpointContext) -> String {
        let result = match kind {
            RepoKind::Workspace | RepoKind::Team => {
                let repo = self.repo(kind);
                let root = self.dest_root(kind);
                let task_ctx = ctx.clone();
                residuum_core::util::spawn_blocking_in_span(move || {
                    commit_tree(kind, &repo, &root, &task_ctx).map(|commit| commit.recorded_id())
                })
                .await
                .unwrap_or_else(|e| {
                    Err(CheckpointError::Git(format!(
                        "checkpoint task panicked: {e}"
                    )))
                })
            }
            RepoKind::AgentConfig | RepoKind::Hub => self.checkpoint_config_kind_now(kind, &ctx),
        };
        report_outcome(self.publisher.as_ref(), kind.dir_name(), &ctx, &result).await;
        if let Ok(Some(id)) = &result {
            return id.clone();
        }
        current_tip(&self.repo(kind))
    }
}

const RESTORE_LOCK_ATTEMPTS: usize = 8;

fn collect_disk_files(root: &Path, rel: &str, out: &mut BTreeSet<String>) {
    let full = root.join(rel);
    let Ok(metadata) = std::fs::symlink_metadata(&full) else {
        return;
    };
    if !metadata.is_dir() {
        out.insert(rel.to_string());
        return;
    }
    let Ok(entries) = std::fs::read_dir(&full) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            collect_disk_files(root, &format!("{rel}/{name}"), out);
        }
    }
}

async fn record_team_writes(
    held: Vec<(String, TeamPathGuard)>,
    writer: &TeamWriter,
    only: Option<&[String]>,
) {
    for (rel, guard) in held {
        if only.is_some_and(|only| !only.contains(&rel)) {
            continue;
        }
        if let Err(e) = guard.record_written(writer).await {
            tracing::warn!(error = %e, path = %rel, "couldn't record a restored team file's writer");
        }
    }
}

fn current_tip(repo: &Mutex<GitRepo>) -> String {
    repo.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .tip()
        .ok()
        .flatten()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_default()
}

fn undo_checkpoint_paths(
    repo: &Mutex<GitRepo>,
    id: &str,
    dest_root: &Path,
) -> Result<(Vec<String>, Vec<String>), CheckpointError> {
    let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
    let oid = guard.resolve_commit(id)?;
    let parent = guard.parent_of(oid)?;
    let changes = guard.changed_paths(oid)?;

    let mut reverted = Vec::new();
    let mut skipped = Vec::new();
    for change in changes {
        if path_changed_since(&guard, oid, &change, dest_root)? {
            skipped.push(change.path);
            continue;
        }
        revert_one_path(&guard, parent, &change, dest_root)?;
        reverted.push(change.path);
    }
    Ok((reverted, skipped))
}

fn path_changed_since(
    guard: &GitRepo,
    id: gix::ObjectId,
    change: &ChangedPath,
    dest_root: &Path,
) -> Result<bool, CheckpointError> {
    let recorded = guard.file_content_at(id, &change.path)?;
    let on_disk = std::fs::read(dest_root.join(&change.path)).ok();
    Ok(recorded != on_disk)
}

fn revert_one_path(
    guard: &GitRepo,
    parent: Option<gix::ObjectId>,
    change: &ChangedPath,
    dest_root: &Path,
) -> Result<(), CheckpointError> {
    let existed_before = match parent {
        Some(parent_id) => guard.file_content_at(parent_id, &change.path)?.is_some(),
        None => false,
    };
    if let (true, Some(parent_id)) = (existed_before, parent) {
        guard.restore_paths(parent_id, &change.path, dest_root)?;
    } else {
        remove_path(&dest_root.join(&change.path))
            .map_err(|e| CheckpointError::Io(e.to_string()))?;
    }
    Ok(())
}

fn remove_path(target: &Path) -> std::io::Result<()> {
    if target.is_dir() {
        std::fs::remove_dir_all(target)
    } else if target.exists() {
        std::fs::remove_file(target)
    } else {
        Ok(())
    }
}

fn collect_tree_files(root: &Path, is_excluded: fn(&str) -> bool) -> Vec<SnapshotFile> {
    let mut files = Vec::new();
    walk_dir(root, root, is_excluded, &mut files);
    files
}

fn walk_dir(root: &Path, dir: &Path, is_excluded: fn(&str) -> bool, out: &mut Vec<SnapshotFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let abs_path = entry.path();
        let Ok(rel_path) = abs_path.strip_prefix(root) else {
            continue;
        };
        let Some(rel_str) = to_slash_string(rel_path) else {
            continue;
        };
        if is_excluded(&rel_str) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_dir(root, &abs_path, is_excluded, out);
        } else if file_type.is_file() {
            out.push(SnapshotFile {
                rel_path: rel_str,
                executable: is_executable(&abs_path),
                abs_path,
            });
        }
    }
}

fn to_slash_string(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        parts.push(component.as_os_str().to_str()?.to_string());
    }
    Some(parts.join("/"))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

fn collect_config_files(
    kind: RepoKind,
    config_dir: &Path,
) -> Result<Vec<SnapshotFile>, CheckpointError> {
    let tracked: &[&str] = match kind {
        RepoKind::Hub => HUB_TRACKED_FILES,
        RepoKind::AgentConfig => AGENT_CONFIG_TRACKED_FILES,
        RepoKind::Workspace | RepoKind::Team => return Err(CheckpointError::NotAConfigRepo),
    };
    Ok(tracked
        .iter()
        .filter_map(|name| {
            let abs_path = config_dir.join(name);
            abs_path.is_file().then(|| SnapshotFile {
                rel_path: (*name).to_string(),
                abs_path,
                executable: false,
            })
        })
        .collect())
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total += dir_size(&entry.path());
        } else if let Ok(metadata) = entry.metadata() {
            total += metadata.len();
        }
    }
    total
}

fn commit_tree(
    kind: RepoKind,
    repo: &Mutex<GitRepo>,
    root: &Path,
    ctx: &CheckpointContext,
) -> Result<SnapshotCommit, CheckpointError> {
    let is_excluded = match kind {
        RepoKind::Team => exclude::is_team_excluded,
        RepoKind::Workspace | RepoKind::AgentConfig | RepoKind::Hub => exclude::is_excluded,
    };
    let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
    let files = collect_tree_files(root, is_excluded);
    guard.commit_snapshot_outcome(&files, Utc::now(), ctx)
}

fn id_and_report(
    outcome: Result<SnapshotCommit, CheckpointError>,
) -> (Result<Option<String>, CheckpointError>, Option<String>) {
    match outcome {
        Ok(commit) => {
            let id = commit.current_id();
            (Ok(commit.recorded_id()), id)
        }
        Err(error) => (Err(error), None),
    }
}

struct PendingCheckpoint(Arc<tokio::sync::watch::Sender<usize>>);

impl PendingCheckpoint {
    fn start(pending: &Arc<tokio::sync::watch::Sender<usize>>) -> Self {
        pending.send_modify(|count| *count += 1);
        Self(Arc::clone(pending))
    }
}

impl Drop for PendingCheckpoint {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

async fn report_outcome(
    publisher: Option<&Publisher>,
    repo_label: &str,
    ctx: &CheckpointContext,
    result: &Result<Option<String>, CheckpointError>,
) {
    match result {
        Ok(Some(id)) => {
            tracing::debug!(repo = repo_label, checkpoint = %id, trigger = ctx.trigger.label(), "checkpoint recorded");
        }
        Ok(None) => {
            tracing::trace!(
                repo = repo_label,
                trigger = ctx.trigger.label(),
                "checkpoint skipped: nothing changed"
            );
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                repo = repo_label,
                trigger = ctx.trigger.label(),
                address = %ctx.address,
                "checkpoint failed"
            );
            notice::publish(
                publisher,
                format!(
                    "Couldn't record a workspace checkpoint ({repo_label}, {}): {e}. Continuing without it.",
                    ctx.trigger.label()
                ),
            )
            .await;
        }
    }
}

fn undo_notice_message(id: &str, reverted: &[String], skipped: &[String]) -> String {
    use std::fmt::Write as _;

    let mut message = format!(
        "Undid checkpoint {} — reverted {} path(s).",
        short_id(id),
        reverted.len()
    );
    if !skipped.is_empty() {
        _ = write!(
            message,
            " Skipped {} path(s) changed again since: {}.",
            skipped.len(),
            skipped.join(", ")
        );
    }
    message
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::super::types::CheckpointTrigger;
    use super::*;

    fn ctx(trigger: CheckpointTrigger, summary: &str) -> CheckpointContext {
        CheckpointContext::system(trigger, summary)
    }

    fn new_engine(dir: &Path) -> CheckpointEngine {
        CheckpointEngine::new(
            "test-agent",
            dir.join("workspace"),
            &residuum_config::paths::TeamPaths::new(dir.join("team")),
            dir.join("agent-config"),
            dir.join("hub"),
            &dir.join("checkpoints"),
            None,
        )
        .unwrap()
    }

    async fn wait_for_checkpoint_count(
        engine: &CheckpointEngine,
        kind: RepoKind,
        count: usize,
    ) -> Vec<CheckpointSummary> {
        residuum_core::testing::wait::guarded(
            "the spawned checkpoints to finish",
            engine.settled(),
        )
        .await;
        let page = engine
            .list_checkpoints(kind, None, None, None, Some(count))
            .await
            .unwrap();
        assert!(
            page.items.len() >= count,
            "expected {count} checkpoint(s), found {}",
            page.items.len()
        );
        page.items
    }

    #[tokio::test]
    async fn pending_counts_spawned_checkpoints_until_they_finish() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());
        let mut pending = engine.pending.subscribe();
        assert_eq!(engine.pending_checkpoints(), 0);
        std::fs::write(workspace.join("a.txt"), "one").unwrap();

        engine.spawn_turn_end_checkpoint(ctx(CheckpointTrigger::TurnEnd, "a turn"));
        assert_eq!(
            engine.pending_checkpoints(),
            2,
            "a turn hook checkpoints the workspace and the team, counted before it returns"
        );
        residuum_core::testing::wait::watch_until(
            "the checkpoints to finish",
            &mut pending,
            |count| *count == 0,
        )
        .await;
        engine.settled().await;
        let page = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "the finished checkpoint is recorded");
    }

    #[test]
    fn collect_config_files_rejects_the_workspace_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            collect_config_files(RepoKind::Workspace, dir.path()),
            Err(CheckpointError::NotAConfigRepo)
        ));
    }

    #[tokio::test]
    async fn turn_start_checkpoint_attributes_an_outside_edit() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        std::fs::write(workspace.join("notes.md"), "edited outside residuum").unwrap();

        engine.spawn_turn_start_checkpoint(ctx(
            CheckpointTrigger::TurnStart,
            "outside edit before turn start",
        ));

        let items = wait_for_checkpoint_count(&engine, RepoKind::Workspace, 1).await;
        let checkpoint = items.into_iter().next().unwrap();
        assert_eq!(checkpoint.trigger, CheckpointTrigger::TurnStart);
        let detail = engine
            .show_checkpoint(RepoKind::Workspace, checkpoint.id)
            .await
            .unwrap();
        assert!(detail.changed_paths.iter().any(|c| c.path == "notes.md"));
    }

    #[tokio::test]
    async fn turn_start_and_turn_end_each_produce_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        std::fs::write(workspace.join("notes.md"), "v1 (outside edit)").unwrap();
        engine.spawn_turn_start_checkpoint(ctx(CheckpointTrigger::TurnStart, "outside edit"));
        wait_for_checkpoint_count(&engine, RepoKind::Workspace, 1).await;

        std::fs::write(workspace.join("notes.md"), "v2 (the turn's own edit)").unwrap();
        engine.spawn_turn_end_checkpoint(ctx(CheckpointTrigger::TurnEnd, "wrote notes.md"));

        let items = wait_for_checkpoint_count(&engine, RepoKind::Workspace, 2).await;
        assert_eq!(
            items.len(),
            2,
            "one checkpoint each for turn start and turn end"
        );
        let newest = items.first().expect("two items");
        let oldest = items.get(1).expect("two items");
        assert_eq!(newest.trigger, CheckpointTrigger::TurnEnd);
        assert_eq!(oldest.trigger, CheckpointTrigger::TurnStart);
    }

    #[tokio::test]
    async fn list_checkpoints_filters_by_turn_id_to_find_a_turns_pair() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        let turn_ctx = |trigger, summary: &str| CheckpointContext {
            turn_id: Some("turn-42".to_string()),
            ..ctx(trigger, summary)
        };

        std::fs::write(workspace.join("notes.md"), "v1").unwrap();
        engine.spawn_turn_start_checkpoint(turn_ctx(CheckpointTrigger::TurnStart, "outside edit"));
        wait_for_checkpoint_count(&engine, RepoKind::Workspace, 1).await;

        std::fs::write(workspace.join("notes.md"), "v2").unwrap();
        engine.spawn_turn_end_checkpoint(turn_ctx(CheckpointTrigger::TurnEnd, "wrote notes.md"));
        wait_for_checkpoint_count(&engine, RepoKind::Workspace, 2).await;

        std::fs::write(workspace.join("notes.md"), "v3").unwrap();
        engine.spawn_turn_end_checkpoint(ctx(CheckpointTrigger::TurnEnd, "a different turn"));
        wait_for_checkpoint_count(&engine, RepoKind::Workspace, 3).await;

        let page = engine
            .list_checkpoints(
                RepoKind::Workspace,
                None,
                Some("turn-42".to_string()),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2, "only turn-42's start/end pair");
        assert!(
            page.items
                .iter()
                .all(|c| c.turn_id.as_deref() == Some("turn-42"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checkpoint_failure_never_blocks_or_fails_the_action() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let checkpoints_dir = dir.path().join("checkpoints");
        let engine = CheckpointEngine::new(
            "test-agent",
            workspace.clone(),
            &residuum_config::paths::TeamPaths::new(dir.path().join("team")),
            dir.path().join("agent-config"),
            dir.path().join("hub"),
            &checkpoints_dir,
            None,
        )
        .unwrap();
        std::fs::write(workspace.join("a.txt"), "1").unwrap();

        let objects_dir = checkpoints_dir
            .join(AGENT_REPOS_DIR)
            .join("test-agent")
            .join(RepoKind::Workspace.dir_name())
            .join("objects");
        std::fs::set_permissions(&objects_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        residuum_core::testing::wait::guarded(
            "checkpoint_workspace_before_action to return when the commit fails",
            engine.checkpoint_workspace_before_action(ctx(
                CheckpointTrigger::PreAction,
                "delete a.txt",
            )),
        )
        .await;

        std::fs::set_permissions(&objects_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[tokio::test]
    async fn undo_skips_a_path_changed_again_since_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        std::fs::write(workspace.join("a.txt"), "v1").unwrap();
        std::fs::write(workspace.join("b.txt"), "v1").unwrap();
        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::TurnEnd, "first"))
            .await;

        std::fs::write(workspace.join("a.txt"), "v2").unwrap();
        std::fs::write(workspace.join("b.txt"), "v2").unwrap();
        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::TurnEnd, "second"))
            .await;
        let page = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        let second_id = page.items.first().unwrap().id.clone();

        std::fs::write(workspace.join("b.txt"), "v3 (edited after the checkpoint)").unwrap();

        let outcome = engine
            .undo_checkpoint(
                RepoKind::Workspace,
                second_id,
                ctx(CheckpointTrigger::Undo, "undo second"),
                &residuum_workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();
        assert_eq!(outcome.reverted_paths, vec!["a.txt".to_string()]);
        assert_eq!(outcome.skipped_paths, vec!["b.txt".to_string()]);
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v1"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("b.txt")).unwrap(),
            "v3 (edited after the checkpoint)",
            "a path changed again since must never be clobbered by undo"
        );
    }

    #[tokio::test]
    async fn undo_is_itself_undoable() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        std::fs::write(workspace.join("a.txt"), "v1").unwrap();
        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::TurnEnd, "first"))
            .await;
        std::fs::write(workspace.join("a.txt"), "v2").unwrap();
        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::TurnEnd, "second"))
            .await;
        let second_id = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap()
            .items
            .first()
            .unwrap()
            .id
            .clone();

        let undo_outcome = engine
            .undo_checkpoint(
                RepoKind::Workspace,
                second_id,
                ctx(CheckpointTrigger::Undo, "undo second"),
                &residuum_workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v1"
        );

        let redo_outcome = engine
            .undo_checkpoint(
                RepoKind::Workspace,
                undo_outcome.checkpoint_id,
                ctx(CheckpointTrigger::Undo, "undo the undo"),
                &residuum_workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();
        assert_eq!(redo_outcome.reverted_paths, vec!["a.txt".to_string()]);
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v2"
        );
    }

    #[tokio::test]
    async fn config_repo_never_configures_a_remote() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"").unwrap();
        let engine = new_engine(dir.path());

        engine
            .checkpoint_config_before_write(ctx(
                CheckpointTrigger::PreConfigWrite,
                "write config.toml",
            ))
            .await;

        let config_git_dir = dir
            .path()
            .join("checkpoints")
            .join(RepoKind::Hub.dir_name());
        let config_text =
            std::fs::read_to_string(config_git_dir.join("config")).unwrap_or_default();
        assert!(
            !config_text.contains("[remote"),
            "the config checkpoint repo must never have a remote configured: {config_text}"
        );
    }

    #[tokio::test]
    async fn config_snapshot_never_includes_machine_key_files() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"").unwrap();
        std::fs::write(hub_dir.join("secrets.toml.enc"), b"ciphertext").unwrap();
        std::fs::write(hub_dir.join("secrets.key"), [0_u8; 32]).unwrap();
        std::fs::write(hub_dir.join("agent-keys.key"), [0_u8; 32]).unwrap();
        let engine = new_engine(dir.path());

        let id = engine
            .checkpoint_config_kind_now(
                RepoKind::Hub,
                &ctx(CheckpointTrigger::PreConfigWrite, "test"),
            )
            .unwrap()
            .unwrap();
        let detail = engine.show_checkpoint(RepoKind::Hub, id).await.unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert!(paths.contains(&"config.toml"));
        assert!(paths.contains(&"secrets.toml.enc"));
        assert!(
            !paths.iter().any(|p| Path::new(p)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("key"))),
            "machine key files must never be checkpointed: {paths:?}"
        );
    }

    #[tokio::test]
    async fn config_snapshot_never_includes_the_push_signing_key_or_devices() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"").unwrap();
        std::fs::write(hub_dir.join("push-vapid.key"), [0_u8; 138]).unwrap();
        std::fs::write(hub_dir.join("push-devices.json"), "{\"devices\":[]}").unwrap();
        let engine = new_engine(dir.path());

        let id = engine
            .checkpoint_config_kind_now(
                RepoKind::Hub,
                &ctx(CheckpointTrigger::PreConfigWrite, "test"),
            )
            .unwrap()
            .unwrap();
        let detail = engine.show_checkpoint(RepoKind::Hub, id).await.unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["config.toml"],
            "a restore must never roll back the push key or the registered devices"
        );
    }

    #[tokio::test]
    async fn agent_config_repo_tracks_exactly_config_and_providers_toml() {
        let dir = tempfile::tempdir().unwrap();
        let agent_config_dir = dir.path().join("agent-config");
        std::fs::create_dir_all(&agent_config_dir).unwrap();
        std::fs::write(agent_config_dir.join("config.toml"), "temperature = 0.5").unwrap();
        std::fs::write(agent_config_dir.join("providers.toml"), "[models]").unwrap();
        let engine = new_engine(dir.path());

        let id = engine
            .checkpoint_config_kind_now(
                RepoKind::AgentConfig,
                &ctx(CheckpointTrigger::PreConfigWrite, "test"),
            )
            .unwrap()
            .unwrap();
        let detail = engine
            .show_checkpoint(RepoKind::AgentConfig, id)
            .await
            .unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"config.toml"));
        assert!(paths.contains(&"providers.toml"));
    }

    #[tokio::test]
    async fn workspace_repo_excludes_the_agents_own_config_and_providers_toml() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(workspace.join("config")).unwrap();
        std::fs::write(workspace.join("SOUL.md"), "hello").unwrap();
        std::fs::write(workspace.join("config").join("config.toml"), "x = 1").unwrap();
        std::fs::write(workspace.join("config").join("providers.toml"), "x = 1").unwrap();
        std::fs::write(workspace.join("config").join("mcp.json"), "{}").unwrap();
        let engine = new_engine(dir.path());

        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::PreAction, "test"))
            .await;
        let page = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        let detail = engine
            .show_checkpoint(RepoKind::Workspace, page.items.first().unwrap().id.clone())
            .await
            .unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert!(paths.contains(&"SOUL.md"));
        assert!(paths.contains(&"config/mcp.json"));
        assert!(
            !paths.contains(&"config/config.toml"),
            "the agent's own config.toml belongs to the AgentConfig repo, not the workspace repo: {paths:?}"
        );
        assert!(
            !paths.contains(&"config/providers.toml"),
            "the agent's own providers.toml belongs to the AgentConfig repo, not the workspace repo: {paths:?}"
        );
    }

    #[tokio::test]
    async fn restore_path_writes_back_checkpointed_content() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        std::fs::write(workspace.join("notes.md"), "version one").unwrap();
        engine
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::TurnEnd, "v1"))
            .await;
        let id = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap()
            .items
            .first()
            .unwrap()
            .id
            .clone();

        std::fs::write(workspace.join("notes.md"), "version two, overwritten").unwrap();
        let outcome = engine
            .restore_path(
                RepoKind::Workspace,
                id,
                "notes.md".to_string(),
                ctx(CheckpointTrigger::Restore, "restore notes.md"),
                &residuum_workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();
        assert_eq!(outcome.restored_paths, vec!["notes.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
            "version one"
        );
    }

    #[tokio::test]
    async fn restore_tree_brings_back_a_deleted_root_and_keeps_extra_entries() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(workspace.join("memory")).unwrap();
        std::fs::write(workspace.join("SOUL.md"), "soul").unwrap();
        std::fs::write(workspace.join("memory").join("notes.md"), "notes").unwrap();
        let engine = new_engine(dir.path());
        let id = engine
            .checkpoint_workspace_id_before_action(ctx(CheckpointTrigger::PreAction, "before"))
            .await
            .unwrap();
        std::fs::remove_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("newer.md"), "kept").unwrap();

        let outcome = engine
            .restore_tree(
                RepoKind::Workspace,
                id,
                ctx(CheckpointTrigger::Restore, "restore tree"),
                &TeamWriter::User,
            )
            .await
            .unwrap();

        assert!(outcome.restored_paths.contains(&"SOUL.md".to_string()));
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md")).unwrap(),
            "soul"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("memory").join("notes.md")).unwrap(),
            "notes"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("newer.md")).unwrap(),
            "kept"
        );
    }

    fn write_team_file(team_root: &Path, rel: &str, content: &str) {
        let path = rel
            .split('/')
            .fold(team_root.to_path_buf(), |acc, part| acc.join(part));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn team_repo_snapshots_the_team_dir_beside_the_hub_and_lists_it() {
        let dir = tempfile::tempdir().unwrap();
        let team_root = dir.path().join("team");
        write_team_file(&team_root, "USER.md", "Bear");
        write_team_file(&team_root, "wiki/index.md", "# Wiki");
        let engine = new_engine(dir.path());

        engine
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "team"))
            .await;

        let page = engine
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let detail = engine
            .show_checkpoint(RepoKind::Team, page.items.first().unwrap().id.clone())
            .await
            .unwrap();
        let mut paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(paths, ["USER.md", "wiki/index.md"]);
        let workspace_page = engine
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        assert!(workspace_page.items.is_empty(), "repos are independent");
    }

    #[tokio::test]
    async fn agents_get_their_own_repos_and_share_the_team_and_hub_repos() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoints_dir = dir.path().join("hub").join("checkpoints");
        let shared = SharedCheckpointRepos::open(
            &dir.path().join("hub"),
            &TeamPaths::new(dir.path().join("team")),
            &checkpoints_dir,
        )
        .unwrap();
        let engine_for = |name: &str| {
            let agent_dir = dir.path().join(name);
            std::fs::create_dir_all(&agent_dir).unwrap();
            CheckpointEngine::with_shared_repos(
                Arc::clone(&shared),
                name,
                agent_dir.clone(),
                agent_dir.join("config"),
                &checkpoints_dir,
                None,
            )
            .unwrap()
        };
        let scout = engine_for("scout");
        let atlas = engine_for("atlas");
        write_team_file(&dir.path().join("team"), "USER.md", "Bear");
        std::fs::write(dir.path().join("scout").join("SOUL.md"), "scout soul").unwrap();

        scout
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::PreAction, "scout"))
            .await;
        scout
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "scout team"))
            .await;
        atlas
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "atlas team"))
            .await;

        let scout_workspace = scout
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        let atlas_workspace = atlas
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(scout_workspace.items.len(), 1);
        assert!(
            atlas_workspace.items.is_empty(),
            "another agent's workspace history is not shared"
        );
        assert!(
            checkpoints_dir
                .join("agents")
                .join("scout")
                .join("workspace.git")
                .is_dir()
        );
        assert!(
            checkpoints_dir
                .join("agents")
                .join("atlas")
                .join("agent-config.git")
                .is_dir()
        );
        let via_atlas = atlas
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap();
        let via_scout = scout
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(via_atlas.items.len(), 1);
        assert_eq!(
            via_atlas.items.first().map(|c| &c.id),
            via_scout.items.first().map(|c| &c.id)
        );
    }

    #[tokio::test]
    async fn an_agents_history_survives_its_directory_and_engine() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoints_dir = dir.path().join("hub").join("checkpoints");
        let agent_dir = dir.path().join("scout");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "scout soul").unwrap();
        let open = || {
            CheckpointEngine::new(
                "scout",
                agent_dir.clone(),
                &TeamPaths::new(dir.path().join("team")),
                agent_dir.join("config"),
                dir.path().join("hub"),
                &checkpoints_dir,
                None,
            )
            .unwrap()
        };
        open()
            .checkpoint_workspace_before_action(ctx(CheckpointTrigger::PreAction, "before delete"))
            .await;
        std::fs::remove_dir_all(&agent_dir).unwrap();

        let rebuilt = open();
        let page = rebuilt
            .list_checkpoints(RepoKind::Workspace, None, None, None, None)
            .await
            .unwrap();
        let detail = rebuilt
            .show_checkpoint(RepoKind::Workspace, page.items.first().unwrap().id.clone())
            .await
            .unwrap();
        assert!(detail.changed_paths.iter().any(|c| c.path == "SOUL.md"));
    }

    #[tokio::test]
    async fn team_repo_excludes_the_search_index_vector_store_and_temps() {
        let dir = tempfile::tempdir().unwrap();
        let team_root = dir.path().join("team");
        write_team_file(&team_root, "wiki/page.md", "kept");
        write_team_file(&team_root, ".index/segments/a.bin", "index");
        write_team_file(&team_root, "vectors.db", "vectors");
        write_team_file(&team_root, "vectors.db-wal", "wal");
        write_team_file(&team_root, "wiki/.page.md.0badf00d.residuum-tmp", "temp");
        let engine = new_engine(dir.path());

        engine
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "team"))
            .await;

        let id = engine
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap()
            .items
            .first()
            .unwrap()
            .id
            .clone();
        let detail = engine.show_checkpoint(RepoKind::Team, id).await.unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(paths, ["wiki/page.md"]);
    }

    #[tokio::test]
    async fn team_restore_writes_back_from_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let team_root = dir.path().join("team");
        write_team_file(&team_root, "wiki/log.md", "v1");
        let engine = new_engine(dir.path());
        engine
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "v1"))
            .await;
        let id = engine
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap()
            .items
            .first()
            .unwrap()
            .id
            .clone();
        write_team_file(&team_root, "wiki/log.md", "v2 clobbered");

        let outcome = engine
            .restore_path(
                RepoKind::Team,
                id,
                "wiki/log.md".to_string(),
                ctx(CheckpointTrigger::Restore, "restore log"),
                &residuum_workspace::team_files::TeamWriter::User,
            )
            .await
            .unwrap();

        assert_eq!(outcome.restored_paths, vec!["wiki/log.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(team_root.join("wiki").join("log.md")).unwrap(),
            "v1"
        );
        assert_eq!(
            outcome.checkpoint_id,
            engine
                .list_checkpoints(RepoKind::Team, None, None, None, None)
                .await
                .unwrap()
                .items
                .first()
                .unwrap()
                .id,
            "the restore reports the team repository tip"
        );
    }

    #[tokio::test]
    async fn turn_hooks_checkpoint_the_team_dir_too() {
        let dir = tempfile::tempdir().unwrap();
        let team_root = dir.path().join("team");
        write_team_file(&team_root, "wiki/index.md", "edited by a teammate");
        std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
        let engine = new_engine(dir.path());

        engine.spawn_turn_end_checkpoint(ctx(CheckpointTrigger::TurnEnd, "turn"));

        let items = wait_for_checkpoint_count(&engine, RepoKind::Team, 1).await;
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn team_repo_is_not_a_config_repo() {
        let dir = tempfile::tempdir().unwrap();
        let engine = new_engine(dir.path());
        assert!(matches!(
            engine.checkpoint_config_kind_now(
                RepoKind::Team,
                &ctx(CheckpointTrigger::PreConfigWrite, "x")
            ),
            Err(CheckpointError::NotAConfigRepo)
        ));
    }

    #[tokio::test]
    async fn engine_and_coordinator_locate_team_through_the_layout() {
        use residuum_config::WorkspaceLayout;
        use residuum_workspace::team_files::TeamWriteCoordinator;

        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("scout"));
        let coordinator = TeamWriteCoordinator::new(layout.team());
        assert_eq!(coordinator.root(), layout.team().root());

        let hub_dir = dir.path().join("elsewhere").join("deeper").join("hub");
        let engine = CheckpointEngine::new(
            "test-agent",
            layout.root().to_path_buf(),
            layout.team(),
            dir.path().join("agent-config"),
            hub_dir,
            &dir.path().join("checkpoints"),
            None,
        )
        .unwrap()
        .with_team_coordinator(coordinator);
        write_team_file(layout.team().root(), "USER.md", "Bear");

        engine
            .checkpoint_team_before_action(ctx(CheckpointTrigger::PreAction, "team"))
            .await;

        let page = engine
            .list_checkpoints(RepoKind::Team, None, None, None, None)
            .await
            .unwrap();
        let first = page.items.first().expect("a team checkpoint");
        let detail = engine
            .show_checkpoint(RepoKind::Team, first.id.clone())
            .await
            .unwrap();
        let paths: Vec<&str> = detail
            .changed_paths
            .iter()
            .map(|c| c.path.as_str())
            .collect();
        assert_eq!(paths, ["USER.md"]);
    }

    #[tokio::test]
    async fn team_restore_tree_records_the_writer_of_each_restored_file() {
        use residuum_workspace::team_files::{CheckError, TeamWriteCoordinator, TeamWriter};

        let dir = tempfile::tempdir().unwrap();
        let team = residuum_config::paths::TeamPaths::new(dir.path().join("team"));
        write_team_file(team.root(), "wiki/agents/scout.md", "role");
        let coordinator = TeamWriteCoordinator::new(&team);
        let engine = new_engine(dir.path()).with_team_coordinator(coordinator.clone());
        let id = engine
            .checkpoint_team_id_before_action(ctx(CheckpointTrigger::PreAction, "team"))
            .await
            .unwrap();
        let page = team.root().join("wiki").join("agents").join("scout.md");
        std::fs::remove_file(&page).unwrap();
        let seen = coordinator.stamp(&page).await.unwrap();
        let creator = TeamWriter::Agent("creator".to_string());

        engine
            .restore_tree(
                RepoKind::Team,
                id,
                ctx(CheckpointTrigger::Restore, "restore team"),
                &creator,
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&page).unwrap(), "role");
        let Err(CheckError::Conflict(conflict)) =
            coordinator.lock(&page).await.check(Some(&seen)).await
        else {
            panic!("a reader who saw the file missing should see a conflict");
        };
        assert_eq!(conflict.changed_by, Some(creator));
    }
}

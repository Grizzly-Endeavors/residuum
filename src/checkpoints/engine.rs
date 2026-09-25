//! Public engine API: turn hooks, pre-action/pre-write hooks, and the
//! tier-1 (list/show/diff/stats) and tier-2 (restore/undo) operations.
//!
//! Every hook that runs on an agent turn or a destructive action never
//! blocks or fails its caller — failures are logged and surface as a
//! notice, then the caller proceeds regardless (see
//! `docs/systems-usage/checkpoints.md`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;

use crate::bus::Publisher;

use super::backend::{GitRepo, SnapshotFile};
use super::types::{
    ChangedPath, CheckpointContext, CheckpointDetail, CheckpointError, CheckpointPage,
    CheckpointSummary, RepoKind, RepoStats, RestoreOutcome, UndoOutcome,
};
use super::{exclude, notice};

/// Root config files and encrypted key stores tracked by the config
/// repository. `secrets.key`/`agent-keys.key` (the machine keys the `.enc`
/// files are decrypted with) are deliberately never in this list — see
/// `docs/systems-usage/checkpoints.md`.
const CONFIG_TRACKED_FILES: &[&str] = &[
    "config.toml",
    "providers.toml",
    "secrets.toml.enc",
    "agent-keys.toml.enc",
    "a2a-keys.toml",
];

/// Default page size for `list_checkpoints` when the caller doesn't specify
/// one.
const DEFAULT_PAGE_LIMIT: usize = 50;

/// The workspace and config checkpoint repositories, and everything needed
/// to take, list, and act on checkpoints in either.
pub struct CheckpointEngine {
    workspace_root: PathBuf,
    workspace_repo: Arc<Mutex<GitRepo>>,
    workspace_git_dir: PathBuf,
    config_dir: PathBuf,
    config_repo: Arc<Mutex<GitRepo>>,
    config_git_dir: PathBuf,
    publisher: Option<Publisher>,
}

impl CheckpointEngine {
    /// Open (or create) both checkpoint repositories.
    ///
    /// `checkpoints_dir` is typically `~/.residuum/checkpoints`; the
    /// workspace repository's git-dir is `checkpoints_dir/workspace.git`
    /// and the config repository's is `checkpoints_dir/config.git`. Neither
    /// git-dir lives inside `workspace_root`, so a `.git` the user keeps
    /// there is never touched.
    ///
    /// # Errors
    /// Returns [`CheckpointError`] if either repository can't be opened or
    /// initialized.
    pub fn new(
        workspace_root: PathBuf,
        config_dir: PathBuf,
        checkpoints_dir: &Path,
        publisher: Option<Publisher>,
    ) -> Result<Self, CheckpointError> {
        let workspace_git_dir = checkpoints_dir.join(RepoKind::Workspace.dir_name());
        let config_git_dir = checkpoints_dir.join(RepoKind::Config.dir_name());
        let workspace_repo = GitRepo::open_or_init(&workspace_git_dir)?;
        let config_repo = GitRepo::open_or_init(&config_git_dir)?;
        Ok(Self {
            workspace_root,
            workspace_repo: Arc::new(Mutex::new(workspace_repo)),
            workspace_git_dir,
            config_dir,
            config_repo: Arc::new(Mutex::new(config_repo)),
            config_git_dir,
            publisher,
        })
    }

    fn repo(&self, kind: RepoKind) -> Arc<Mutex<GitRepo>> {
        match kind {
            RepoKind::Workspace => Arc::clone(&self.workspace_repo),
            RepoKind::Config => Arc::clone(&self.config_repo),
        }
    }

    fn dest_root(&self, kind: RepoKind) -> PathBuf {
        match kind {
            RepoKind::Workspace => self.workspace_root.clone(),
            RepoKind::Config => self.config_dir.clone(),
        }
    }

    fn git_dir(&self, kind: RepoKind) -> PathBuf {
        match kind {
            RepoKind::Workspace => self.workspace_git_dir.clone(),
            RepoKind::Config => self.config_git_dir.clone(),
        }
    }

    // ─── Automatic checkpoints (never block or fail the caller) ────────

    /// Turn-start hook: if the workspace tree changed since the last
    /// checkpoint (an edit made outside Residuum), record it. Runs off the
    /// hot path — this returns immediately, before the checkpoint is
    /// necessarily done.
    pub fn spawn_turn_start_checkpoint(&self, ctx: CheckpointContext) {
        self.spawn_workspace_checkpoint(ctx);
    }

    /// Turn-end hook: record what the turn changed. Runs off the hot path.
    pub fn spawn_turn_end_checkpoint(&self, ctx: CheckpointContext) {
        self.spawn_workspace_checkpoint(ctx);
    }

    fn spawn_workspace_checkpoint(&self, ctx: CheckpointContext) {
        let repo = Arc::clone(&self.workspace_repo);
        let root = self.workspace_root.clone();
        let publisher = self.publisher.clone();
        tokio::spawn(async move {
            let task_ctx = ctx.clone();
            let result =
                tokio::task::spawn_blocking(move || commit_workspace(&repo, &root, &task_ctx))
                    .await
                    .unwrap_or_else(|e| {
                        Err(CheckpointError::Git(format!(
                            "checkpoint task panicked: {e}"
                        )))
                    });
            report_outcome(publisher.as_ref(), "workspace", &ctx, &result).await;
        });
    }

    /// Before a destructive workspace API action (delete, overwrite,
    /// move/rename with overwrite, workbench artifact delete): checkpoint
    /// the current workspace state first. Awaited so the checkpoint
    /// happens-before the action, but never fails or blocks it — any
    /// error is logged and notified, then this returns regardless.
    pub async fn checkpoint_workspace_before_action(&self, ctx: CheckpointContext) {
        let repo = Arc::clone(&self.workspace_repo);
        let root = self.workspace_root.clone();
        let task_ctx = ctx.clone();
        let result = tokio::task::spawn_blocking(move || commit_workspace(&repo, &root, &task_ctx))
            .await
            .unwrap_or_else(|e| {
                Err(CheckpointError::Git(format!(
                    "checkpoint task panicked: {e}"
                )))
            });
        report_outcome(self.publisher.as_ref(), "workspace", &ctx, &result).await;
    }

    /// Checkpoint the config repository now (root config files and
    /// encrypted key stores). Synchronous and safe to call from inside a
    /// blocking context (e.g. a `spawn_blocking` closure already holding a
    /// store's own write lock) — this does no `.await`ing of its own.
    ///
    /// # Errors
    /// Returns [`CheckpointError`] on failure. Callers on the hot path
    /// should not propagate it — see [`Self::checkpoint_config_before_write`]
    /// and [`Self::report_config_outcome`] for the "never fail the caller"
    /// wrapping.
    pub fn checkpoint_config_now(
        &self,
        ctx: &CheckpointContext,
    ) -> Result<Option<String>, CheckpointError> {
        let files = collect_config_files(&self.config_dir);
        let guard = self
            .config_repo
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.commit_snapshot(&files, Utc::now(), ctx)
    }

    /// Before a write to a root config file or an encrypted key store:
    /// checkpoint the config repository first. Never fails or blocks the
    /// write.
    pub async fn checkpoint_config_before_write(&self, ctx: CheckpointContext) {
        let result = self.checkpoint_config_now(&ctx);
        report_outcome(self.publisher.as_ref(), "config", &ctx, &result).await;
    }

    /// Report the outcome of a [`Self::checkpoint_config_now`] call made
    /// from inside a blocking closure this engine doesn't own (agent-key
    /// and A2A-key storage checkpoint from inside their own
    /// `spawn_blocking` write path — see `src/agent_keys/runtime.rs` and
    /// `src/a2a/keys_runtime.rs`). Call this after returning to async code.
    pub async fn report_config_outcome(
        &self,
        ctx: &CheckpointContext,
        result: &Result<Option<String>, CheckpointError>,
    ) {
        report_outcome(self.publisher.as_ref(), "config", ctx, result).await;
    }

    // ─── Tier 1: visibility ──────────────────────────────────────────

    /// List checkpoints in `kind`, newest first, optionally filtered to
    /// those that changed `path_filter` (a file, or a directory prefix).
    ///
    /// # Errors
    /// Returns [`CheckpointError::InvalidCursor`] if `before` isn't a
    /// checkpoint id this method previously returned.
    pub async fn list_checkpoints(
        &self,
        kind: RepoKind,
        path_filter: Option<String>,
        before: Option<String>,
        limit: Option<usize>,
    ) -> Result<CheckpointPage, CheckpointError> {
        let repo = self.repo(kind);
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).max(1);
        tokio::task::spawn_blocking(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let before_id = before
                .map(|id| guard.resolve_commit(&id))
                .transpose()
                .map_err(|e| {
                    tracing::debug!(error = %e, "checkpoint list cursor did not resolve");
                    CheckpointError::InvalidCursor
                })?;
            let (rows, next) = guard.log(before_id, limit, path_filter.as_deref())?;
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

    /// A checkpoint's own metadata plus the paths it changed.
    ///
    /// # Errors
    /// Returns [`CheckpointError::NotFound`] if `id` doesn't name a
    /// checkpoint in `kind`.
    pub async fn show_checkpoint(
        &self,
        kind: RepoKind,
        id: String,
    ) -> Result<CheckpointDetail, CheckpointError> {
        let repo = self.repo(kind);
        tokio::task::spawn_blocking(move || {
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
        tokio::task::spawn_blocking(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            guard.file_diff(oid, &path)
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("diff task panicked: {e}"))))
    }

    /// A file's content at checkpoint `id`. `None` if `path` doesn't exist
    /// there or names a directory.
    ///
    /// # Errors
    /// Returns [`CheckpointError::NotFound`] if `id` doesn't name a
    /// checkpoint in `kind`.
    pub async fn file_content_at(
        &self,
        kind: RepoKind,
        id: String,
        path: String,
    ) -> Result<Option<Vec<u8>>, CheckpointError> {
        let repo = self.repo(kind);
        tokio::task::spawn_blocking(move || {
            let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
            let oid = guard.resolve_commit(&id)?;
            guard.file_content_at(oid, &path)
        })
        .await
        .unwrap_or_else(|e| Err(CheckpointError::Git(format!("read task panicked: {e}"))))
    }

    /// On-disk size, checkpoint count, and oldest checkpoint for `kind`.
    ///
    /// # Errors
    /// Returns [`CheckpointError`] if the repository can't be read.
    pub async fn stats(&self, kind: RepoKind) -> Result<RepoStats, CheckpointError> {
        let repo = self.repo(kind);
        let git_dir = self.git_dir(kind);
        tokio::task::spawn_blocking(move || {
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

    // ─── Tier 2: restore / undo ──────────────────────────────────────

    /// Restore `path` (a file or a directory) to its content at checkpoint
    /// `id`, then checkpoint the result so the restore itself can be
    /// undone. Publishes a notice describing what was restored.
    ///
    /// # Errors
    /// Returns [`CheckpointError::NotFound`] if `id` doesn't name a
    /// checkpoint, or [`CheckpointError::PathNotFound`] if `path` isn't
    /// present there.
    pub async fn restore_path(
        &self,
        kind: RepoKind,
        id: String,
        path: String,
        ctx: CheckpointContext,
    ) -> Result<RestoreOutcome, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let restored_paths = {
            let repo = Arc::clone(&repo);
            let id = id.clone();
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
                let oid = guard.resolve_commit(&id)?;
                guard.restore_paths(oid, &path, &dest_root)
            })
            .await
            .unwrap_or_else(|e| Err(CheckpointError::Git(format!("restore task panicked: {e}"))))?
        };

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

    /// Undo a checkpoint's own changes: revert every path it changed back
    /// to its content just before it, skipping any path that changed again
    /// since (by a later checkpoint or the user) so it isn't clobbered.
    /// Checkpoints the result, so an undo can itself be undone. Publishes
    /// a notice naming what was reverted and what was skipped.
    ///
    /// # Errors
    /// Returns [`CheckpointError::NotFound`] if `id` doesn't name a
    /// checkpoint in `kind`.
    pub async fn undo_checkpoint(
        &self,
        kind: RepoKind,
        id: String,
        ctx: CheckpointContext,
    ) -> Result<UndoOutcome, CheckpointError> {
        let repo = self.repo(kind);
        let dest_root = self.dest_root(kind);
        let (reverted_paths, skipped_paths) = {
            let repo = Arc::clone(&repo);
            let id = id.clone();
            tokio::task::spawn_blocking(move || undo_checkpoint_paths(&repo, &id, &dest_root))
                .await
                .unwrap_or_else(|e| Err(CheckpointError::Git(format!("undo task panicked: {e}"))))?
        };

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

    /// Checkpoint `kind` right after a restore/undo mutated it, so the
    /// mutation itself is recorded. Reported the same way any automatic
    /// checkpoint is (logged + notice); a failure here never turns an
    /// already-completed restore/undo into an error — the caller gets back
    /// the repository's current tip either way.
    async fn checkpoint_after_mutation(&self, kind: RepoKind, ctx: CheckpointContext) -> String {
        let result = match kind {
            RepoKind::Workspace => {
                let repo = Arc::clone(&self.workspace_repo);
                let root = self.workspace_root.clone();
                let task_ctx = ctx.clone();
                tokio::task::spawn_blocking(move || commit_workspace(&repo, &root, &task_ctx))
                    .await
                    .unwrap_or_else(|e| {
                        Err(CheckpointError::Git(format!(
                            "checkpoint task panicked: {e}"
                        )))
                    })
            }
            RepoKind::Config => self.checkpoint_config_now(&ctx),
        };
        report_outcome(self.publisher.as_ref(), kind.dir_name(), &ctx, &result).await;
        if let Ok(Some(id)) = &result {
            return id.clone();
        }
        current_tip(&self.repo(kind))
    }
}

/// The repository's current tip id as a hex string, or empty if it has no
/// checkpoints (should not happen here — a restore/undo only runs after at
/// least one checkpoint already exists).
fn current_tip(repo: &Mutex<GitRepo>) -> String {
    repo.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .tip()
        .ok()
        .flatten()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_default()
}

/// Revert every path checkpoint `id` changed, skipping any whose current
/// on-disk content no longer matches what that checkpoint recorded.
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

/// Whether the on-disk content at `change.path` differs from what
/// checkpoint `id` recorded for it — meaning something touched it again
/// after that checkpoint, so undoing would clobber that later edit.
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

/// Revert `change.path` to its content at `parent` (deleting it if it
/// didn't exist there, restoring it otherwise). `parent` is `None` when the
/// checkpoint being undone was the repository's first.
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

/// Build a checkpoint's file list by walking `root`, skipping anything
/// [`exclude::is_excluded`] flags, and following no symlinks.
fn collect_workspace_files(root: &Path) -> Vec<SnapshotFile> {
    let mut files = Vec::new();
    walk_dir(root, root, &mut files);
    files
}

fn walk_dir(root: &Path, dir: &Path, out: &mut Vec<SnapshotFile>) {
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
        if exclude::is_excluded(&rel_str) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_dir(root, &abs_path, out);
        } else if file_type.is_file() {
            out.push(SnapshotFile {
                rel_path: rel_str,
                executable: is_executable(&abs_path),
                abs_path,
            });
        }
    }
}

/// Convert a filesystem-relative path into a `/`-separated string. `None`
/// if any component isn't valid UTF-8.
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

/// Build the config repository's file list: every tracked file that
/// currently exists.
fn collect_config_files(config_dir: &Path) -> Vec<SnapshotFile> {
    CONFIG_TRACKED_FILES
        .iter()
        .filter_map(|name| {
            let abs_path = config_dir.join(name);
            abs_path.is_file().then(|| SnapshotFile {
                rel_path: (*name).to_string(),
                abs_path,
                executable: false,
            })
        })
        .collect()
}

/// Total size on disk of every file under `dir`, symlinks not followed.
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

/// Snapshot the workspace and commit if it changed. Free function (rather
/// than a method) so it can be called from inside `spawn_blocking` without
/// capturing `&CheckpointEngine` across the blocking boundary.
fn commit_workspace(
    repo: &Mutex<GitRepo>,
    root: &Path,
    ctx: &CheckpointContext,
) -> Result<Option<String>, CheckpointError> {
    let files = collect_workspace_files(root);
    let guard = repo.lock().unwrap_or_else(PoisonError::into_inner);
    guard.commit_snapshot(&files, Utc::now(), ctx)
}

/// Log and, on failure, publish a notice describing what happened. Called
/// after every automatic checkpoint attempt so a failure is never silent
/// but never blocks or fails whatever triggered the checkpoint either.
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

/// First 12 hex characters of a checkpoint id, for a human-readable notice.
fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::types::CheckpointTrigger;
    use super::*;

    fn ctx(trigger: CheckpointTrigger, summary: &str) -> CheckpointContext {
        CheckpointContext::system(trigger, summary)
    }

    fn new_engine(dir: &Path) -> CheckpointEngine {
        CheckpointEngine::new(
            dir.join("workspace"),
            dir.join("config"),
            &dir.join("checkpoints"),
            None,
        )
        .unwrap()
    }

    /// Poll `list_checkpoints` until at least `count` checkpoints appear,
    /// for asserting on a fire-and-forget `spawn_*_checkpoint` call.
    async fn wait_for_checkpoint_count(
        engine: &CheckpointEngine,
        kind: RepoKind,
        count: usize,
    ) -> Vec<CheckpointSummary> {
        for _ in 0..200 {
            let page = engine
                .list_checkpoints(kind, None, None, Some(count))
                .await
                .unwrap();
            if page.items.len() >= count {
                return page.items;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {count} checkpoint(s) within the timeout");
    }

    #[tokio::test]
    async fn turn_start_checkpoint_attributes_an_outside_edit() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let engine = new_engine(dir.path());

        // An edit made outside Residuum, before any turn runs.
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
        // Newest first: the turn-end checkpoint comes before turn-start.
        let newest = items.first().expect("two items");
        let oldest = items.get(1).expect("two items");
        assert_eq!(newest.trigger, CheckpointTrigger::TurnEnd);
        assert_eq!(oldest.trigger, CheckpointTrigger::TurnStart);
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
            workspace.clone(),
            dir.path().join("config"),
            &checkpoints_dir,
            None,
        )
        .unwrap();
        std::fs::write(workspace.join("a.txt"), "1").unwrap();

        // Make the workspace git dir's object store unwritable so the next
        // commit attempt fails with a real I/O error.
        let objects_dir = checkpoints_dir
            .join(RepoKind::Workspace.dir_name())
            .join("objects");
        std::fs::set_permissions(&objects_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            engine.checkpoint_workspace_before_action(ctx(
                CheckpointTrigger::PreAction,
                "delete a.txt",
            )),
        )
        .await;
        assert!(
            result.is_ok(),
            "checkpoint_workspace_before_action must never hang, even when the underlying commit fails"
        );

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
            .list_checkpoints(RepoKind::Workspace, None, None, None)
            .await
            .unwrap();
        let second_id = page.items.first().unwrap().id.clone();

        // A later edit — by the user or a subsequent turn — touches b.txt
        // after the checkpoint being undone.
        std::fs::write(workspace.join("b.txt"), "v3 (edited after the checkpoint)").unwrap();

        let outcome = engine
            .undo_checkpoint(
                RepoKind::Workspace,
                second_id,
                ctx(CheckpointTrigger::Undo, "undo second"),
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
            .list_checkpoints(RepoKind::Workspace, None, None, None)
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
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v1"
        );

        // Undo the undo: should restore the "v2" state the first undo reverted.
        let redo_outcome = engine
            .undo_checkpoint(
                RepoKind::Workspace,
                undo_outcome.checkpoint_id,
                ctx(CheckpointTrigger::Undo, "undo the undo"),
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), "timezone = \"UTC\"").unwrap();
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
            .join(RepoKind::Config.dir_name());
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
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), "timezone = \"UTC\"").unwrap();
        std::fs::write(config_dir.join("secrets.toml.enc"), b"ciphertext").unwrap();
        std::fs::write(config_dir.join("secrets.key"), [0_u8; 32]).unwrap();
        std::fs::write(config_dir.join("agent-keys.key"), [0_u8; 32]).unwrap();
        let engine = new_engine(dir.path());

        let id = engine
            .checkpoint_config_now(&ctx(CheckpointTrigger::PreConfigWrite, "test"))
            .unwrap()
            .unwrap();
        let detail = engine.show_checkpoint(RepoKind::Config, id).await.unwrap();
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
            .list_checkpoints(RepoKind::Workspace, None, None, None)
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
            )
            .await
            .unwrap();
        assert_eq!(outcome.restored_paths, vec!["notes.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(workspace.join("notes.md")).unwrap(),
            "version one"
        );
    }
}

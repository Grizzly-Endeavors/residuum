//! `gix` (gitoxide) wrapper: the only file in the checkpoints system that
//! knows about the git object model. Every other module works with plain
//! [`CheckpointSummary`]/[`ChangedPath`]/`String` ids, so swapping the git
//! library later only touches this file.
//!
//! Each checkpoint repository is a bare git directory (no work tree of its
//! own — the association between a git directory and the directory it
//! snapshots is data [`crate::checkpoints::engine`] carries, not something
//! `gix` is asked to understand). A single branch, `refs/heads/checkpoints`,
//! is the linear history of checkpoints; every commit's tree is rebuilt from
//! scratch from the currently-included files rather than patched onto the
//! parent, so a file that disappeared from the snapshot is simply never
//! re-added — no separate deletion bookkeeping needed.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use gix::bstr::BString;
use gix::objs::tree::EntryKind;

use super::types::{ChangeKind, ChangedPath, CheckpointError, CheckpointTrigger};

/// The one branch every checkpoint repository keeps its history on.
const REF_NAME: &str = "refs/heads/checkpoints";

/// Author/committer identity stamped on every checkpoint commit.
const COMMITTER_NAME: &str = "Residuum";
const COMMITTER_EMAIL: &str = "checkpoints@residuum.local";

/// Lock file, inside the git-dir, serializing a commit's read-tip ->
/// write-commit -> update-ref sequence across processes: the gateway and a
/// `residuum` CLI command may both want to commit to the same repository
/// at once.
const COMMIT_LOCK_FILE: &str = "checkpoint.lock";

/// How many times to poll for the cross-process commit lock before giving
/// up. Checkpointing must never block its caller for long, so this is a
/// short, bounded wait, not an indefinite one.
const LOCK_RETRY_ATTEMPTS: u32 = 40;
const LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

/// How many times to retry the write-commit -> update-ref step if the ref
/// changed underneath us. With the commit lock held this should never
/// actually happen -- it's a defensive second layer, not the primary
/// safety mechanism.
const REF_CAS_RETRY_ATTEMPTS: u32 = 3;

/// A file to include in a snapshot commit: its path relative to the
/// snapshotted directory (`/`-separated), its absolute path on disk, and
/// whether it should be recorded as executable.
pub(super) struct SnapshotFile {
    pub(super) rel_path: String,
    pub(super) abs_path: PathBuf,
    pub(super) executable: bool,
}

/// One page of [`GitRepo::log`]: commits (newest first) with their parsed
/// fields and changed-path counts, plus a cursor for the next page.
type LogPage = (
    Vec<(gix::ObjectId, CommitFields, usize)>,
    Option<gix::ObjectId>,
);

/// Metadata recorded on a checkpoint commit, parsed back out of its trailer.
pub(super) struct CommitFields {
    pub(super) address: String,
    pub(super) run_id: Option<String>,
    pub(super) turn_id: Option<String>,
    pub(super) trigger: CheckpointTrigger,
    pub(super) summary: String,
    pub(super) timestamp: DateTime<Utc>,
}

/// A bare checkpoint git repository.
pub(super) struct GitRepo {
    repo: gix::Repository,
}

fn git_err(e: impl std::fmt::Display) -> CheckpointError {
    CheckpointError::Git(e.to_string())
}

fn io_err(e: impl std::fmt::Display) -> CheckpointError {
    CheckpointError::Io(e.to_string())
}

impl GitRepo {
    /// Open the checkpoint repository at `git_dir`, initializing a fresh
    /// bare repository there if it doesn't exist yet.
    pub(super) fn open_or_init(git_dir: &Path) -> Result<Self, CheckpointError> {
        if git_dir.join("HEAD").is_file() {
            let repo = gix::open(git_dir).map_err(git_err)?;
            return Ok(Self { repo });
        }

        std::fs::create_dir_all(git_dir).map_err(io_err)?;
        let repo = gix::init_bare(git_dir).map_err(git_err)?;
        Ok(Self { repo })
    }

    /// The current tip of `refs/heads/checkpoints`, or `None` before the
    /// first checkpoint.
    pub(super) fn tip(&self) -> Result<Option<gix::ObjectId>, CheckpointError> {
        match self.repo.try_find_reference(REF_NAME).map_err(git_err)? {
            Some(mut reference) => Ok(reference.peel_to_id().ok().map(gix::Id::detach)),
            None => Ok(None),
        }
    }

    /// Resolve a full checkpoint id (as handed out by this module — never
    /// abbreviated) to an object id known to exist as a commit.
    pub(super) fn resolve_commit(&self, id: &str) -> Result<gix::ObjectId, CheckpointError> {
        let oid = gix::ObjectId::from_hex(id.as_bytes()).map_err(|e| {
            tracing::debug!(error = %e, id, "checkpoint id is not valid hex");
            CheckpointError::NotFound(id.to_string())
        })?;
        self.repo.find_commit(oid).map_err(|e| {
            tracing::debug!(error = %e, id, "checkpoint id does not name a commit");
            CheckpointError::NotFound(id.to_string())
        })?;
        Ok(oid)
    }

    /// The first parent of commit `id`, or `None` when it has none (the
    /// repository's first checkpoint).
    pub(super) fn parent_of(
        &self,
        id: gix::ObjectId,
    ) -> Result<Option<gix::ObjectId>, CheckpointError> {
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        Ok(commit.parent_ids().next().map(gix::Id::detach))
    }

    /// Build a tree from exactly `files` (starting from the empty tree, so a
    /// path not present in `files` is simply absent from the result) and, if
    /// it differs from the current tip's tree, commit it as a new
    /// checkpoint. Returns `None` when nothing changed.
    ///
    /// Serializes the read-tip -> write-commit -> update-ref sequence
    /// against every other process (a running gateway, a `residuum` CLI
    /// command, or another instance of either) that might commit to this
    /// same repository at the same moment, via a lock file in the git-dir.
    /// The ref update itself additionally requires the tip to still match
    /// what was just read (rather than writing unconditionally), and
    /// retries a few times on a mismatch, as a second, independent layer
    /// in case the lock ever doesn't cover a real race.
    pub(super) fn commit_snapshot(
        &self,
        files: &[SnapshotFile],
        author_time: DateTime<Utc>,
        ctx: &super::types::CheckpointContext,
    ) -> Result<Option<String>, CheckpointError> {
        let new_tree_id = self.build_tree(files)?;
        let _lock = self.acquire_commit_lock()?;

        let mut last_err = None;
        for _ in 0..REF_CAS_RETRY_ATTEMPTS {
            let parent = self.tip()?;
            let parent_tree_id = self.tree_id_of(parent)?;
            if new_tree_id == parent_tree_id {
                return Ok(None);
            }

            let message = build_message(ctx);
            match self.write_commit(new_tree_id, parent, author_time, &message) {
                Ok(commit_id) => return Ok(Some(commit_id.to_hex().to_string())),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            CheckpointError::Git("commit retry loop produced no result".to_string())
        }))
    }

    /// Acquire this repository's cross-process commit lock, polling briefly
    /// if another process currently holds it. Held by the caller for the
    /// duration of a commit attempt; released when the returned file is
    /// dropped.
    fn acquire_commit_lock(&self) -> Result<std::fs::File, CheckpointError> {
        let lock_path = self.repo.git_dir().join(COMMIT_LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(io_err)?;

        let mut attempt = 0;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::Error(e)) => return Err(io_err(e)),
                Err(std::fs::TryLockError::WouldBlock) => {
                    attempt += 1;
                    if attempt >= LOCK_RETRY_ATTEMPTS {
                        return Err(CheckpointError::Git(format!(
                            "timed out waiting for the checkpoint commit lock at {} (held by another process)",
                            lock_path.display()
                        )));
                    }
                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
            }
        }
    }

    /// Build a tree object from exactly `files`, starting from the empty
    /// tree.
    fn build_tree(&self, files: &[SnapshotFile]) -> Result<gix::ObjectId, CheckpointError> {
        let empty_tree_id = self.repo.empty_tree().id().detach();
        let mut editor = self.repo.edit_tree(empty_tree_id).map_err(git_err)?;
        for file in files {
            let data = std::fs::read(&file.abs_path).map_err(io_err)?;
            let blob_id = self.repo.write_blob(data).map_err(git_err)?.detach();
            let kind = if file.executable {
                EntryKind::BlobExecutable
            } else {
                EntryKind::Blob
            };
            editor
                .upsert(file.rel_path.as_str(), kind, blob_id)
                .map_err(git_err)?;
        }
        Ok(editor.write().map_err(git_err)?.detach())
    }

    /// The tree id of `commit`, or the empty tree when there is no commit
    /// yet.
    fn tree_id_of(&self, commit: Option<gix::ObjectId>) -> Result<gix::ObjectId, CheckpointError> {
        match commit {
            Some(id) => Ok(self
                .repo
                .find_commit(id)
                .map_err(git_err)?
                .tree_id()
                .map_err(git_err)?
                .detach()),
            None => Ok(self.repo.empty_tree().id().detach()),
        }
    }

    /// Write a commit object on top of `parent` (or as the repository's
    /// first commit when `parent` is `None`) and move `refs/heads/checkpoints`
    /// to it.
    fn write_commit(
        &self,
        tree: gix::ObjectId,
        parent: Option<gix::ObjectId>,
        author_time: DateTime<Utc>,
        message: &str,
    ) -> Result<gix::ObjectId, CheckpointError> {
        let signature = gix::actor::Signature {
            name: BString::from(COMMITTER_NAME),
            email: BString::from(COMMITTER_EMAIL),
            time: gix::date::Time::new(author_time.timestamp(), 0),
        };
        let parents: Vec<gix::ObjectId> = parent.into_iter().collect();
        let commit = gix::objs::Commit {
            tree,
            parents: parents.into(),
            author: signature.clone(),
            committer: signature,
            encoding: None,
            message: BString::from(message),
            extra_headers: vec![],
        };
        let commit_id = self.repo.write_object(commit).map_err(git_err)?.detach();
        // CAS on the ref: it must still be exactly what we read as `parent`
        // (or must not exist yet, for the repository's first commit). This
        // is what actually makes the retry loop in `commit_snapshot`
        // meaningful — with `PreviousValue::Any` here the write would
        // always succeed unconditionally and silently orphan a sibling
        // commit written by another process between our `tip()` read and
        // this write.
        let expected = match parent {
            Some(p) => gix::refs::transaction::PreviousValue::MustExistAndMatch(
                gix::refs::Target::Object(p),
            ),
            None => gix::refs::transaction::PreviousValue::MustNotExist,
        };
        self.repo
            .reference(REF_NAME, commit_id, expected, "checkpoint")
            .map_err(git_err)?;
        Ok(commit_id)
    }

    /// The trailer-parsed metadata for `id`.
    pub(super) fn commit_fields(&self, id: gix::ObjectId) -> Result<CommitFields, CheckpointError> {
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        let message = commit.message_raw().map_err(git_err)?.to_string();
        let commit_time = commit.time().map_err(git_err)?;
        let timestamp = Utc
            .timestamp_opt(commit_time.seconds, 0)
            .single()
            .unwrap_or_else(Utc::now);
        Ok(parse_message(&message, timestamp))
    }

    /// Changed paths between `id` and its first parent (or the empty tree,
    /// for the repository's first commit).
    pub(super) fn changed_paths(
        &self,
        id: gix::ObjectId,
    ) -> Result<Vec<ChangedPath>, CheckpointError> {
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        let new_tree = commit.tree().map_err(git_err)?;
        let old_tree = self.parent_tree(&commit)?;

        let changes = self
            .repo
            .diff_tree_to_tree(
                old_tree.as_ref(),
                Some(&new_tree),
                Some(gix::diff::Options::default()),
            )
            .map_err(git_err)?;

        Ok(changes.into_iter().filter_map(changed_path_from).collect())
    }

    /// Unified-diff text for `path` between `id` and its first parent.
    /// `None` when `path` didn't change.
    pub(super) fn file_diff(
        &self,
        id: gix::ObjectId,
        path: &str,
    ) -> Result<Option<String>, CheckpointError> {
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        let new_tree = commit.tree().map_err(git_err)?;
        let old_tree = self.parent_tree(&commit)?;

        let old_content = match &old_tree {
            Some(tree) => self.blob_at_path(tree, path)?,
            None => None,
        };
        let new_content = self.blob_at_path(&new_tree, path)?;

        Ok(render_unified_diff(
            path,
            old_content.as_deref(),
            new_content.as_deref(),
        ))
    }

    /// The parent commit's tree, or `None` when `commit` has no parent.
    fn parent_tree<'repo>(
        &'repo self,
        commit: &gix::Commit<'repo>,
    ) -> Result<Option<gix::Tree<'repo>>, CheckpointError> {
        match commit.parent_ids().next() {
            Some(parent_id) => {
                let parent_commit = self.repo.find_commit(parent_id).map_err(git_err)?;
                Ok(Some(parent_commit.tree().map_err(git_err)?))
            }
            None => Ok(None),
        }
    }

    /// A file's bytes at `path` inside `tree`, or `None` if `path` doesn't
    /// exist there or names a directory.
    fn blob_at_path(
        &self,
        tree: &gix::Tree<'_>,
        path: &str,
    ) -> Result<Option<Vec<u8>>, CheckpointError> {
        let Some(entry) = tree.lookup_entry(path.split('/')).map_err(git_err)? else {
            return Ok(None);
        };
        if entry.mode().is_tree() {
            return Ok(None);
        }
        Ok(Some(
            self.repo
                .find_blob(entry.object_id())
                .map_err(git_err)?
                .data
                .clone(),
        ))
    }

    /// File content at `path` at checkpoint `id`. `None` if `path` doesn't
    /// exist there or names a directory.
    pub(super) fn file_content_at(
        &self,
        id: gix::ObjectId,
        path: &str,
    ) -> Result<Option<Vec<u8>>, CheckpointError> {
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        let tree = commit.tree().map_err(git_err)?;
        self.blob_at_path(&tree, path)
    }

    /// Restore `path` (a file or a directory) from checkpoint `id` onto
    /// disk under `dest_root`, returning the workspace-relative paths that
    /// were written or removed. A directory restore makes the on-disk
    /// subtree match the checkpoint's subtree exactly: files present now
    /// but absent from the checkpoint at that path are removed.
    ///
    /// # Errors
    /// Returns [`CheckpointError::PathNotFound`] if `path` isn't present in
    /// the checkpoint's tree.
    pub(super) fn restore_paths(
        &self,
        id: gix::ObjectId,
        path: &str,
        dest_root: &Path,
    ) -> Result<Vec<String>, CheckpointError> {
        if !super::is_root_relative(path) {
            return Err(CheckpointError::PathNotFound(
                path.to_string(),
                id.to_hex().to_string(),
            ));
        }
        let commit = self.repo.find_commit(id).map_err(git_err)?;
        let tree = commit.tree().map_err(git_err)?;
        let Some(entry) = tree.lookup_entry(path.split('/')).map_err(git_err)? else {
            return Err(CheckpointError::PathNotFound(
                path.to_string(),
                id.to_hex().to_string(),
            ));
        };

        let mut written = Vec::new();
        if entry.mode().is_tree() {
            let dest_dir = dest_root.join(path);
            self.replace_directory(entry.object_id(), path, &dest_dir, &mut written)?;
        } else {
            self.write_blob_to_disk(
                entry.object_id(),
                entry.mode().is_executable(),
                &dest_root.join(path),
            )?;
            written.push(path.to_string());
        }
        Ok(written)
    }

    /// Make `dest_dir` match the tree at `tree_id` exactly: write every
    /// entry the tree has, and remove anything under `dest_dir` the tree
    /// doesn't have.
    fn replace_directory(
        &self,
        tree_id: gix::ObjectId,
        rel_prefix: &str,
        dest_dir: &Path,
        written: &mut Vec<String>,
    ) -> Result<(), CheckpointError> {
        let tree = self.repo.find_tree(tree_id).map_err(git_err)?;
        let decoded = tree.decode().map_err(git_err)?;
        let mut kept_names = std::collections::HashSet::new();

        for entry in &decoded.entries {
            let name = entry.filename.to_string();
            kept_names.insert(name.clone());
            let rel_path = format!("{rel_prefix}/{name}");
            let dest_path = dest_dir.join(&name);
            if entry.mode.is_tree() {
                self.replace_directory(entry.oid.to_owned(), &rel_path, &dest_path, written)?;
            } else {
                self.write_blob_to_disk(
                    entry.oid.to_owned(),
                    entry.mode.is_executable(),
                    &dest_path,
                )?;
                written.push(rel_path);
            }
        }

        remove_stale_entries(dest_dir, &kept_names).map_err(io_err)?;
        Ok(())
    }

    /// Write blob `id`'s content to `dest_path`, creating parent
    /// directories as needed and setting the executable bit on Unix.
    fn write_blob_to_disk(
        &self,
        id: gix::ObjectId,
        executable: bool,
        dest_path: &Path,
    ) -> Result<(), CheckpointError> {
        let data = self.repo.find_blob(id).map_err(git_err)?.data.clone();
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        std::fs::write(dest_path, &data).map_err(io_err)?;
        set_executable(dest_path, executable).map_err(io_err)?;
        Ok(())
    }

    /// Walk history from the tip backward, newest first, yielding parsed
    /// commit fields alongside each commit's id and how many paths it
    /// changed. `before` (a previously returned cursor) resumes exactly
    /// there; `limit` bounds how many are returned. When `path_filter` is
    /// set, only checkpoints that changed a path equal to it or nested
    /// under it as a directory count toward `limit` or appear in the
    /// result — every commit in history is still walked to find them, so a
    /// narrow filter over a long history costs proportionally more.
    pub(super) fn log(
        &self,
        before: Option<gix::ObjectId>,
        limit: usize,
        path_filter: Option<&str>,
    ) -> Result<LogPage, CheckpointError> {
        let Some(tip) = self.tip()? else {
            return Ok((Vec::new(), None));
        };

        // `before` is a cursor this method previously returned as the next
        // (not-yet-shown) checkpoint, so resuming starts exactly there
        // rather than skipping past it.
        let mut current = before.or(Some(tip));

        let mut items = Vec::new();
        let mut next_cursor = None;
        while let Some(id) = current {
            if items.len() >= limit {
                next_cursor = Some(id);
                break;
            }
            let changed = self.changed_paths(id)?;
            let matches = path_filter.is_none_or(|filter| {
                changed
                    .iter()
                    .any(|change| path_matches(&change.path, filter))
            });
            let commit = self.repo.find_commit(id).map_err(git_err)?;
            current = commit.parent_ids().next().map(gix::Id::detach);
            if matches {
                let fields = self.commit_fields(id)?;
                items.push((id, fields, changed.len()));
            }
        }
        Ok((items, next_cursor))
    }

    /// Total number of checkpoints, and when the oldest was taken.
    pub(super) fn stats(&self) -> Result<(u64, Option<DateTime<Utc>>), CheckpointError> {
        let Some(tip) = self.tip()? else {
            return Ok((0, None));
        };
        let mut count: u64 = 0;
        let mut oldest = None;
        let mut current = Some(tip);
        while let Some(id) = current {
            count += 1;
            let fields = self.commit_fields(id)?;
            oldest = Some(fields.timestamp);
            let commit = self.repo.find_commit(id).map_err(git_err)?;
            current = commit.parent_ids().next().map(gix::Id::detach);
        }
        Ok((count, oldest))
    }
}

/// Whether `changed_path` (a checkpoint's changed file, e.g.
/// `"wiki/index.md"`) is `filter` itself or nested under it as a directory.
fn path_matches(changed_path: &str, filter: &str) -> bool {
    changed_path == filter
        || changed_path
            .strip_prefix(filter)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Remove any file or directory directly under `dir` whose name isn't in
/// `keep`. No-op if `dir` doesn't exist (nothing to prune).
fn remove_stale_entries(
    dir: &Path,
    keep: &std::collections::HashSet<String>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if keep.contains(&name) {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "same signature as the Unix version, which can fail setting the mode"
)]
fn set_executable(_path: &Path, _executable: bool) -> std::io::Result<()> {
    // No executable bit to preserve outside Unix.
    Ok(())
}

fn changed_path_from(change: gix::object::tree::diff::ChangeDetached) -> Option<ChangedPath> {
    match change {
        gix::object::tree::diff::ChangeDetached::Addition {
            location,
            entry_mode,
            ..
        } => (!entry_mode.is_tree()).then(|| ChangedPath {
            path: location.to_string(),
            kind: ChangeKind::Added,
        }),
        gix::object::tree::diff::ChangeDetached::Deletion {
            location,
            entry_mode,
            ..
        } => (!entry_mode.is_tree()).then(|| ChangedPath {
            path: location.to_string(),
            kind: ChangeKind::Deleted,
        }),
        gix::object::tree::diff::ChangeDetached::Modification {
            location,
            entry_mode,
            ..
        } => (!entry_mode.is_tree()).then(|| ChangedPath {
            path: location.to_string(),
            kind: ChangeKind::Modified,
        }),
        // Rewrite tracking is never enabled (see `commit_snapshot`'s use of
        // `gix::diff::Options::default()`), so this never fires in
        // practice; treated as a modification at the new location so the
        // match stays exhaustive without silently dropping data if that
        // ever changes.
        gix::object::tree::diff::ChangeDetached::Rewrite {
            location,
            entry_mode,
            ..
        } => (!entry_mode.is_tree()).then(|| ChangedPath {
            path: location.to_string(),
            kind: ChangeKind::Modified,
        }),
    }
}

/// Render a unified diff between `old` and `new` file content. `None` when
/// both sides are absent or identical. Binary content on either side falls
/// back to a one-line placeholder rather than a byte-level diff.
fn render_unified_diff(path: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> Option<String> {
    if old == new {
        return None;
    }
    let old_text = old.map(content_as_text);
    let new_text = new.map(content_as_text);
    if matches!(old_text, Some(None)) || matches!(new_text, Some(None)) {
        return Some(format!("Binary file {path} differs\n"));
    }
    let old_str = old_text.flatten().unwrap_or_default();
    let new_str = new_text.flatten().unwrap_or_default();
    let diff = similar::TextDiff::from_lines(&old_str, &new_str);
    Some(
        diff.unified_diff()
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string(),
    )
}

/// Decode `data` as UTF-8 text, or `None` if it isn't (treated as binary).
fn content_as_text(data: &[u8]) -> Option<String> {
    std::str::from_utf8(data).ok().map(str::to_string)
}

/// Build a commit message: the summary, then a blank line, then trailers
/// carrying every field needed to reconstruct a [`super::types::CheckpointContext`].
fn build_message(ctx: &super::types::CheckpointContext) -> String {
    use std::fmt::Write as _;

    let mut message = String::new();
    if ctx.summary.is_empty() {
        message.push_str(ctx.trigger.label());
    } else {
        message.push_str(&ctx.summary);
    }
    message.push_str("\n\n");
    _ = writeln!(message, "Address: {}", ctx.address);
    if let Some(run_id) = &ctx.run_id {
        _ = writeln!(message, "Run-Id: {run_id}");
    }
    if let Some(turn_id) = &ctx.turn_id {
        _ = writeln!(message, "Turn-Id: {turn_id}");
    }
    _ = writeln!(message, "Trigger: {}", ctx.trigger.label());
    message
}

/// Parse a message [`build_message`] produced back into its fields. Never
/// fails: a message from an unrecognized shape (there shouldn't be any,
/// since this repository only ever holds commits this module wrote) is
/// treated as an untitled `turn_end` checkpoint by `system` rather than
/// erroring the whole listing over one row.
fn parse_message(message: &str, timestamp: DateTime<Utc>) -> CommitFields {
    let lines: Vec<&str> = message.lines().collect();

    let mut address = "system".to_string();
    let mut run_id = None;
    let mut turn_id = None;
    let mut trigger = CheckpointTrigger::TurnEnd;
    let mut trailer_start = lines.len();

    for (index, line) in lines.iter().enumerate().rev() {
        if let Some(value) = line.strip_prefix("Address: ") {
            address = value.to_string();
        } else if let Some(value) = line.strip_prefix("Run-Id: ") {
            run_id = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Turn-Id: ") {
            turn_id = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Trigger: ") {
            trigger = CheckpointTrigger::from_label(value).unwrap_or(CheckpointTrigger::TurnEnd);
        } else {
            break;
        }
        trailer_start = index;
    }

    while trailer_start > 0
        && lines
            .get(trailer_start - 1)
            .is_some_and(|l| l.trim().is_empty())
    {
        trailer_start -= 1;
    }

    let summary = lines.get(..trailer_start).unwrap_or_default().join("\n");
    CommitFields {
        address,
        run_id,
        turn_id,
        trigger,
        summary,
        timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::CheckpointContext;
    use super::*;

    fn ctx(trigger: CheckpointTrigger, summary: &str) -> CheckpointContext {
        CheckpointContext {
            address: "main".to_string(),
            run_id: None,
            turn_id: None,
            trigger,
            summary: summary.to_string(),
        }
    }

    fn write_files(dir: &Path, files: &[(&str, &str)]) -> Vec<SnapshotFile> {
        files
            .iter()
            .map(|(name, content)| {
                let abs_path = dir.join(name);
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&abs_path, content).unwrap();
                SnapshotFile {
                    rel_path: (*name).to_string(),
                    abs_path,
                    executable: false,
                }
            })
            .collect()
    }

    #[test]
    fn commit_message_round_trips() {
        let message = build_message(&CheckpointContext {
            address: "main".to_string(),
            run_id: Some("run-1".to_string()),
            turn_id: Some("turn-2".to_string()),
            trigger: CheckpointTrigger::TurnEnd,
            summary: "wrote SOUL.md".to_string(),
        });
        let fields = parse_message(&message, Utc::now());
        assert_eq!(fields.address, "main");
        assert_eq!(fields.run_id, Some("run-1".to_string()));
        assert_eq!(fields.turn_id, Some("turn-2".to_string()));
        assert_eq!(fields.trigger, CheckpointTrigger::TurnEnd);
        assert_eq!(fields.summary, "wrote SOUL.md");
    }

    #[test]
    fn commit_message_round_trips_without_optional_fields() {
        let message = build_message(&CheckpointContext {
            address: "system".to_string(),
            run_id: None,
            turn_id: None,
            trigger: CheckpointTrigger::PreConfigWrite,
            summary: String::new(),
        });
        let fields = parse_message(&message, Utc::now());
        assert_eq!(fields.address, "system");
        assert_eq!(fields.run_id, None);
        assert_eq!(fields.turn_id, None);
        assert_eq!(fields.trigger, CheckpointTrigger::PreConfigWrite);
        assert_eq!(fields.summary, "pre_config_write");
    }

    #[test]
    fn init_creates_a_usable_repo_with_no_checkpoints_yet() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();
        assert!(repo.tip().unwrap().is_none());
    }

    #[test]
    fn open_reuses_an_existing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("workspace.git");
        let repo = GitRepo::open_or_init(&git_dir).unwrap();
        let files = write_files(dir.path(), &[("a.txt", "hello")]);
        let id = repo
            .commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "first"),
            )
            .unwrap()
            .unwrap();

        let reopened = GitRepo::open_or_init(&git_dir).unwrap();
        assert_eq!(reopened.tip().unwrap().unwrap().to_hex().to_string(), id);
    }

    #[test]
    fn commit_snapshot_skips_when_nothing_changed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();
        let files = write_files(dir.path(), &[("a.txt", "hello")]);

        let first = repo
            .commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnStart, "outside edit"),
            )
            .unwrap();
        assert!(first.is_some(), "first snapshot should commit");

        let second = repo
            .commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "no changes"),
            )
            .unwrap();
        assert!(second.is_none(), "unchanged tree should not commit again");
    }

    #[test]
    fn concurrent_commits_from_independent_repo_handles_never_lose_one() {
        // Two independently-opened `GitRepo`s over the same git-dir, each
        // with its own in-process state, simulate the gateway and a
        // `residuum` CLI command committing to the same checkpoint
        // repository at once -- only the on-disk commit lock (never a
        // shared Rust `Mutex`) can serialize them.
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("workspace.git");
        let workspace_a = dir.path().join("a");
        let workspace_b = dir.path().join("b");
        std::fs::create_dir_all(&workspace_a).unwrap();
        std::fs::create_dir_all(&workspace_b).unwrap();

        let repo_a = GitRepo::open_or_init(&git_dir).unwrap();
        let repo_b = GitRepo::open_or_init(&git_dir).unwrap();

        let handle_a = std::thread::spawn(move || {
            let files = write_files(&workspace_a, &[("from-a.txt", "committed by a")]);
            repo_a.commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "from a"),
            )
        });
        let handle_b = std::thread::spawn(move || {
            let files = write_files(&workspace_b, &[("from-b.txt", "committed by b")]);
            repo_b.commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "from b"),
            )
        });

        let result_a = handle_a.join().unwrap();
        let result_b = handle_b.join().unwrap();
        assert!(
            result_a.is_ok(),
            "commit from handle a should not fail: {result_a:?}"
        );
        assert!(
            result_b.is_ok(),
            "commit from handle b should not fail: {result_b:?}"
        );
        assert!(
            result_a.unwrap().is_some() && result_b.unwrap().is_some(),
            "both concurrent commits should produce a checkpoint, not silently no-op"
        );

        // Neither commit lost: history has both, one linear chain (a
        // silently-overwritten ref would leave only one).
        let repo = GitRepo::open_or_init(&git_dir).unwrap();
        let (items, _) = repo.log(None, 10, None).unwrap();
        assert_eq!(
            items.len(),
            2,
            "both concurrent commits must be present in history, none lost to a ref race"
        );
        let summaries: Vec<&str> = items
            .iter()
            .map(|(_, fields, _)| fields.summary.as_str())
            .collect();
        assert!(summaries.contains(&"from a"));
        assert!(summaries.contains(&"from b"));
    }

    #[test]
    fn commit_snapshot_omits_files_deleted_from_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();

        let files = write_files(dir.path(), &[("a.txt", "hello"), ("b.txt", "world")]);
        let created = repo
            .commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "two files"),
            )
            .unwrap()
            .unwrap();
        let created_oid = repo.resolve_commit(&created).unwrap();
        let paths_after_first: Vec<String> = repo
            .changed_paths(created_oid)
            .unwrap()
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert!(paths_after_first.contains(&"a.txt".to_string()));
        assert!(paths_after_first.contains(&"b.txt".to_string()));

        // "delete" b.txt by omitting it from the next snapshot's file list.
        let remaining = write_files(dir.path(), &[("a.txt", "hello")]);
        let after_delete = repo
            .commit_snapshot(
                &remaining,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, "one file left"),
            )
            .unwrap()
            .unwrap();
        let after_delete_oid = repo.resolve_commit(&after_delete).unwrap();

        let changes = repo.changed_paths(after_delete_oid).unwrap();
        assert_eq!(changes.len(), 1);
        let deleted = changes.first().expect("exactly one change");
        assert_eq!(deleted.path, "b.txt");
        assert_eq!(deleted.kind, ChangeKind::Deleted);
        assert!(
            repo.file_content_at(after_delete_oid, "b.txt")
                .unwrap()
                .is_none(),
            "b.txt should be gone from the new checkpoint's tree"
        );
    }

    #[test]
    fn restore_path_writes_file_content_back() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("workspace.git");
        let worktree = dir.path().join("workspace");
        std::fs::create_dir_all(&worktree).unwrap();
        let repo = GitRepo::open_or_init(&git_dir).unwrap();

        let files = write_files(&worktree, &[("notes.md", "version one")]);
        let id = repo
            .commit_snapshot(&files, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "v1"))
            .unwrap()
            .unwrap();
        let oid = repo.resolve_commit(&id).unwrap();

        std::fs::write(worktree.join("notes.md"), "version two, overwritten").unwrap();

        let restored = repo.restore_paths(oid, "notes.md", &worktree).unwrap();
        assert_eq!(restored, vec!["notes.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(worktree.join("notes.md")).unwrap(),
            "version one"
        );
    }

    #[test]
    fn restore_missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("workspace.git");
        let worktree = dir.path().join("workspace");
        std::fs::create_dir_all(&worktree).unwrap();
        let repo = GitRepo::open_or_init(&git_dir).unwrap();

        let files = write_files(&worktree, &[("notes.md", "hi")]);
        let id = repo
            .commit_snapshot(&files, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "v1"))
            .unwrap()
            .unwrap();
        let oid = repo.resolve_commit(&id).unwrap();

        let result = repo.restore_paths(oid, "missing.md", &worktree);
        assert!(matches!(result, Err(CheckpointError::PathNotFound(_, _))));
    }

    #[test]
    fn log_walks_history_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();

        let first = write_files(dir.path(), &[("a.txt", "1")]);
        repo.commit_snapshot(&first, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "one"))
            .unwrap();
        let second = write_files(dir.path(), &[("a.txt", "2")]);
        repo.commit_snapshot(&second, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "two"))
            .unwrap();

        let (items, next) = repo.log(None, 10, None).unwrap();
        assert_eq!(items.len(), 2);
        let newest = items.first().expect("two items");
        let oldest = items.get(1).expect("two items");
        assert_eq!(newest.1.summary, "two", "newest first");
        assert_eq!(oldest.1.summary, "one");
        assert!(next.is_none());
    }

    #[test]
    fn log_pages_with_a_limit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();

        for n in 0..3 {
            let files = write_files(dir.path(), &[("a.txt", &n.to_string())]);
            repo.commit_snapshot(
                &files,
                Utc::now(),
                &ctx(CheckpointTrigger::TurnEnd, &n.to_string()),
            )
            .unwrap();
        }

        let (first_page, cursor) = repo.log(None, 2, None).unwrap();
        assert_eq!(first_page.len(), 2);
        let cursor = cursor.expect("a third checkpoint remains");
        let (second_page, next) = repo.log(Some(cursor), 2, None).unwrap();
        assert_eq!(second_page.len(), 1);
        assert!(next.is_none());
    }

    #[test]
    fn stats_report_count_and_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();
        let (count_before, oldest_before) = repo.stats().unwrap();
        assert_eq!(count_before, 0);
        assert!(oldest_before.is_none());

        let files = write_files(dir.path(), &[("a.txt", "1")]);
        repo.commit_snapshot(&files, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "one"))
            .unwrap();
        let (count_after, oldest_after) = repo.stats().unwrap();
        assert_eq!(count_after, 1);
        assert!(oldest_after.is_some());
    }

    #[test]
    fn file_diff_renders_unified_text_for_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::open_or_init(&dir.path().join("workspace.git")).unwrap();

        let first = write_files(dir.path(), &[("a.txt", "line one\n")]);
        repo.commit_snapshot(&first, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "one"))
            .unwrap();
        let second = write_files(dir.path(), &[("a.txt", "line one\nline two\n")]);
        let id = repo
            .commit_snapshot(&second, Utc::now(), &ctx(CheckpointTrigger::TurnEnd, "two"))
            .unwrap()
            .unwrap();
        let oid = repo.resolve_commit(&id).unwrap();

        let diff = repo.file_diff(oid, "a.txt").unwrap().unwrap();
        assert!(diff.contains("+line two"));
    }
}

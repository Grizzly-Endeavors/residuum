//! The `team/` namespace and the team write coordinator.
//!
//! Every agent sees one logical tree: its own directory, plus a `team/`
//! prefix that maps onto the shared team directory. [`TeamFiles`] is one
//! writer's view of that tree — path resolution, plus the coordinator that
//! keeps concurrent writers to a team file from silently overwriting each
//! other.
//!
//! [`TeamWriteCoordinator`] is a hub-level service: constructed once, then
//! cloned (it is a handle around shared state) into every agent's tools and
//! into the web file API. It keeps a per-path async lock and, for each path,
//! a write generation plus the identity of the last writer that went through
//! it. A file tool records a [`FileStamp`] when it reads a team file; a later
//! write or edit takes the path's lock, compares the file's current stamp
//! with the recorded one, and writes only when they match. A mismatch names
//! who changed the file, or an unknown writer when the change did not go
//! through Residuum.

use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::OwnedMutexGuard;

use super::version::version_token;

/// The name of the directory prefix that addresses the shared team layer.
pub const TEAM_PREFIX: &str = "team";

/// More markers than this are dropped wholesale; see
/// [`TeamPathGuard::record_tree_removed`].
const TREE_MARKER_LIMIT: usize = 256;

/// Path locks are pruned once the table grows past this many entries.
const LOCK_TABLE_PRUNE_THRESHOLD: usize = 1024;

/// Who wrote a team file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamWriter {
    /// An agent, by name.
    Agent(String),
    /// The user, through the web file API.
    User,
}

impl TeamWriter {
    /// Stable label for logs: `agent:<name>` or `user`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Agent(name) => format!("agent:{name}"),
            Self::User => "user".to_string(),
        }
    }
}

impl fmt::Display for TeamWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// A file's state as seen at one moment: its mtime-based version token
/// (`None` when the file does not exist) plus how many writes have gone
/// through the coordinator for its path.
///
/// The generation catches back-to-back writes that land on the same mtime
/// tick and size, which the version token alone cannot tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    /// [`version_token`] of the file, `None` when it does not exist.
    pub version: Option<String>,
    /// Writes recorded by the coordinator for this path so far.
    pub generation: u64,
}

/// What the coordinator knows about the last write to one path.
#[derive(Debug, Default)]
struct PathRecord {
    generation: u64,
    /// The last write that went through the coordinator: who wrote it, and
    /// the version the file had right after (`None` for a removal).
    last: Option<(TeamWriter, Option<String>)>,
}

#[derive(Default)]
struct Inner {
    root: PathBuf,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    records: Mutex<HashMap<PathBuf, PathRecord>>,
    /// Directories removed or moved away by a writer, so a later write into
    /// the gone subtree can still name who removed it.
    removed_trees: Mutex<HashMap<PathBuf, TeamWriter>>,
}

/// Hub-level service that serializes and checks writes under the team
/// directory. Cloning yields another handle to the same shared state.
#[derive(Clone)]
pub struct TeamWriteCoordinator {
    inner: Arc<Inner>,
}

impl TeamWriteCoordinator {
    /// A coordinator for the team directory at `team_root`.
    #[must_use]
    pub fn new(team_root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                root: team_root.into(),
                ..Inner::default()
            }),
        }
    }

    /// The team directory this coordinator guards.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The view of the team namespace for the agent named `agent_name`, whose
    /// own directory is `agent_dir`. Its writes are attributed to that agent.
    #[must_use]
    pub fn view_for_agent(&self, agent_name: &str, agent_dir: impl Into<PathBuf>) -> TeamFiles {
        TeamFiles {
            coordinator: self.clone(),
            writer: TeamWriter::Agent(agent_name.to_string()),
            agent_dir: agent_dir.into(),
        }
    }

    /// The view used by the web file API, whose writes are attributed to the
    /// user.
    #[must_use]
    pub fn view_for_user(&self, agent_dir: impl Into<PathBuf>) -> TeamFiles {
        TeamFiles {
            coordinator: self.clone(),
            writer: TeamWriter::User,
            agent_dir: agent_dir.into(),
        }
    }

    /// Whether `path` is the team directory or lies under it.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        canonical_key(path).starts_with(canonical_key(&self.inner.root))
    }

    /// The current stamp of `path`, for a reader to record before it reads.
    ///
    /// # Errors
    /// Returns an error if the file can't be examined for a reason other than
    /// not existing.
    pub async fn stamp(&self, path: &Path) -> std::io::Result<FileStamp> {
        let key = canonical_key(path);
        // Generation first, then the file: a write that lands in between
        // leaves the recorded stamp older than the content read next, which
        // can only produce a spurious conflict, never a missed one.
        let generation = lock(&self.inner.records)
            .get(&key)
            .map_or(0, |record| record.generation);
        let version = current_version(path).await?;
        Ok(FileStamp {
            version,
            generation,
        })
    }

    /// Take the write lock for `path`, waiting for any writer already
    /// holding it.
    pub async fn lock(&self, path: &Path) -> TeamPathGuard {
        let key = canonical_key(path);
        let mutex = {
            let mut locks = lock(&self.inner.locks);
            if locks.len() > LOCK_TABLE_PRUNE_THRESHOLD {
                locks.retain(|_, held| Arc::strong_count(held) > 1);
            }
            Arc::clone(locks.entry(key.clone()).or_default())
        };
        let held = mutex.lock_owned().await;
        TeamPathGuard {
            coordinator: self.clone(),
            path: path.to_path_buf(),
            key,
            _held: held,
        }
    }
}

/// Exclusive access to one team path. Dropping it releases the lock.
pub struct TeamPathGuard {
    coordinator: TeamWriteCoordinator,
    path: PathBuf,
    key: PathBuf,
    _held: OwnedMutexGuard<()>,
}

/// Who a conflicting change is attributed to, as far as Residuum knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamConflict {
    /// The writer of the change the caller has not seen, `None` when the file
    /// changed outside Residuum.
    pub changed_by: Option<TeamWriter>,
    /// Whether the file no longer exists.
    pub missing: bool,
}

impl TeamPathGuard {
    /// The path's current stamp, read under the lock.
    ///
    /// # Errors
    /// Returns an error if the file can't be examined for a reason other than
    /// not existing.
    pub async fn stamp(&self) -> std::io::Result<FileStamp> {
        self.coordinator.stamp(&self.path).await
    }

    /// Compare the path's current stamp with `expected`, the stamp the caller
    /// recorded when it read the file (`None` if it never read one, which
    /// only permits creating a file that does not exist).
    ///
    /// # Errors
    /// Returns a [`TeamConflict`] naming the last known writer when the file
    /// is not in the state the caller last saw.
    pub async fn check(&self, expected: Option<&FileStamp>) -> Result<(), CheckError> {
        let current = self.stamp().await.map_err(CheckError::Io)?;
        let unchanged = match expected {
            Some(seen) => *seen == current,
            None => current.version.is_none(),
        };
        if unchanged {
            return Ok(());
        }
        Err(CheckError::Conflict(TeamConflict {
            changed_by: self.last_writer(&current),
            missing: current.version.is_none(),
        }))
    }

    /// The last writer known to have produced `current`'s state.
    fn last_writer(&self, current: &FileStamp) -> Option<TeamWriter> {
        let recorded = lock(&self.coordinator.inner.records)
            .get(&self.key)
            .and_then(|record| record.last.clone())
            .filter(|(_, version)| *version == current.version)
            .map(|(writer, _)| writer);
        if recorded.is_some() || current.version.is_some() {
            return recorded;
        }
        lock(&self.coordinator.inner.removed_trees)
            .iter()
            .find(|(tree, _)| self.key.starts_with(tree))
            .map(|(_, writer)| writer.clone())
    }

    /// Write `bytes` to the path atomically, creating missing parent
    /// directories, and record `writer` as the path's last writer. Returns
    /// the stamp of the file as written.
    ///
    /// # Errors
    /// Returns an error if the parent can't be created or the write fails.
    pub async fn commit(&self, writer: &TeamWriter, bytes: &[u8]) -> anyhow::Result<FileStamp> {
        use anyhow::Context as _;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        crate::util::fs::atomic_write(&self.path, bytes).await?;
        let version = current_version(&self.path)
            .await
            .with_context(|| format!("failed to stat {} after writing", self.path.display()))?;
        Ok(self.record(writer, version))
    }

    /// Record that `writer` removed or moved away the file at the path, after
    /// the caller did so.
    pub fn record_removed(&self, writer: &TeamWriter) {
        self.record(writer, None);
    }

    /// Record that `writer` created or replaced the file at the path by some
    /// means other than [`Self::commit`] (a move landing on it), after the
    /// caller did so. Returns the resulting stamp.
    ///
    /// # Errors
    /// Returns an error if the file can't be examined.
    pub async fn record_written(&self, writer: &TeamWriter) -> std::io::Result<FileStamp> {
        let version = current_version(&self.path).await?;
        Ok(self.record(writer, version))
    }

    /// Record that `writer` removed the directory at the path, so a later
    /// write into it can name who removed it. Markers past a bounded count
    /// are dropped all at once; attribution then falls back to an unknown
    /// writer, never to a wrong one.
    pub fn record_tree_removed(&self, writer: &TeamWriter) {
        let mut trees = lock(&self.coordinator.inner.removed_trees);
        if trees.len() >= TREE_MARKER_LIMIT {
            trees.clear();
        }
        trees.insert(self.key.clone(), writer.clone());
    }

    fn record(&self, writer: &TeamWriter, version: Option<String>) -> FileStamp {
        let mut records = lock(&self.coordinator.inner.records);
        let record = records.entry(self.key.clone()).or_default();
        record.generation += 1;
        record.last = Some((writer.clone(), version.clone()));
        FileStamp {
            version,
            generation: record.generation,
        }
    }
}

/// Why [`TeamPathGuard::check`] did not let a write proceed.
#[derive(Debug)]
pub enum CheckError {
    /// The file changed since the caller read it.
    Conflict(TeamConflict),
    /// The file could not be examined.
    Io(std::io::Error),
}

/// Why a path could not be placed in the namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamPathError(pub String);

impl fmt::Display for TeamPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TeamPathError {}

/// One writer's view of the logical tree: the agent's directory plus the
/// `team/` prefix.
#[derive(Clone)]
pub struct TeamFiles {
    coordinator: TeamWriteCoordinator,
    writer: TeamWriter,
    agent_dir: PathBuf,
}

impl TeamFiles {
    /// The coordinator shared by every view of this team.
    #[must_use]
    pub fn coordinator(&self) -> &TeamWriteCoordinator {
        &self.coordinator
    }

    /// The identity this view's writes are recorded under.
    #[must_use]
    pub fn writer(&self) -> &TeamWriter {
        &self.writer
    }

    /// The team directory.
    #[must_use]
    pub fn team_root(&self) -> &Path {
        self.coordinator.root()
    }

    /// The agent directory relative paths outside `team/` resolve against.
    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Resolve a path as given to a file tool. A relative path starting with
    /// `team/` (or exactly `team`) addresses the team directory; any other
    /// relative path resolves against the agent directory; an absolute path
    /// is used as is.
    #[must_use]
    pub fn resolve(&self, path: &str) -> PathBuf {
        let (base, rest) = self.locate(path);
        base.join(rest)
    }

    /// Split a path as given to a file tool into the directory it is relative
    /// to (the team directory for `team/...`, otherwise the agent directory)
    /// and the remainder. An absolute path comes back whole, and joins onto
    /// either base as itself.
    #[must_use]
    pub fn locate(&self, path: &str) -> (&Path, PathBuf) {
        match team_relative_path(path) {
            Some(rest) => (self.team_root(), rest),
            None => (&self.agent_dir, PathBuf::from(path)),
        }
    }

    /// Whether `path` lies in the team directory.
    #[must_use]
    pub fn is_team_path(&self, path: &Path) -> bool {
        self.coordinator.contains(path)
    }

    /// `path` as the agent addresses it: `team/...` for team files, otherwise
    /// the path unchanged.
    #[must_use]
    pub fn display(&self, path: &Path) -> String {
        let key = canonical_key(path);
        match key.strip_prefix(canonical_key(self.team_root())) {
            Ok(rest) if rest.as_os_str().is_empty() => TEAM_PREFIX.to_string(),
            Ok(rest) => format!(
                "{TEAM_PREFIX}/{}",
                rest.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            ),
            Err(_) => path.display().to_string(),
        }
    }

    /// Refuse a path that names, or lies inside, a `team` entry of the agent
    /// directory. That name is the prefix for the shared team directory, so
    /// the agent directory may not hold one of its own.
    ///
    /// # Errors
    /// Returns a [`TeamPathError`] explaining the reservation.
    pub fn check_no_agent_team_entry(&self, path: &Path) -> Result<(), TeamPathError> {
        let reserved = canonical_key(&self.agent_dir.join(TEAM_PREFIX));
        if canonical_key(path).starts_with(&reserved) {
            return Err(TeamPathError(format!(
                "{} is inside the agent folder's own `{TEAM_PREFIX}` entry, which is not allowed: \
                 `{TEAM_PREFIX}/` always means the shared team folder. Use a relative path \
                 starting with `{TEAM_PREFIX}/` to work in the shared team folder, or choose a \
                 different name",
                path.display()
            )));
        }
        Ok(())
    }

    /// Take the path's write lock and verify the file is still as the caller
    /// last saw it. On a conflict the message is ready to hand to the agent
    /// and the conflict has been logged.
    ///
    /// # Errors
    /// Returns the tool-facing message when the file changed since it was
    /// read, or could not be examined.
    pub async fn lock_unchanged(
        &self,
        path: &Path,
        expected: Option<&FileStamp>,
    ) -> Result<TeamPathGuard, String> {
        let guard = self.coordinator.lock(path).await;
        match guard.check(expected).await {
            Ok(()) => Ok(guard),
            Err(CheckError::Io(e)) => Err(format!(
                "failed to check {} before writing: {e}",
                self.display(path)
            )),
            Err(CheckError::Conflict(conflict)) => {
                let shown = self.display(path);
                let changed_by = conflict
                    .changed_by
                    .as_ref()
                    .map_or_else(|| "unknown".to_string(), TeamWriter::label);
                tracing::info!(
                    path = %shown,
                    attempted_by = %self.writer,
                    changed_by = %changed_by,
                    "team write refused: the file changed since it was read"
                );
                Err(self.conflict_message(&shown, &conflict))
            }
        }
    }

    fn conflict_message(&self, display: &str, conflict: &TeamConflict) -> String {
        let who = match &conflict.changed_by {
            Some(writer) if *writer == self.writer => "another session of yours".to_string(),
            Some(TeamWriter::Agent(name)) => format!("teammate {name}"),
            Some(TeamWriter::User) => "the user".to_string(),
            None => "an unknown writer (a change made outside Residuum)".to_string(),
        };
        let next = if conflict.missing {
            "It no longer exists. Read it again with read_file to confirm, then write_file can \
             create it again"
        } else {
            "Read it again with read_file, then reapply your change to the current contents"
        };
        format!(
            "{display} changed since you read it: it was changed by {who}. Nothing was written. \
             {next}"
        )
    }
}

/// The remainder of `path` after a leading `team` component, when `path` is a
/// relative path addressing the team directory.
#[must_use]
pub fn team_relative_path(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return None;
    }
    let mut components = path
        .components()
        .skip_while(|component| matches!(component, Component::CurDir));
    match components.next() {
        Some(Component::Normal(first)) if first == TEAM_PREFIX => {
            Some(components.collect::<PathBuf>())
        }
        _ => None,
    }
}

/// Lexically normalize `path` (drop `.`, apply `..`), then canonicalize its
/// nearest existing ancestor, so the result is stable whether or not the file
/// exists and whichever symlinked spelling reached it.
pub(crate) fn canonical_key(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    let mut existing = normalized.clone();
    let mut missing = Vec::new();
    while !existing.exists() {
        match existing.file_name().map(std::ffi::OsStr::to_os_string) {
            Some(name) => missing.push(name),
            None => return normalized,
        }
        if !existing.pop() {
            return normalized;
        }
    }
    let mut key = std::fs::canonicalize(&existing).unwrap_or(existing);
    key.extend(missing.into_iter().rev());
    key
}

/// The version token of the file at `path`, `None` when it does not exist.
async fn current_version(path: &Path) -> std::io::Result<Option<String>> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(Some(version_token(&metadata))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Lock a std mutex, recovering the data if a panicking holder poisoned it:
/// every critical section here leaves the maps consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, TeamWriteCoordinator, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let team = dir.path().join("team");
        let agent = dir.path().join("scout");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::create_dir_all(&agent).unwrap();
        (dir, TeamWriteCoordinator::new(&team), agent, team)
    }

    /// Write `bytes` to `path` through the coordinator as `writer`.
    async fn commit_as(
        coordinator: &TeamWriteCoordinator,
        path: &Path,
        writer: &TeamWriter,
        bytes: &[u8],
    ) {
        let guard = coordinator.lock(path).await;
        guard.commit(writer, bytes).await.unwrap();
    }

    #[test]
    fn team_prefix_resolves_under_the_team_root() {
        let (_dir, coordinator, agent, team) = setup();
        let files = coordinator.view_for_agent("scout", &agent);
        assert_eq!(
            files.resolve("team/wiki/a.md"),
            team.join("wiki").join("a.md")
        );
        assert_eq!(files.resolve("./team/a.md"), team.join("a.md"));
        assert_eq!(files.resolve("team"), team);
        assert_eq!(
            files.resolve("notes/a.md"),
            agent.join("notes").join("a.md")
        );
        assert_eq!(
            files.resolve("teamwork/a.md"),
            agent.join("teamwork").join("a.md"),
            "only the exact `team` segment is the prefix"
        );
    }

    #[test]
    fn absolute_paths_are_used_as_is() {
        let (_dir, coordinator, agent, team) = setup();
        let files = coordinator.view_for_agent("scout", &agent);
        let absolute = team.join("a.md");
        assert_eq!(files.resolve(absolute.to_str().unwrap()), absolute);
        assert!(files.is_team_path(&absolute));
        assert!(!files.is_team_path(&agent.join("a.md")));
    }

    #[test]
    fn display_uses_the_team_prefix() {
        let (_dir, coordinator, agent, team) = setup();
        let files = coordinator.view_for_agent("scout", &agent);
        assert_eq!(
            files.display(&team.join("wiki").join("a.md")),
            "team/wiki/a.md"
        );
        assert_eq!(files.display(&team), "team");
    }

    #[test]
    fn the_agent_dir_cannot_hold_a_team_entry() {
        let (_dir, coordinator, agent, team) = setup();
        let files = coordinator.view_for_agent("scout", &agent);
        assert!(
            files
                .check_no_agent_team_entry(&agent.join("team").join("a.md"))
                .is_err()
        );
        assert!(
            files
                .check_no_agent_team_entry(&agent.join("team"))
                .is_err()
        );
        assert!(
            files
                .check_no_agent_team_entry(&agent.join("teamwork"))
                .is_ok()
        );
        assert!(files.check_no_agent_team_entry(&team.join("a.md")).is_ok());
    }

    #[tokio::test]
    async fn stale_stamp_is_a_conflict_naming_the_last_writer() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        commit_as(
            &coordinator,
            &path,
            &TeamWriter::Agent("robin".into()),
            b"one",
        )
        .await;

        let seen = coordinator.stamp(&path).await.unwrap();

        commit_as(
            &coordinator,
            &path,
            &TeamWriter::Agent("robin".into()),
            b"two, longer",
        )
        .await;

        let err = sam.lock_unchanged(&path, Some(&seen)).await.err().unwrap();
        assert!(err.contains("team/a.md"), "{err}");
        assert!(err.contains("teammate robin"), "{err}");
        assert!(err.contains("read_file"), "{err}");
    }

    #[tokio::test]
    async fn unchanged_stamp_allows_the_write() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        commit_as(&coordinator, &path, sam.writer(), b"one").await;
        let seen = coordinator.stamp(&path).await.unwrap();
        assert!(sam.lock_unchanged(&path, Some(&seen)).await.is_ok());
    }

    #[tokio::test]
    async fn same_size_rewrite_in_one_tick_is_still_a_conflict() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        commit_as(&coordinator, &path, &TeamWriter::User, b"aaaa").await;
        let seen = coordinator.stamp(&path).await.unwrap();
        commit_as(&coordinator, &path, &TeamWriter::User, b"bbbb").await;

        let err = sam.lock_unchanged(&path, Some(&seen)).await.err().unwrap();
        assert!(err.contains("the user"), "{err}");
    }

    #[tokio::test]
    async fn outside_change_is_an_unknown_writer() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        std::fs::write(&path, "one").unwrap();
        let seen = coordinator.stamp(&path).await.unwrap();
        std::fs::write(&path, "changed by an editor").unwrap();

        let err = sam.lock_unchanged(&path, Some(&seen)).await.err().unwrap();
        assert!(err.contains("unknown writer"), "{err}");
    }

    #[tokio::test]
    async fn creating_over_a_file_that_appeared_is_a_conflict() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("new.md");
        commit_as(&coordinator, &path, &TeamWriter::User, b"hi").await;

        let err = sam.lock_unchanged(&path, None).await.err().unwrap();
        assert!(err.contains("the user"), "{err}");
    }

    #[tokio::test]
    async fn removal_names_the_remover() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        commit_as(&coordinator, &path, sam.writer(), b"one").await;
        let seen = coordinator.stamp(&path).await.unwrap();

        let guard = coordinator.lock(&path).await;
        std::fs::remove_file(&path).unwrap();
        guard.record_removed(&TeamWriter::User);
        drop(guard);

        let err = sam.lock_unchanged(&path, Some(&seen)).await.err().unwrap();
        assert!(err.contains("the user"), "{err}");
    }

    #[tokio::test]
    async fn own_earlier_session_is_not_called_a_teammate() {
        let (_dir, coordinator, agent, team) = setup();
        let sam = coordinator.view_for_agent("sam", &agent);
        let path = team.join("a.md");
        commit_as(&coordinator, &path, sam.writer(), b"one").await;
        let seen = coordinator.stamp(&path).await.unwrap();
        commit_as(&coordinator, &path, sam.writer(), b"two!").await;

        let err = sam.lock_unchanged(&path, Some(&seen)).await.err().unwrap();
        assert!(err.contains("another session of yours"), "{err}");
    }
}

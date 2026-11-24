use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::OwnedMutexGuard;

use super::version::version_token;
use residuum_config::paths::TeamPaths;

pub const TEAM_PREFIX: &str = "team";

const TREE_MARKER_LIMIT: usize = 256;

const LOCK_TABLE_PRUNE_THRESHOLD: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamWriter {
    Agent(String),
    User,
}

impl TeamWriter {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub version: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Default)]
struct PathRecord {
    generation: u64,
    last: Option<(TeamWriter, Option<String>)>,
}

#[derive(Default)]
struct Inner {
    root: PathBuf,
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    records: Mutex<HashMap<PathBuf, PathRecord>>,
    removed_trees: Mutex<HashMap<PathBuf, TeamWriter>>,
}

#[derive(Clone)]
pub struct TeamWriteCoordinator {
    inner: Arc<Inner>,
}

impl TeamWriteCoordinator {
    #[must_use]
    pub fn new(team: &TeamPaths) -> Self {
        Self {
            inner: Arc::new(Inner {
                root: team.root().to_path_buf(),
                ..Inner::default()
            }),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    #[must_use]
    pub fn view_for_agent(&self, agent_name: &str, agent_dir: impl Into<PathBuf>) -> TeamFiles {
        TeamFiles {
            coordinator: self.clone(),
            writer: TeamWriter::Agent(agent_name.to_string()),
            agent_dir: agent_dir.into(),
        }
    }

    #[must_use]
    pub fn view_for_user(&self, agent_dir: impl Into<PathBuf>) -> TeamFiles {
        TeamFiles {
            coordinator: self.clone(),
            writer: TeamWriter::User,
            agent_dir: agent_dir.into(),
        }
    }

    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        canonical_key(path).starts_with(canonical_key(&self.inner.root))
    }

    pub async fn stamp(&self, path: &Path) -> std::io::Result<FileStamp> {
        let key = canonical_key(path);
        let generation = lock(&self.inner.records)
            .get(&key)
            .map_or(0, |record| record.generation);
        let version = current_version(path).await?;
        Ok(FileStamp {
            version,
            generation,
        })
    }

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

    #[must_use]
    pub fn lock_contenders(&self, path: &Path) -> usize {
        lock(&self.inner.locks)
            .get(&canonical_key(path))
            .map_or(0, |held| Arc::strong_count(held).saturating_sub(1))
    }

    pub async fn lock_all(&self, paths: &[PathBuf]) -> Vec<TeamPathGuard> {
        let mut ordered: Vec<&PathBuf> = paths.iter().collect();
        ordered.sort_by_key(|path| canonical_key(path));
        ordered.dedup_by_key(|path| canonical_key(path));
        let mut guards = Vec::with_capacity(ordered.len());
        for path in ordered {
            guards.push(self.lock(path).await);
        }
        guards
    }
}

pub struct TeamPathGuard {
    coordinator: TeamWriteCoordinator,
    path: PathBuf,
    key: PathBuf,
    _held: OwnedMutexGuard<()>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamConflict {
    pub changed_by: Option<TeamWriter>,
    pub missing: bool,
}

impl TeamPathGuard {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn stamp(&self) -> std::io::Result<FileStamp> {
        self.coordinator.stamp(&self.path).await
    }

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

    pub async fn commit(&self, writer: &TeamWriter, bytes: &[u8]) -> anyhow::Result<FileStamp> {
        use anyhow::Context as _;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        residuum_core::util::fs::atomic_write(&self.path, bytes).await?;
        let version = current_version(&self.path)
            .await
            .with_context(|| format!("failed to stat {} after writing", self.path.display()))?;
        Ok(self.record(writer, version))
    }

    pub fn record_removed(&self, writer: &TeamWriter) {
        self.record(writer, None);
    }

    pub async fn record_written(&self, writer: &TeamWriter) -> std::io::Result<FileStamp> {
        let version = current_version(&self.path).await?;
        Ok(self.record(writer, version))
    }

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

#[derive(Debug)]
pub enum CheckError {
    Conflict(TeamConflict),
    Io(std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamPathError(pub String);

impl fmt::Display for TeamPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TeamPathError {}

#[derive(Clone)]
pub struct TeamFiles {
    coordinator: TeamWriteCoordinator,
    writer: TeamWriter,
    agent_dir: PathBuf,
}

impl TeamFiles {
    #[must_use]
    pub fn coordinator(&self) -> &TeamWriteCoordinator {
        &self.coordinator
    }

    #[must_use]
    pub fn writer(&self) -> &TeamWriter {
        &self.writer
    }

    #[must_use]
    pub fn team_root(&self) -> &Path {
        self.coordinator.root()
    }

    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    #[must_use]
    pub fn resolve(&self, path: &str) -> PathBuf {
        let (base, rest) = self.locate(path);
        base.join(rest)
    }

    #[must_use]
    pub fn locate(&self, path: &str) -> (&Path, PathBuf) {
        match team_relative_path(path) {
            Some(rest) => (self.team_root(), rest),
            None => (&self.agent_dir, PathBuf::from(path)),
        }
    }

    #[must_use]
    pub fn is_team_path(&self, path: &Path) -> bool {
        self.coordinator.contains(path)
    }

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

#[must_use]
pub fn canonical_key(path: &Path) -> PathBuf {
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

async fn current_version(path: &Path) -> std::io::Result<Option<String>> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(Some(version_token(&metadata))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

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
        (
            dir,
            TeamWriteCoordinator::new(&TeamPaths::new(&team)),
            agent,
            team,
        )
    }

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

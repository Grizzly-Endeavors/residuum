//! Test fixtures for code that works on the hub's directories.

use std::path::PathBuf;

use crate::config::paths::TeamPaths;
use crate::workspace::team_files::TeamWriteCoordinator;

/// A hub directory with a team directory and any number of agent directories,
/// and the coordinator every writer of the team directory shares.
pub(crate) struct TeamHub {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) coordinator: TeamWriteCoordinator,
}

impl TeamHub {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("team")).unwrap();
        let coordinator = TeamWriteCoordinator::new(&TeamPaths::new(dir.path().join("team")));
        Self { dir, coordinator }
    }

    pub(crate) fn team(&self) -> PathBuf {
        self.dir.path().join("team")
    }

    /// The directory of the agent `name`, created if it is not there yet.
    pub(crate) fn agent_dir(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

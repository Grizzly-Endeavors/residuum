//! Test fixtures for code that works on the hub's directories. Behind
//! `test-support`, and compiled for this crate's own tests.

use std::path::PathBuf;

use residuum_config::paths::TeamPaths;

use crate::team_files::TeamWriteCoordinator;

/// A hub directory with a team directory and any number of agent directories,
/// and the coordinator every writer of the team directory shares.
pub struct TeamHub {
    /// The hub directory itself; the team directory and every agent directory sit under it.
    pub dir: tempfile::TempDir,
    /// The coordinator every writer of the team directory shares.
    pub coordinator: TeamWriteCoordinator,
}

impl TeamHub {
    /// A hub directory with an empty team directory.
    ///
    /// # Panics
    ///
    /// Panics when the temporary directory or its team directory cannot be created.
    #[must_use]
    #[expect(
        clippy::unwrap_used,
        reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
    )]
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("team")).unwrap();
        let coordinator = TeamWriteCoordinator::new(&TeamPaths::new(dir.path().join("team")));
        Self { dir, coordinator }
    }

    /// The team directory under the hub directory.
    #[must_use]
    pub fn team(&self) -> PathBuf {
        self.dir.path().join("team")
    }

    /// The directory of the agent `name`, created if it is not there yet.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be created.
    #[must_use]
    #[expect(
        clippy::unwrap_used,
        reason = "test-support helper: a fixture that cannot be built fails the test that asked for it"
    )]
    pub fn agent_dir(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

impl Default for TeamHub {
    fn default() -> Self {
        Self::new()
    }
}

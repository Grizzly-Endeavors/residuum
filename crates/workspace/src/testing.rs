//! Test fixtures for code that works on the hub's directories. Behind
//! `test-support`, and compiled for this crate's own tests.

use std::path::{Path, PathBuf};

use residuum_config::paths::TeamPaths;

use crate::inbox::InboxItem;
use crate::inbox::mail::{Mailbox, MailboxDirectory};
use crate::team_files::TeamWriteCoordinator;

/// Write `item` as `filename` in `inbox_dir` without claiming an id, for a
/// fixture that needs an item already in place.
///
/// # Errors
/// Returns an error if the item cannot be serialized or written.
pub async fn save_item_for_tests(
    inbox_dir: &Path,
    filename: &str,
    item: &InboxItem,
) -> anyhow::Result<()> {
    crate::inbox::save_item(inbox_dir, filename, item).await
}

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

/// A fixed set of mailboxes, for tests of code that reads mail through a
/// [`MailboxDirectory`].
#[derive(Debug, Clone, Default)]
pub struct StaticMailboxDirectory {
    mailboxes: Vec<Mailbox>,
}

impl StaticMailboxDirectory {
    /// A directory of `mailboxes`, kept sorted by agent name.
    pub fn new(mailboxes: impl IntoIterator<Item = Mailbox>) -> Self {
        let mut mailboxes: Vec<Mailbox> = mailboxes.into_iter().collect();
        mailboxes.sort_by(|a, b| a.agent.cmp(&b.agent));
        Self { mailboxes }
    }

    /// The mailboxes of `agents` under `hub`'s directory, in UTC, each
    /// agent's directory created.
    #[must_use]
    pub fn for_team_hub(hub: &TeamHub, agents: &[&str]) -> Self {
        Self::new(agents.iter().map(|agent| Mailbox {
            agent: (*agent).to_string(),
            dir: hub.agent_dir(agent),
            timezone: chrono_tz::UTC,
        }))
    }
}

impl MailboxDirectory for StaticMailboxDirectory {
    fn mailboxes(&self) -> Vec<Mailbox> {
        self.mailboxes.clone()
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        self.mailboxes.iter().find(|m| m.agent == agent).cloned()
    }
}

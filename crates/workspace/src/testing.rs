use std::path::{Path, PathBuf};

use residuum_config::paths::TeamPaths;

use crate::inbox::InboxItem;
use crate::inbox::mail::{Mailbox, MailboxDirectory};
use crate::team_files::TeamWriteCoordinator;

pub async fn save_item_for_tests(
    inbox_dir: &Path,
    filename: &str,
    item: &InboxItem,
) -> anyhow::Result<()> {
    crate::inbox::save_item(inbox_dir, filename, item).await
}

pub struct TeamHub {
    pub dir: tempfile::TempDir,
    pub coordinator: TeamWriteCoordinator,
}

impl TeamHub {
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

    #[must_use]
    pub fn team(&self) -> PathBuf {
        self.dir.path().join("team")
    }

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

#[derive(Debug, Clone, Default)]
pub struct StaticMailboxDirectory {
    mailboxes: Vec<Mailbox>,
}

impl StaticMailboxDirectory {
    pub fn new(mailboxes: impl IntoIterator<Item = Mailbox>) -> Self {
        let mut mailboxes: Vec<Mailbox> = mailboxes.into_iter().collect();
        mailboxes.sort_by(|a, b| a.agent.cmp(&b.agent));
        Self { mailboxes }
    }

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

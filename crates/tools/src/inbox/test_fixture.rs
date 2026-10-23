//! Agents' workspaces with inbox items written straight to disk, for the
//! inbox tool tests.

use std::sync::Arc;

use chrono::NaiveDate;
use residuum_config::WorkspaceLayout;
use residuum_workspace::inbox::mail::MailboxDirectory;
use residuum_workspace::inbox::{InboxItem, InboxKind, list_items_or_empty, load_item, save_item};
use residuum_workspace::testing::{StaticMailboxDirectory, TeamHub};

use super::MailAccess;

/// One inbox item to write.
pub(super) struct Item {
    agent: String,
    kind: InboxKind,
    id: String,
    contents: InboxItem,
    archived: bool,
}

impl Item {
    fn new(
        agent: &str,
        kind: InboxKind,
        id: &str,
        title: &str,
        body: &str,
        source: &str,
        hour: u32,
    ) -> Self {
        Self {
            agent: agent.to_string(),
            kind,
            id: id.to_string(),
            contents: InboxItem {
                title: title.to_string(),
                body: body.to_string(),
                source: source.to_string(),
                timestamp: NaiveDate::from_ymd_opt(2026, 2, 25)
                    .unwrap()
                    .and_hms_opt(hour, 0, 0)
                    .unwrap(),
                read: false,
                attachments: Vec::new(),
                in_reply_to: None,
            },
            archived: false,
        }
    }

    /// A message `agent` sent the user: an item in its user inbox.
    pub fn agent_message(agent: &str, id: &str, title: &str, body: &str, hour: u32) -> Self {
        Self::new(agent, InboxKind::User, id, title, body, "agent", hour)
    }

    /// A message the user sent `agent` from the web: an item in its agent inbox.
    pub fn user_mail(agent: &str, id: &str, title: &str, body: &str, hour: u32) -> Self {
        Self::new(agent, InboxKind::Agent, id, title, body, "web", hour)
    }

    /// The item answers the message `parent` names.
    pub fn replying_to(mut self, parent: &str) -> Self {
        self.contents.in_reply_to = Some(parent.to_string());
        self
    }

    /// The item is in the archive.
    pub fn archived(mut self) -> Self {
        self.archived = true;
        self.contents.read = true;
        self
    }
}

/// A hub directory of agents, each with its own workspace.
pub(super) struct MailHub {
    hub: TeamHub,
    directory: Arc<StaticMailboxDirectory>,
}

impl MailHub {
    pub fn new(agents: &[&str]) -> Self {
        let hub = TeamHub::new();
        let directory = Arc::new(StaticMailboxDirectory::for_team_hub(&hub, agents));
        Self { hub, directory }
    }

    pub fn layout(&self, agent: &str) -> WorkspaceLayout {
        WorkspaceLayout::new(self.hub.agent_dir(agent))
    }

    /// The mail access of `agent`, reaching every agent.
    pub fn access(&self, agent: &str) -> MailAccess {
        let directory: Arc<dyn MailboxDirectory> =
            Arc::<StaticMailboxDirectory>::clone(&self.directory);
        MailAccess::new(Some(directory), &self.layout(agent), chrono_tz::UTC)
    }

    /// The mail access of `agent` with no hub directory.
    pub fn access_without_directory(&self, agent: &str) -> MailAccess {
        MailAccess::new(None, &self.layout(agent), chrono_tz::UTC)
    }

    pub async fn write(&self, item: Item) {
        let dir = item
            .kind
            .items_dir(&self.layout(&item.agent), item.archived);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        save_item(&dir, &format!("{}.json", item.id), &item.contents)
            .await
            .unwrap();
    }

    /// The active item `id` of `agent`'s inbox of `kind`.
    pub async fn load(&self, agent: &str, kind: InboxKind, id: &str) -> InboxItem {
        let dir = kind.items_dir(&self.layout(agent), false);
        load_item(&dir.join(format!("{id}.json"))).await.unwrap()
    }

    /// Every active item of `agent`'s inbox of `kind`, newest first.
    pub async fn items(&self, agent: &str, kind: InboxKind) -> Vec<(String, InboxItem)> {
        list_items_or_empty(&kind.items_dir(&self.layout(agent), false))
            .await
            .unwrap()
    }
}

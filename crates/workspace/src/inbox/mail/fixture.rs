//! Agents' workspaces with mail items written straight to disk, for the mail tests.

use std::path::PathBuf;

use chrono::{NaiveDate, NaiveDateTime};
use residuum_config::WorkspaceLayout;

use super::{MailIndex, Mailbox};
use crate::inbox::{InboxItem, InboxKind, InboxRef, archive_item, load_item, mark_read, save_item};
use crate::testing::StaticMailboxDirectory;

pub(super) struct Fixture {
    _dir: tempfile::TempDir,
    pub directory: StaticMailboxDirectory,
}

pub(super) fn at(hour: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 2, 25)
        .unwrap()
        .and_hms_opt(hour, 0, 0)
        .unwrap()
}

impl Fixture {
    pub fn new(agents: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let directory = StaticMailboxDirectory::new(agents.iter().map(|agent| Mailbox {
            agent: (*agent).to_string(),
            dir: dir.path().join(agent),
            timezone: chrono_tz::UTC,
        }));
        Self {
            _dir: dir,
            directory,
        }
    }

    pub fn dir(&self, agent: &str) -> PathBuf {
        use super::MailboxDirectory as _;
        self.directory.mailbox(agent).unwrap().dir
    }

    fn layout(&self, agent: &str) -> WorkspaceLayout {
        WorkspaceLayout::new(self.dir(agent))
    }

    pub async fn index(&self) -> MailIndex {
        MailIndex::load(&self.directory).await
    }

    async fn write(&self, agent: &str, kind: InboxKind, id: &str, item: &InboxItem) -> InboxRef {
        let dir = kind.items_dir(&self.layout(agent), false);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        save_item(&dir, &format!("{id}.json"), item).await.unwrap();
        InboxRef::new(agent, kind, id).unwrap()
    }

    fn item(title: &str, body: &str, source: &str, hour: u32, reply: Option<String>) -> InboxItem {
        InboxItem {
            title: title.to_string(),
            body: body.to_string(),
            source: source.to_string(),
            timestamp: at(hour),
            read: false,
            attachments: Vec::new(),
            in_reply_to: reply,
        }
    }

    /// A message `agent` sent the user.
    pub async fn user_item(
        &self,
        agent: &str,
        id: &str,
        title: &str,
        hour: u32,
        in_reply_to: Option<&InboxRef>,
    ) -> InboxRef {
        let item = Self::item(
            title,
            &format!("{title} body"),
            "agent",
            hour,
            in_reply_to.map(ToString::to_string),
        );
        self.write(agent, InboxKind::User, id, &item).await
    }

    /// A user-inbox item whose `in_reply_to` is stored as `raw`.
    pub async fn write_raw_reply(&self, agent: &str, id: &str, title: &str, hour: u32, raw: &str) {
        let item = Self::item(title, "body", "agent", hour, Some(raw.to_string()));
        self.write(agent, InboxKind::User, id, &item).await;
    }

    /// A user message with a copy in each of `to`'s agent inboxes; returns the
    /// first recipient's copy.
    pub async fn user_mail(
        &self,
        id: &str,
        to: &[&str],
        title: &str,
        hour: u32,
        in_reply_to: Option<&InboxRef>,
    ) -> InboxRef {
        let item = Self::item(
            title,
            &format!("{title} body"),
            "web",
            hour,
            in_reply_to.map(ToString::to_string),
        );
        let mut refs = Vec::new();
        for agent in to {
            refs.push(self.write(agent, InboxKind::Agent, id, &item).await);
        }
        refs.sort();
        refs.remove(0)
    }

    /// An agent-inbox item with source `web`.
    pub async fn agent_item(&self, agent: &str, id: &str, title: &str, body: &str, hour: u32) {
        let item = Self::item(title, body, "web", hour, None);
        self.write(agent, InboxKind::Agent, id, &item).await;
    }

    pub async fn agent_item_with_source(&self, agent: &str, id: &str, source: &str, hour: u32) {
        let item = Self::item(id, "body", source, hour, None);
        self.write(agent, InboxKind::Agent, id, &item).await;
    }

    pub async fn archive_user_item(&self, agent: &str, id: &str) {
        let layout = self.layout(agent);
        archive_item(
            &layout.user_inbox_dir(),
            &layout.user_inbox_archive_dir(),
            id,
        )
        .await
        .unwrap();
    }

    pub async fn mark_agent_copy_read(&self, agent: &str, id: &str) {
        mark_read(&self.layout(agent).agent_inbox_dir(), id)
            .await
            .unwrap();
    }

    pub async fn archive_agent_copy(&self, agent: &str, id: &str) {
        let layout = self.layout(agent);
        archive_item(
            &layout.agent_inbox_dir(),
            &layout.agent_inbox_archive_dir(),
            id,
        )
        .await
        .unwrap();
    }

    /// Put a file in an active user-inbox item's attachment directory and
    /// record it on the item.
    pub async fn attach_to_user_item(&self, agent: &str, id: &str, filename: &str, bytes: &[u8]) {
        let layout = self.layout(agent);
        let dir = layout.user_inbox_attachments_dir().join(id);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join(filename), bytes).await.unwrap();
        let path = layout.user_inbox_dir().join(format!("{id}.json"));
        let mut item = load_item(&path).await.unwrap();
        item.attachments.push(
            PathBuf::from("inbox/user/attachments")
                .join(id)
                .join(filename),
        );
        save_item(&layout.user_inbox_dir(), &format!("{id}.json"), &item)
            .await
            .unwrap();
    }

    /// The stored item at `item`, active or archived.
    pub async fn load(&self, item: &InboxRef) -> (InboxItem, bool) {
        let layout = self.layout(item.agent());
        for archived in [false, true] {
            let path = item
                .kind()
                .items_dir(&layout, archived)
                .join(format!("{}.json", item.id()));
            if let Ok(loaded) = load_item(&path).await {
                return (loaded, archived);
            }
        }
        panic!("no item at {item}");
    }
}

//! Agents' workspaces with inbox items written straight to disk, for the
//! inbox tool tests.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::NaiveDate;
use residuum_agent::Tool;
use residuum_config::WorkspaceLayout;
use residuum_workspace::inbox::mail::MailboxDirectory;
use residuum_workspace::inbox::{InboxItem, InboxKind, list_items_or_empty, load_item};
use residuum_workspace::testing::save_item_for_tests;
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

    /// The item carries a file named `filename`, written beside it when the
    /// item is saved.
    pub fn with_attachment(mut self, filename: &str) -> Self {
        self.contents.attachments.push(PathBuf::from(filename));
        self
    }

    /// The item has been read.
    pub fn read(mut self) -> Self {
        self.contents.read = true;
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
        let layout = self.layout(&item.agent);
        let dir = item.kind.items_dir(&layout, item.archived);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        save_item_for_tests(&dir, &format!("{}.json", item.id), &item.contents)
            .await
            .unwrap();
        if !item.contents.attachments.is_empty() {
            let files = item
                .kind
                .attachments_dir(&layout, item.archived)
                .join(&item.id);
            tokio::fs::create_dir_all(&files).await.unwrap();
            for stored in &item.contents.attachments {
                tokio::fs::write(files.join(stored), b"attached")
                    .await
                    .unwrap();
            }
        }
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

/// The ids on the item lines of a listing's output, in order: the last word
/// before the title on each line.
pub(super) fn listed_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.starts_with("  ["))
        .filter_map(|line| line.split(" — ").next()?.rsplit(' ').next())
        .map(str::to_string)
        .collect()
}

/// The cursor a listing's output tells the caller to pass as `before`, when
/// a next page follows.
pub(super) fn next_cursor(output: &str) -> Option<String> {
    let (_, rest) = output.split_once("before=\"")?;
    let (cursor, _) = rest.split_once('"')?;
    Some(cursor.to_string())
}

/// The output of every page `tool` lists for `arguments`, first to last: each
/// call after the first passes the cursor the page before it ended with.
pub(super) async fn pages(tool: &dyn Tool, mut arguments: serde_json::Value) -> Vec<String> {
    let mut outputs = Vec::new();
    loop {
        let result = tool.execute(arguments.clone()).await.unwrap();
        assert!(!result.is_error, "{}", result.output);
        let next = next_cursor(&result.output);
        outputs.push(result.output);
        let Some(cursor) = next else {
            return outputs;
        };
        assert!(outputs.len() < 1_000, "the pages never end");
        if let Some(map) = arguments.as_object_mut() {
            map.insert("before".to_string(), serde_json::Value::String(cursor));
        }
    }
}

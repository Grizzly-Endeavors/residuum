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

    pub fn agent_message(agent: &str, id: &str, title: &str, body: &str, hour: u32) -> Self {
        Self::new(agent, InboxKind::User, id, title, body, "agent", hour)
    }

    pub fn user_mail(agent: &str, id: &str, title: &str, body: &str, hour: u32) -> Self {
        Self::new(agent, InboxKind::Agent, id, title, body, "web", hour)
    }

    pub fn replying_to(mut self, parent: &str) -> Self {
        self.contents.in_reply_to = Some(parent.to_string());
        self
    }

    pub fn with_attachment(mut self, filename: &str) -> Self {
        self.contents.attachments.push(PathBuf::from(filename));
        self
    }

    pub fn read(mut self) -> Self {
        self.contents.read = true;
        self
    }

    pub fn archived(mut self) -> Self {
        self.archived = true;
        self.contents.read = true;
        self
    }
}

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

    pub fn access(&self, agent: &str) -> MailAccess {
        let directory: Arc<dyn MailboxDirectory> =
            Arc::<StaticMailboxDirectory>::clone(&self.directory);
        MailAccess::new(Some(directory), &self.layout(agent), chrono_tz::UTC)
    }

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

    pub async fn load(&self, agent: &str, kind: InboxKind, id: &str) -> InboxItem {
        let dir = kind.items_dir(&self.layout(agent), false);
        load_item(&dir.join(format!("{id}.json"))).await.unwrap()
    }

    pub async fn items(&self, agent: &str, kind: InboxKind) -> Vec<(String, InboxItem)> {
        list_items_or_empty(&kind.items_dir(&self.layout(agent), false))
            .await
            .unwrap()
    }
}

pub(super) fn listed_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.starts_with("  ["))
        .filter_map(|line| line.split(" — ").next()?.rsplit(' ').next())
        .map(str::to_string)
        .collect()
}

pub(super) fn next_cursor(output: &str) -> Option<String> {
    let (_, rest) = output.split_once("before=\"")?;
    let (cursor, _) = rest.split_once('"')?;
    Some(cursor.to_string())
}

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

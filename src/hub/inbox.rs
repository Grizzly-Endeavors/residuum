//! Every agent's user inbox, read and changed from the hub.
//!
//! An agent's user inbox is files under its directory: `inbox/user/` for
//! items and `archive/inbox/user/` for archived ones. The hub reads and
//! changes them directly, so an agent answers whether it is running, stopped
//! or failed. [`list`] merges every agent's items into one newest-first list,
//! [`unread`] counts the unread ones, and [`mark_read`], [`archive`] and
//! [`restore`] change one item.
//!
//! Stored item times are naive local times. The hub reads them in its
//! configured timezone at the moment it reads them, so a change to the
//! timezone moves the instants it reports (see
//! [`crate::time::local_to_instant`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentDirectory, AgentFiles, LifecycleError};
use crate::inbox::{InboxAttachment, InboxItem};
use crate::workspace::layout::WorkspaceLayout;

/// How many items a page holds when the request names no limit.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// The most items a page holds; a larger limit is treated as this.
pub const MAX_PAGE_SIZE: usize = 200;

/// Which of an agent's two lists of user inbox items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InboxStatus {
    /// Items waiting in the inbox.
    #[default]
    Active,
    /// Items the user archived.
    Archived,
}

impl InboxStatus {
    /// The directory holding this list's items, and the one holding their
    /// attachments.
    fn dirs(self, layout: &WorkspaceLayout) -> (PathBuf, PathBuf) {
        match self {
            Self::Active => (layout.user_inbox_dir(), layout.user_inbox_attachments_dir()),
            Self::Archived => (
                layout.user_inbox_archive_dir(),
                layout.user_inbox_archive_attachments_dir(),
            ),
        }
    }
}

/// One user inbox item of one agent, as the hub API returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct HubInboxItem {
    /// The agent whose inbox holds the item.
    pub agent: String,
    /// The item's id within that agent's inbox. Two agents can have items
    /// with the same id.
    pub id: String,
    /// Short summary of the item.
    pub title: String,
    /// Full body text, Markdown.
    pub body: String,
    /// Where the item came from, such as `agent` or `hub`.
    pub source: String,
    /// When the item was created, as RFC 3339 with an offset.
    pub at: String,
    /// Whether the user has opened the item.
    pub read: bool,
    /// The item's files.
    pub attachments: Vec<InboxAttachment>,
}

/// One page of the cross-agent inbox listing, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct HubInboxPage {
    /// The items on this page.
    pub items: Vec<HubInboxItem>,
    /// Pass as `before` to get the page after this one; `null` on the last page.
    pub next_cursor: Option<String>,
}

/// Unread user inbox items, in total and per agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct HubInboxUnread {
    /// Every agent's unread items added up.
    pub total: u32,
    /// Each agent's unread items, agents with none included.
    pub by_agent: BTreeMap<String, u32>,
}

/// What [`list`] is asked for.
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    /// Which list to read.
    pub status: InboxStatus,
    /// Only this agent's items; every agent's when `None`.
    pub agent: Option<String>,
    /// A `next_cursor` from an earlier page: the page returned starts after it.
    pub before: Option<String>,
    /// How many items at most; [`DEFAULT_PAGE_SIZE`] when `None`.
    pub limit: Option<usize>,
}

/// Why an inbox call failed. The HTTP layer maps these to status codes:
/// `UnknownAgent` and `UnknownItem` 404, `BadRequest` 400, `Conflict` 409,
/// `Failed` 500. Every message is safe to show the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HubInboxError {
    /// No agent has this name.
    #[error("no agent named '{0}'")]
    UnknownAgent(String),
    /// The agent has no such item where the call looks for it.
    #[error("{0}")]
    UnknownItem(String),
    /// The request is malformed; the message says how.
    #[error("{0}")]
    BadRequest(String),
    /// The move would replace a different item that has the same id.
    #[error("{0}")]
    Conflict(String),
    /// Reading or changing the inbox's files failed.
    #[error("{0}")]
    Failed(String),
}

/// An item found on disk, with the instant its stored time denotes.
struct Entry {
    agent: String,
    id: String,
    at: DateTime<chrono_tz::Tz>,
    item: InboxItem,
    attachments_root: PathBuf,
}

impl Entry {
    /// The order of [`list`], read from its last to its first element: the
    /// instant, then the id, then the agent. The agent is last so the order
    /// is total when two agents have an item with the same id at the same
    /// instant.
    fn sort_key(&self) -> (i64, &str, &str) {
        (self.at.timestamp(), &self.id, &self.agent)
    }
}

/// Where a page stopped, carried in `next_cursor`: the sort key of its last
/// item, as `<unix seconds>:<agent>:<id>`. An agent name has no `:`, so the
/// id is everything after the second one.
#[derive(Debug, PartialEq, Eq)]
struct Cursor {
    at: i64,
    agent: String,
    id: String,
}

impl Cursor {
    fn after(entry: &Entry) -> Self {
        Self {
            at: entry.at.timestamp(),
            agent: entry.agent.clone(),
            id: entry.id.clone(),
        }
    }

    fn render(&self) -> String {
        format!("{}:{}:{}", self.at, self.agent, self.id)
    }

    fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(3, ':');
        let at = parts.next()?.parse().ok()?;
        let agent = parts.next().filter(|agent| !agent.is_empty())?;
        let id = parts.next().filter(|id| !id.is_empty())?;
        Some(Self {
            at,
            agent: agent.to_string(),
            id: id.to_string(),
        })
    }

    fn sort_key(&self) -> (i64, &str, &str) {
        (self.at, &self.id, &self.agent)
    }
}

/// One page of the user inbox items of every agent (or of `query.agent`),
/// newest first by time, then id.
///
/// # Errors
/// [`HubInboxError::UnknownAgent`] when `query.agent` names no agent;
/// [`HubInboxError::BadRequest`] for a zero limit or a cursor this call
/// didn't issue; [`HubInboxError::Failed`] when an agent's inbox can't be
/// read, which fails the whole call so the user sees it instead of a list
/// with an agent's items missing.
pub async fn list(
    directory: &dyn AgentDirectory,
    query: &ListQuery,
) -> Result<HubInboxPage, HubInboxError> {
    let limit = match query.limit {
        None => DEFAULT_PAGE_SIZE,
        Some(0) => {
            return Err(HubInboxError::BadRequest(
                "the limit must be at least 1".to_string(),
            ));
        }
        Some(limit) => limit.min(MAX_PAGE_SIZE),
    };
    let before = query
        .before
        .as_deref()
        .map(|raw| {
            Cursor::parse(raw).ok_or_else(|| {
                HubInboxError::BadRequest(
                    "the before cursor isn't one this inbox returned".to_string(),
                )
            })
        })
        .transpose()?;

    let mut entries = Vec::new();
    match &query.agent {
        Some(name) => {
            let files = files_of(directory, name)?;
            entries.extend(scan(name, &files, query.status).await?);
        }
        None => {
            for summary in directory.list() {
                let Some(files) = listed_files(directory, &summary.name)? else {
                    continue;
                };
                entries.extend(scan(&summary.name, &files, query.status).await?);
            }
        }
    }

    entries.sort_by(|a, b| b.sort_key().cmp(&a.sort_key()));
    if let Some(cursor) = &before {
        entries.retain(|entry| entry.sort_key() < cursor.sort_key());
    }
    let more = entries.len() > limit;
    entries.truncate(limit);
    let next_cursor = if more {
        entries.last().map(|entry| Cursor::after(entry).render())
    } else {
        None
    };

    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        items.push(hub_item(entry).await);
    }
    Ok(HubInboxPage { items, next_cursor })
}

/// Every agent's unread active items, counted from disk. An agent whose inbox
/// can't be read counts as none and is logged, so one broken agent doesn't
/// blank the total for the rest.
///
/// # Errors
/// [`HubInboxError::Failed`] when the directory can't say where an agent's
/// files are.
pub async fn unread(directory: &dyn AgentDirectory) -> Result<HubInboxUnread, HubInboxError> {
    let mut by_agent = BTreeMap::new();
    let mut total: u32 = 0;
    for summary in directory.list() {
        let Some(files) = listed_files(directory, &summary.name)? else {
            continue;
        };
        let count = count_unread(&summary.name, &files.dir).await;
        total = total.saturating_add(count);
        by_agent.insert(summary.name, count);
    }
    Ok(HubInboxUnread { total, by_agent })
}

/// Mark an item read, wherever it is: the active inbox, else the archive.
///
/// # Errors
/// [`HubInboxError::UnknownAgent`], [`HubInboxError::BadRequest`] for an id
/// that is not a bare item id, [`HubInboxError::UnknownItem`] when neither
/// list holds the item, [`HubInboxError::Failed`] when the file can't be
/// changed.
pub async fn mark_read(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
) -> Result<HubInboxItem, HubInboxError> {
    let files = files_of(directory, agent)?;
    validate_id(id)?;
    let layout = WorkspaceLayout::new(&files.dir);
    for status in [InboxStatus::Active, InboxStatus::Archived] {
        let (dir, _) = status.dirs(&layout);
        if !item_exists(&dir, id).await? {
            continue;
        }
        let item = crate::inbox::mark_read(&dir, &file_name(id))
            .await
            .map_err(|e| failed(agent, id, "mark the item read", &e))?;
        tracing::debug!(agent = %agent, id = %id, "hub marked an inbox item read");
        let (_, attachments_root) = status.dirs(&layout);
        let entry = entry_of(agent, id.to_string(), item, &files, attachments_root);
        return Ok(hub_item(entry).await);
    }
    Err(HubInboxError::UnknownItem(format!(
        "{agent} has no inbox item '{id}'"
    )))
}

/// Move an active item to the archive.
///
/// # Errors
/// As [`mark_read`], with [`HubInboxError::UnknownItem`] when the item isn't
/// active, and [`HubInboxError::Conflict`] when the archive already holds a
/// different item with that id.
pub async fn archive(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
) -> Result<HubInboxItem, HubInboxError> {
    move_item(directory, agent, id, InboxStatus::Active).await
}

/// Move an archived item back to the active inbox.
///
/// # Errors
/// As [`mark_read`], with [`HubInboxError::UnknownItem`] when the item isn't
/// archived, and [`HubInboxError::Conflict`] when the active inbox already
/// holds a different item with that id.
pub async fn restore(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
) -> Result<HubInboxItem, HubInboxError> {
    move_item(directory, agent, id, InboxStatus::Archived).await
}

/// Move an item out of the list `from` into the other one, and return it as
/// it reads there.
async fn move_item(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
    from: InboxStatus,
) -> Result<HubInboxItem, HubInboxError> {
    let files = files_of(directory, agent)?;
    validate_id(id)?;
    let layout = WorkspaceLayout::new(&files.dir);
    let (to, verb, from_name, to_name) = match from {
        InboxStatus::Active => (InboxStatus::Archived, "archive", "active", "archive"),
        InboxStatus::Archived => (InboxStatus::Active, "restore", "archived", "inbox"),
    };
    let (from_dir, _) = from.dirs(&layout);
    let (to_dir, _) = to.dirs(&layout);

    if !item_exists(&from_dir, id).await? {
        return Err(HubInboxError::UnknownItem(format!(
            "{agent} has no {from_name} inbox item '{id}'"
        )));
    }
    if item_exists(&to_dir, id).await? {
        tracing::warn!(agent = %agent, id = %id, verb, "an inbox item can't move because a different item with its id is already there");
        return Err(HubInboxError::Conflict(format!(
            "the {to_name} already holds a different item with id '{id}' for {agent}, so this one was left where it is"
        )));
    }

    let moved = match from {
        InboxStatus::Active => crate::inbox::archive_item(&from_dir, &to_dir, &file_name(id)).await,
        InboxStatus::Archived => {
            crate::inbox::restore_item(&from_dir, &to_dir, &file_name(id)).await
        }
    };
    moved.map_err(|e| failed(agent, id, verb, &e))?;
    tracing::debug!(agent = %agent, id = %id, verb, "hub moved an inbox item");
    read_back(agent, id, &files, to).await
}

/// The item as it now reads in `status`'s list.
async fn read_back(
    agent: &str,
    id: &str,
    files: &AgentFiles,
    status: InboxStatus,
) -> Result<HubInboxItem, HubInboxError> {
    let (dir, attachments_root) = status.dirs(&WorkspaceLayout::new(&files.dir));
    let item = crate::inbox::load_item(&dir.join(file_name(id)))
        .await
        .map_err(|e| failed(agent, id, "read the item back", &e))?;
    Ok(hub_item(entry_of(
        agent,
        id.to_string(),
        item,
        files,
        attachments_root,
    ))
    .await)
}

fn entry_of(
    agent: &str,
    id: String,
    item: InboxItem,
    files: &AgentFiles,
    attachments_root: PathBuf,
) -> Entry {
    Entry {
        agent: agent.to_string(),
        id,
        at: crate::time::local_to_instant(files.timezone, item.timestamp),
        item,
        attachments_root,
    }
}

/// The item as the API returns it, with its attachments resolved against disk.
async fn hub_item(entry: Entry) -> HubInboxItem {
    let attachments = crate::inbox::resolve_attachments(
        &entry.agent,
        &entry.id,
        &entry.item,
        &entry.attachments_root,
    )
    .await;
    HubInboxItem {
        at: crate::time::format_rfc3339(&entry.at),
        agent: entry.agent,
        id: entry.id,
        title: entry.item.title,
        body: entry.item.body,
        source: entry.item.source,
        read: entry.item.read,
        attachments,
    }
}

/// Every item in one agent's `status` list.
async fn scan(
    agent: &str,
    files: &AgentFiles,
    status: InboxStatus,
) -> Result<Vec<Entry>, HubInboxError> {
    let (dir, attachments_root) = status.dirs(&WorkspaceLayout::new(&files.dir));
    let items = crate::inbox::list_items_or_empty(&dir).await.map_err(|e| {
        tracing::error!(agent = %agent, error = %e, "couldn't read an agent's inbox for the hub inbox");
        HubInboxError::Failed(format!(
            "couldn't read {agent}'s inbox: {}",
            root_cause(&e)
        ))
    })?;
    Ok(items
        .into_iter()
        .map(|(id, item)| entry_of(agent, id, item, files, attachments_root.clone()))
        .collect())
}

async fn count_unread(agent: &str, agent_dir: &Path) -> u32 {
    let (dir, _) = InboxStatus::Active.dirs(&WorkspaceLayout::new(agent_dir));
    match crate::inbox::list_items_or_empty(&dir).await {
        Ok(items) => {
            let unread = items.iter().filter(|(_, item)| !item.read).count();
            u32::try_from(unread).unwrap_or(u32::MAX)
        }
        Err(e) => {
            tracing::warn!(agent = %agent, error = %e, "couldn't read an agent's inbox to count its unread items; counting none");
            0
        }
    }
}

fn files_of(directory: &dyn AgentDirectory, agent: &str) -> Result<AgentFiles, HubInboxError> {
    directory.agent_files(agent).map_err(|e| {
        if let LifecycleError::NotFound(name) = e {
            HubInboxError::UnknownAgent(name)
        } else {
            HubInboxError::Failed(e.to_string())
        }
    })
}

/// The files of an agent that appeared in the directory's list, or `None` when
/// it was deleted since, so it has nothing to add.
fn listed_files(
    directory: &dyn AgentDirectory,
    agent: &str,
) -> Result<Option<AgentFiles>, HubInboxError> {
    match directory.agent_files(agent) {
        Ok(files) => Ok(Some(files)),
        Err(LifecycleError::NotFound(_)) => Ok(None),
        Err(e) => Err(HubInboxError::Failed(e.to_string())),
    }
}

/// An id names one file in the inbox directory, so it can't point elsewhere.
fn validate_id(id: &str) -> Result<(), HubInboxError> {
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
        return Err(HubInboxError::BadRequest(format!(
            "'{id}' isn't an inbox item id"
        )));
    }
    Ok(())
}

/// The item's file name. The inbox functions add `.json` only when it is
/// missing, so giving it always keeps an id that ends in `.json` itself
/// meaning its own file.
fn file_name(id: &str) -> String {
    format!("{id}.json")
}

async fn item_exists(dir: &Path, id: &str) -> Result<bool, HubInboxError> {
    let path = dir.join(file_name(id));
    tokio::fs::try_exists(&path).await.map_err(|e| {
        tracing::error!(path = %path.display(), error = %e, "couldn't check for an inbox item");
        HubInboxError::Failed(format!("couldn't look for the inbox item '{id}': {e}"))
    })
}

fn failed(agent: &str, id: &str, action: &str, error: &anyhow::Error) -> HubInboxError {
    tracing::error!(agent = %agent, id = %id, error = %format!("{error:#}"), "couldn't change an inbox item");
    HubInboxError::Failed(format!(
        "couldn't {action} for {agent}'s inbox item '{id}': {}",
        root_cause(error)
    ))
}

/// The innermost cause of `error`, which is what the user can act on; the
/// whole chain goes to the log.
fn root_cause(error: &anyhow::Error) -> String {
    error.root_cause().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_survives_a_round_trip_even_when_the_id_holds_colons() {
        let cursor = Cursor {
            at: 1_790_000_000,
            agent: "scout".to_string(),
            id: "20260930_a:b:c".to_string(),
        };
        assert_eq!(Cursor::parse(&cursor.render()), Some(cursor));
    }

    #[test]
    fn a_cursor_needs_a_time_an_agent_and_an_id() {
        for raw in [
            "",
            "scout:id",
            "soon:scout:id",
            "1790000000::id",
            "1790000000:scout:",
            "1790000000",
        ] {
            assert_eq!(Cursor::parse(raw), None, "{raw:?} isn't a cursor");
        }
    }

    #[test]
    fn an_item_id_is_one_path_segment() {
        for id in ["20260227_deploy_completed", "café", "a.b", "..x", "x.json"] {
            assert!(validate_id(id).is_ok(), "{id:?} is usable");
        }
        for id in ["", ".", "..", "a/b", "a\\b", "../x", "a\0b"] {
            assert!(
                matches!(validate_id(id), Err(HubInboxError::BadRequest(_))),
                "{id:?} is refused"
            );
        }
    }

    #[tokio::test]
    async fn a_directory_that_cannot_place_an_agents_files_fails_the_call() {
        let directory = crate::a2a::StaticAgentDirectory::new().with_agent(
            "scout",
            crate::hub::A2aVisibility::Private,
            axum::Router::new(),
        );

        let unread = unread(&directory).await;
        let page = list(&directory, &ListQuery::default()).await;

        assert!(
            matches!(unread, Err(HubInboxError::Failed(_))),
            "{unread:?}"
        );
        assert!(matches!(page, Err(HubInboxError::Failed(_))), "{page:?}");
    }
}

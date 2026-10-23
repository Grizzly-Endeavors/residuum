//! How the inbox tools reach mail: every agent's mailbox through the hub's
//! [`MailboxDirectory`], or only this agent's own when there is none.

use std::path::PathBuf;
use std::sync::Arc;

use residuum_config::WorkspaceLayout;
use residuum_workspace::inbox::mail::{Mailbox, MailboxDirectory};
use residuum_workspace::inbox::{self, InboxItem, InboxKind, InboxRef};

/// The line a tool adds when it could read only this agent's own inbox.
pub(super) const OWN_INBOX_ONLY_NOTE: &str = "Note: only this agent's own inboxes could be read here; the other agents' inboxes are not reachable right now.";

/// This agent's mailbox, and the hub's directory of every agent's mailbox
/// when the registry was given one.
#[derive(Clone)]
pub struct MailAccess {
    directory: Option<Arc<dyn MailboxDirectory>>,
    own: Mailbox,
}

impl MailAccess {
    /// Mail access for the agent whose workspace is `layout`, whose items are
    /// stored in `tz`. Its folder name is the workspace directory's name.
    /// `directory` reaches every agent's mailbox; `None` reaches only this
    /// agent's own.
    #[must_use]
    pub fn new(
        directory: Option<Arc<dyn MailboxDirectory>>,
        layout: &WorkspaceLayout,
        tz: chrono_tz::Tz,
    ) -> Self {
        let agent = layout
            .root()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            directory,
            own: Mailbox {
                agent,
                dir: layout.root().to_path_buf(),
                timezone: tz,
            },
        }
    }

    /// This agent's folder name, the first part of its items' refs.
    pub(super) fn own_agent(&self) -> &str {
        &self.own.agent
    }

    /// This agent's workspace layout.
    pub(super) fn own_layout(&self) -> WorkspaceLayout {
        WorkspaceLayout::new(self.own.dir.clone())
    }

    /// The timezone this agent's items are stored in.
    pub(super) fn own_timezone(&self) -> chrono_tz::Tz {
        self.own.timezone
    }

    /// The directory to read mail through now. The hub's directory when it
    /// knows this agent; otherwise (no directory, or the hub is shutting
    /// down) this agent's own mailbox alone.
    pub(super) fn view(&self) -> MailView {
        match &self.directory {
            Some(directory) if directory.mailbox(&self.own.agent).is_some() => MailView {
                directory: Arc::clone(directory),
                every_agent: true,
            },
            _ => MailView {
                directory: Arc::new(OwnMailboxOnly(self.own.clone())),
                every_agent: false,
            },
        }
    }
}

/// The mailboxes a tool call reads.
pub(super) struct MailView {
    pub directory: Arc<dyn MailboxDirectory>,
    /// Whether `directory` reaches every agent, not just this one.
    pub every_agent: bool,
}

/// A directory holding only this agent's own mailbox.
struct OwnMailboxOnly(Mailbox);

impl MailboxDirectory for OwnMailboxOnly {
    fn mailboxes(&self) -> Vec<Mailbox> {
        vec![self.0.clone()]
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        (self.0.agent == agent).then(|| self.0.clone())
    }
}

/// An inbox item found by its ref.
pub(super) struct LocatedItem {
    pub item: InboxItem,
    /// The directory holding the item's JSON file.
    pub items_dir: PathBuf,
    /// Whether the item is in the archive.
    pub archived: bool,
    /// The owning agent's workspace, which the item's attachment paths are
    /// relative to.
    pub workspace_dir: PathBuf,
}

/// Why an item could not be found by its ref.
#[derive(Debug)]
pub(super) enum LocateError {
    UnknownAgent(String),
    NotFound(InboxRef),
    Read {
        item: InboxRef,
        error: anyhow::Error,
    },
}

impl std::fmt::Display for LocateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownAgent(agent) => write!(f, "there is no agent named '{agent}'"),
            Self::NotFound(item) => write!(f, "no inbox item {item} was found"),
            Self::Read { item, error } => write!(f, "failed to read inbox item {item}: {error:#}"),
        }
    }
}

/// Find the item `item_ref` names, active or archived, in the mailboxes of
/// `directory`.
pub(super) async fn locate(
    directory: &dyn MailboxDirectory,
    item_ref: &InboxRef,
) -> Result<LocatedItem, LocateError> {
    let mailbox = directory
        .mailbox(item_ref.agent())
        .ok_or_else(|| LocateError::UnknownAgent(item_ref.agent().to_string()))?;
    let layout = WorkspaceLayout::new(mailbox.dir.clone());
    for archived in [false, true] {
        let items_dir = item_ref.kind().items_dir(&layout, archived);
        let path = items_dir.join(format!("{}.json", item_ref.id()));
        if !matches!(tokio::fs::try_exists(&path).await, Ok(true)) {
            continue;
        }
        let item = inbox::load_item(&path)
            .await
            .map_err(|error| LocateError::Read {
                item: item_ref.clone(),
                error,
            })?;
        return Ok(LocatedItem {
            item,
            items_dir,
            archived,
            workspace_dir: mailbox.dir,
        });
    }
    Err(LocateError::NotFound(item_ref.clone()))
}

/// The ref an agent passed to a tool: a full `<agent>/<user|agent>/<id>`
/// ref, or an id alone, which names an item in this agent's own inbox of
/// `own_kind`. A trailing `.json` on an id is dropped.
pub(super) fn parse_item_ref(
    raw: &str,
    own_agent: &str,
    own_kind: InboxKind,
) -> Result<InboxRef, residuum_workspace::inbox::InboxRefError> {
    let raw = raw.trim();
    if raw.contains('/') {
        InboxRef::parse(raw)
    } else {
        InboxRef::new(own_agent, own_kind, raw.trim_end_matches(".json"))
    }
}

/// The local minute format every inbox tool prints times in.
pub(super) const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M";

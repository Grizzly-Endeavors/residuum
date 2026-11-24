use std::path::PathBuf;
use std::sync::Arc;

use residuum_config::WorkspaceLayout;
use residuum_workspace::inbox::mail::{Mailbox, MailboxDirectory};
use residuum_workspace::inbox::{self, InboxItem, InboxKind, InboxRef};

pub(super) const OWN_INBOX_ONLY_NOTE: &str = "Note: only this agent's own inboxes could be read here; the other agents' inboxes are not reachable right now.";

#[derive(Clone)]
pub struct MailAccess {
    directory: Option<Arc<dyn MailboxDirectory>>,
    own: Mailbox,
}

impl MailAccess {
    #[must_use]
    pub fn new(
        directory: Option<Arc<dyn MailboxDirectory>>,
        layout: &WorkspaceLayout,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            directory,
            own: Mailbox::for_workspace(layout, tz),
        }
    }

    pub(super) fn own_agent(&self) -> &str {
        &self.own.agent
    }

    pub(super) fn own_layout(&self) -> WorkspaceLayout {
        WorkspaceLayout::new(self.own.dir.clone())
    }

    pub(super) fn own_timezone(&self) -> chrono_tz::Tz {
        self.own.timezone
    }

    pub(super) fn view(&self) -> MailView {
        match &self.directory {
            Some(directory) if directory.mailbox(&self.own.agent).is_some() => MailView {
                directory: Arc::clone(directory),
                every_agent: true,
            },
            _ => MailView {
                directory: Arc::new(self.own.clone()),
                every_agent: false,
            },
        }
    }
}

pub(super) struct MailView {
    pub directory: Arc<dyn MailboxDirectory>,
    pub every_agent: bool,
}

pub(super) struct LocatedItem {
    pub item: InboxItem,
    pub items_dir: PathBuf,
    pub archived: bool,
    pub workspace_dir: PathBuf,
}

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

pub(super) async fn locate(
    directory: &dyn MailboxDirectory,
    item_ref: &InboxRef,
) -> Result<LocatedItem, LocateError> {
    let mailbox = directory
        .mailbox(item_ref.agent())
        .ok_or_else(|| LocateError::UnknownAgent(item_ref.agent().to_string()))?;
    let layout = WorkspaceLayout::new(mailbox.dir.clone());
    let found = inbox::locate_item(&layout, item_ref)
        .await
        .map_err(|error| LocateError::Read {
            item: item_ref.clone(),
            error,
        })?
        .ok_or_else(|| LocateError::NotFound(item_ref.clone()))?;
    Ok(LocatedItem {
        item: found.item,
        items_dir: found.items_dir,
        archived: found.archived,
        workspace_dir: mailbox.dir,
    })
}

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

pub(super) const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M";

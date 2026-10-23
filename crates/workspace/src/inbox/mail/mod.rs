//! Mail: the user's conversations with the agents, derived from the inbox files.
//!
//! There is no mail store. A message an agent sent the user is an item in that
//! agent's `inbox/user/`; a message the user sent is an item in each recipient's
//! `inbox/agent/` with source `web` or `cli`, one copy per recipient, all with the
//! same id. Every other agent-inbox item (pulse, cron, webhook, artifacts, system
//! notes) is not mail and never appears here. Active and archived items both
//! count; archiving keeps an item's id, so its [`InboxRef`] stays the same.
//!
//! - [`MailIndex`] reads every agent's mail through a [`MailboxDirectory`] and
//!   assembles the threads: an item whose `in_reply_to` names another message
//!   is threaded under it, across agents and across the archive.
//! - [`mark_thread_read`], [`archive_thread`] and [`restore_thread`] change a
//!   thread's user-inbox items (the user's mailbox), never the agents' copies of
//!   the user's messages.
//! - [`send_mail`] writes one user-authored message to one or more agents, and
//!   [`OutgoingMail::reply`] builds the reply-all to a thread.
//!
//! [`InboxRef`]: crate::inbox::InboxRef

use std::path::PathBuf;

use residuum_config::WorkspaceLayout;

mod handle;
mod send;
mod thread;
mod update;

pub use handle::MailboxDirectoryHandle;
pub use send::{
    OutgoingMail, SendMailError, SentMail, UserMailSource, is_user_mail_source, reply_subject,
    send_mail,
};
pub use thread::{
    MailAttachment, MailFolder, MailIndex, MailLoadProblem, MailMessage, MailSeen, MailSender,
    MailThread, ThreadFilter,
};
pub use update::{
    MailItemFailure, ThreadUpdateError, archive_thread, mark_thread_read, restore_thread,
};

/// One agent's workspace as mail reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    /// The agent's folder name, the first part of its items' refs.
    pub agent: String,
    /// The agent's workspace directory (`~/.residuum/<name>`).
    pub dir: PathBuf,
    /// The timezone the naive local times stored in the agent's items are in.
    pub timezone: chrono_tz::Tz,
}

impl Mailbox {
    /// The mailbox of the agent whose workspace is `layout`, whose items are
    /// stored in `tz`. Its folder name is the workspace directory's name.
    #[must_use]
    pub fn for_workspace(layout: &WorkspaceLayout, tz: chrono_tz::Tz) -> Self {
        let agent = layout
            .root()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            agent,
            dir: layout.root().to_path_buf(),
            timezone: tz,
        }
    }
}

/// One mailbox is a directory of one agent: what an agent reaches on its own,
/// without the hub's directory.
impl MailboxDirectory for Mailbox {
    fn mailboxes(&self) -> Vec<Mailbox> {
        vec![self.clone()]
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        (self.agent == agent).then(|| self.clone())
    }
}

/// The agents whose workspaces hold mail. The hub implements it on its agent
/// directory; tools hold it as an `Arc<dyn MailboxDirectory>`, reaching the
/// hub through a [`MailboxDirectoryHandle`].
pub trait MailboxDirectory: Send + Sync {
    /// Every agent's mailbox, whatever the agent's state, sorted by agent name.
    fn mailboxes(&self) -> Vec<Mailbox>;

    /// The mailbox of the agent with folder name `agent`, or `None` for an
    /// unknown agent.
    fn mailbox(&self, agent: &str) -> Option<Mailbox>;
}

#[cfg(test)]
mod fixture;

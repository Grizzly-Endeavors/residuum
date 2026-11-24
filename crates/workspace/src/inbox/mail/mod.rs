use std::path::PathBuf;

use residuum_config::WorkspaceLayout;

mod cursor;
mod handle;
mod send;
mod thread;
mod update;

pub use cursor::MailCursor;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub agent: String,
    pub dir: PathBuf,
    pub timezone: chrono_tz::Tz,
}

impl Mailbox {
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

impl MailboxDirectory for Mailbox {
    fn mailboxes(&self) -> Vec<Mailbox> {
        vec![self.clone()]
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        (self.agent == agent).then(|| self.clone())
    }
}

pub trait MailboxDirectory: Send + Sync {
    fn mailboxes(&self) -> Vec<Mailbox>;

    fn mailbox(&self, agent: &str) -> Option<Mailbox>;
}

#[cfg(test)]
mod fixture;

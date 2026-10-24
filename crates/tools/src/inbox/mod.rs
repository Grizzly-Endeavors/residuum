//! Inbox tools: this agent's own agent inbox (`inbox_list`, `inbox_read`,
//! `inbox_archive`, `inbox_restore`) and the user inbox every agent writes to
//! (`user_inbox_add`, `user_inbox_list`). The mail-aware ones read every
//! agent's mailbox through [`MailAccess`].

mod agent;
mod mail;
mod paging;
mod user;

#[cfg(test)]
mod test_fixture;

pub use agent::{InboxArchiveTool, InboxListTool, InboxReadTool, InboxRestoreTool};
pub use mail::MailAccess;
pub use user::{UserInboxAddTool, UserInboxListTool};

//! The user's mail, read and changed from the hub: the wire types of the
//! `/api/hub/mail/*` routes, the agents' mailboxes as the mail domain reads
//! them, and the unread counts the overview, the app badge and pushes use.
//!
//! Threads are assembled by [`residuum_workspace::inbox::mail`] from the
//! agents' inbox files, so the hub answers whether an agent is running,
//! stopped or failed. The types here are that domain as the web sees it:
//! refs as strings, times as RFC 3339, and each attachment with the URL that
//! serves its bytes.
//!
//! Stored item times are naive local times. The hub reads them in its
//! configured timezone at the moment it reads them, so a change to the
//! timezone moves the instants it reports (see
//! [`residuum_core::time::local_to_instant`]).

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use super::{AgentDirectory, AgentFiles};
use crate::overview::plain_preview_within;
use residuum_background::team::LifecycleError;
use residuum_config::WorkspaceLayout;
use residuum_workspace::inbox::mail::{self as domain, Mailbox, MailboxDirectory};
use residuum_workspace::inbox::{InboxKind, InboxRef};

/// How many characters of the latest message a thread summary previews.
pub const SNIPPET_CHARS: usize = 140;

/// The agents of an [`AgentDirectory`] as mailboxes: each agent's directory,
/// read in the hub's timezone.
pub struct AgentMailboxes<'a>(pub &'a dyn AgentDirectory);

impl MailboxDirectory for AgentMailboxes<'_> {
    fn mailboxes(&self) -> Vec<Mailbox> {
        self.0
            .list()
            .into_iter()
            .filter_map(|summary| mailbox_of(self.0, &summary.name))
            .collect()
    }

    fn mailbox(&self, agent: &str) -> Option<Mailbox> {
        mailbox_of(self.0, agent)
    }
}

/// `agent`'s mailbox, or `None` when the directory doesn't know it. A
/// directory that knows the agent but can't place its files is logged, since
/// the agent's mail is then missing from what the user sees.
fn mailbox_of(directory: &dyn AgentDirectory, agent: &str) -> Option<Mailbox> {
    match directory.agent_files(agent) {
        Ok(AgentFiles { dir, timezone }) => Some(Mailbox {
            agent: agent.to_string(),
            dir,
            timezone,
        }),
        Err(LifecycleError::NotFound(_)) => None,
        Err(e) => {
            tracing::warn!(agent = %agent, error = %e, "couldn't place an agent's files; its mail is left out");
            None
        }
    }
}

/// One page of threads, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailThreadPage {
    /// The threads on this page.
    pub threads: Vec<MailThreadSummary>,
    /// Pass as `before` to get the page after this one; `null` on the last page.
    pub next: Option<String>,
}

/// A thread as the thread list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire shape the mail contract names: each flag is its own JSON field the thread list reads"
)]
pub struct MailThreadSummary {
    /// The root message's ref.
    pub thread_id: String,
    /// The root message's title.
    pub subject: String,
    /// Every agent in the thread, by folder name.
    pub participants: Vec<String>,
    /// How many messages the thread holds.
    #[ts(type = "number")]
    pub message_count: usize,
    /// When the latest message was written, as RFC 3339 with an offset.
    pub latest_at: String,
    /// The start of the latest message's body, as plain text.
    pub snippet: String,
    /// Whether an active message from an agent is unread.
    pub unread: bool,
    /// Whether any message has a file.
    pub has_attachments: bool,
    /// Whether the thread is in the Inbox folder.
    pub in_inbox: bool,
    /// Whether the thread is in the Sent folder.
    pub in_sent: bool,
    /// Whether the thread is in the Archived folder.
    pub archived: bool,
}

/// A whole thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailThread {
    /// The root message's ref.
    pub thread_id: String,
    /// The root message's title.
    pub subject: String,
    /// Every agent in the thread, by folder name.
    pub participants: Vec<String>,
    /// The messages, the root first, then oldest first.
    pub messages: Vec<MailMessage>,
    /// The agents a reply goes to.
    pub recipients_for_reply: Vec<String>,
}

/// One message of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailMessage {
    /// The message's ref. A ref to any copy of it finds its thread.
    #[serde(rename = "ref")]
    #[ts(rename = "ref")]
    pub item_ref: String,
    /// Who wrote it.
    pub from: MailSender,
    /// The agents a message of the user's went to; empty for an agent's.
    pub to: Vec<String>,
    /// When it was written, as RFC 3339 with an offset.
    pub at: String,
    /// Its title.
    pub title: String,
    /// Its body, as markdown.
    pub body: String,
    /// Its files.
    pub attachments: Vec<MailAttachment>,
    /// The ref of the message it answers.
    pub in_reply_to: Option<String>,
    /// Whether the user opened it; always true for the user's own.
    pub read: bool,
    /// Whether the user archived it.
    pub archived: bool,
    /// For a message of the user's, whether each recipient has seen its copy.
    pub seen_by: Vec<MailSeen>,
}

/// Who wrote a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MailSender {
    /// The user.
    User,
    /// An agent, by folder name.
    Agent {
        /// The agent's folder name.
        agent: String,
    },
}

/// Whether one recipient has seen its copy of a message of the user's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailSeen {
    /// The recipient's folder name.
    pub agent: String,
    /// Whether its copy is read or archived.
    pub seen: bool,
}

/// One file of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailAttachment {
    /// The file's name.
    pub filename: String,
    /// The file's size in bytes.
    #[ts(type = "number")]
    pub size: u64,
    /// The file's MIME type, from its extension.
    pub content_type: String,
    /// Where to fetch the file's bytes.
    pub url: String,
    /// The agent whose workspace holds the file.
    pub agent: String,
    /// The file's path in that workspace, for the file panel.
    pub path: String,
}

/// The answer to `POST /api/hub/mail/send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailSendResponse {
    /// The thread the message started or joined.
    pub thread_id: String,
}

/// The answer to `POST /api/hub/mail/threads/{ref}/reply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailReplyResponse {
    /// The reply as the thread now shows it.
    pub message: MailMessage,
}

impl MailThreadSummary {
    /// The list entry of `thread`.
    #[must_use]
    pub fn of(thread: &domain::MailThread) -> Self {
        Self {
            thread_id: thread.thread_id().to_string(),
            subject: thread.subject().to_string(),
            participants: thread.participants().to_vec(),
            message_count: thread.message_count(),
            latest_at: residuum_core::time::format_rfc3339(&thread.latest_at()),
            snippet: plain_preview_within(&thread.latest().body, SNIPPET_CHARS),
            unread: thread.unread(),
            has_attachments: thread.has_attachments(),
            in_inbox: thread.in_inbox(),
            in_sent: thread.in_sent(),
            archived: thread.archived(),
        }
    }
}

impl MailThread {
    /// The whole of `thread`.
    #[must_use]
    pub fn of(thread: &domain::MailThread) -> Self {
        Self {
            thread_id: thread.thread_id().to_string(),
            subject: thread.subject().to_string(),
            participants: thread.participants().to_vec(),
            messages: thread.messages().map(MailMessage::of).collect(),
            recipients_for_reply: thread.recipients_for_reply().to_vec(),
        }
    }
}

impl MailMessage {
    /// `message` as the web sees it.
    #[must_use]
    pub fn of(message: &domain::MailMessage) -> Self {
        Self {
            item_ref: message.item_ref.to_string(),
            from: match &message.from {
                domain::MailSender::User => MailSender::User,
                domain::MailSender::Agent(agent) => MailSender::Agent {
                    agent: agent.clone(),
                },
            },
            to: message.to.clone(),
            at: residuum_core::time::format_rfc3339(&message.at),
            title: message.title.clone(),
            body: message.body.clone(),
            attachments: message.attachments.iter().map(MailAttachment::of).collect(),
            in_reply_to: message.in_reply_to.as_ref().map(ToString::to_string),
            read: message.read,
            archived: message.archived,
            seen_by: message
                .seen_by
                .iter()
                .map(|seen| MailSeen {
                    agent: seen.agent.clone(),
                    seen: seen.seen,
                })
                .collect(),
        }
    }
}

impl MailAttachment {
    /// `attachment` with the URL that serves its bytes.
    #[must_use]
    pub fn of(attachment: &domain::MailAttachment) -> Self {
        Self {
            filename: attachment.filename.clone(),
            size: attachment.size,
            content_type: attachment.content_type.clone(),
            url: attachment_url(&attachment.item, attachment.index),
            agent: attachment.agent().to_string(),
            path: attachment.path.to_string_lossy().replace('\\', "/"),
        }
    }
}

/// Where the bytes of attachment `index` of `item` are served, active or
/// archived: `/api/agents/{agent}/inbox/{id}/attachments/{index}` for a
/// user-inbox item, `/api/agents/{agent}/inbox/agent/{id}/attachments/{index}`
/// for an agent-inbox item.
#[must_use]
pub fn attachment_url(item: &InboxRef, index: usize) -> String {
    let agent = path_segment(item.agent());
    let id = path_segment(item.id());
    match item.kind() {
        InboxKind::User => format!("/api/agents/{agent}/inbox/{id}/attachments/{index}"),
        InboxKind::Agent => format!("/api/agents/{agent}/inbox/agent/{id}/attachments/{index}"),
    }
}

/// `value` escaped to be one segment of a URL path: every byte but the
/// unreserved characters of RFC 3986 percent-encoded.
fn path_segment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            for nibble in [byte >> 4, byte & 0x0F] {
                let digit = char::from_digit(u32::from(nibble), 16).unwrap_or('0');
                escaped.push(digit.to_ascii_uppercase());
            }
        }
    }
    escaped
}

/// The deep link into the Inbox that opens the thread holding `item`.
#[must_use]
pub fn thread_link(item: &InboxRef) -> String {
    residuum_net::browser_landing::app_path("/inbox", &[("thread", &item.to_string())])
}

/// Every agent's unread active user-inbox items added up: the app badge.
pub async fn unread_total(directory: &dyn AgentDirectory) -> u32 {
    let mut total: u32 = 0;
    for mailbox in AgentMailboxes(directory).mailboxes() {
        total = total.saturating_add(count_unread(&mailbox.agent, &mailbox.dir).await);
    }
    total
}

/// How many active items in the agent's user inbox the user hasn't opened.
/// An inbox that can't be read counts as none and is logged.
pub(crate) async fn count_unread(agent: &str, agent_dir: &Path) -> u32 {
    let dir = InboxKind::User.items_dir(&WorkspaceLayout::new(agent_dir), false);
    match residuum_workspace::inbox::list_items_or_empty(&dir).await {
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

/// The title and body of an agent's active user-inbox item `id`, for the
/// words of a push. `Ok(None)` when the active inbox no longer holds it: the
/// user already moved it.
///
/// # Errors
/// When the agent is unknown, `id` is not an item id, or the item's file
/// can't be read; the message says which.
pub async fn active_text(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let item = InboxRef::new(agent, InboxKind::User, id)?;
    let Some(mailbox) = AgentMailboxes(directory).mailbox(item.agent()) else {
        anyhow::bail!("no agent named '{agent}'");
    };
    let path = InboxKind::User
        .items_dir(&WorkspaceLayout::new(&mailbox.dir), false)
        .join(format!("{}.json", item.id()));
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let loaded = residuum_workspace::inbox::load_item(&path).await?;
    Ok(Some((loaded.title, loaded.body)))
}

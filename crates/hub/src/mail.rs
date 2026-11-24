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

pub const SNIPPET_CHARS: usize = 140;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailThreadPage {
    pub threads: Vec<MailThreadSummary>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire shape the mail contract names: each flag is its own JSON field the thread list reads"
)]
pub struct MailThreadSummary {
    pub thread_id: String,
    pub subject: String,
    pub participants: Vec<String>,
    #[ts(type = "number")]
    pub message_count: usize,
    pub latest_at: String,
    pub snippet: String,
    pub unread: bool,
    pub has_attachments: bool,
    pub in_inbox: bool,
    pub in_sent: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailThread {
    pub thread_id: String,
    pub subject: String,
    pub participants: Vec<String>,
    pub messages: Vec<MailMessage>,
    pub recipients_for_reply: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailMessage {
    #[serde(rename = "ref")]
    #[ts(rename = "ref")]
    pub item_ref: String,
    pub from: MailSender,
    pub to: Vec<String>,
    pub at: String,
    pub title: String,
    pub body: String,
    pub attachments: Vec<MailAttachment>,
    pub in_reply_to: Option<String>,
    pub read: bool,
    pub archived: bool,
    pub seen_by: Vec<MailSeen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MailSender {
    User,
    Agent { agent: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailSeen {
    pub agent: String,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailAttachment {
    pub filename: String,
    #[ts(type = "number")]
    pub size: u64,
    pub content_type: String,
    pub url: String,
    pub agent: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailSendResponse {
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct MailReplyResponse {
    pub message: MailMessage,
}

impl MailThreadSummary {
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

#[must_use]
pub fn attachment_url(item: &InboxRef, index: usize) -> String {
    let agent = residuum_core::util::percent_encode_unreserved(item.agent().as_bytes());
    let id = residuum_core::util::percent_encode_unreserved(item.id().as_bytes());
    match item.kind() {
        InboxKind::User => format!("/api/agents/{agent}/inbox/{id}/attachments/{index}"),
        InboxKind::Agent => format!("/api/agents/{agent}/inbox/agent/{id}/attachments/{index}"),
    }
}

#[must_use]
pub fn thread_link(item: &InboxRef) -> String {
    residuum_net::browser_landing::app_path("/inbox", &[("thread", &item.to_string())])
}

pub async fn unread_total(directory: &dyn AgentDirectory) -> u32 {
    let mut total: u32 = 0;
    for mailbox in AgentMailboxes(directory).mailboxes() {
        total = total.saturating_add(count_unread(&mailbox.agent, &mailbox.dir).await);
    }
    total
}

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

pub async fn active_text(
    directory: &dyn AgentDirectory,
    agent: &str,
    id: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let item = InboxRef::new(agent, InboxKind::User, id)?;
    let Some(mailbox) = AgentMailboxes(directory).mailbox(item.agent()) else {
        anyhow::bail!("no agent named '{agent}'");
    };
    let found =
        residuum_workspace::inbox::locate_item(&WorkspaceLayout::new(&mailbox.dir), &item).await?;
    Ok(found
        .filter(|found| !found.archived)
        .map(|found| (found.item.title, found.item.body)))
}

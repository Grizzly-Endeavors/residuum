//! The user-inbox tools: `user_inbox_add` writes an item to the user,
//! `user_inbox_list` lists every agent's items.

use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use residuum_agent::Tool;
use residuum_bus::{Publisher, UserInboxAddedEvent, topics};
use residuum_inference::{ToolDefinition, ToolError, ToolResult};
use residuum_workspace::inbox::mail::{
    MailIndex, MailMessage, MailSender, MailThread, ThreadFilter, reply_subject,
};
use residuum_workspace::inbox::{self, InboxKind};

use super::mail::{MailAccess, OWN_INBOX_ONLY_NOTE, TIME_FORMAT, parse_item_ref};

// ─── user_inbox_add ─────────────────────────────────────────────────────────

/// Tool for adding items to the user's inbox.
pub struct UserInboxAddTool {
    mail: MailAccess,
    /// Where each saved item is announced, so the hub learns of it without
    /// waiting for a file change. `None` announces nothing.
    publisher: Option<Publisher>,
}

impl UserInboxAddTool {
    /// Create a new `UserInboxAddTool` writing to the user inbox of the
    /// agent `mail` belongs to.
    #[must_use]
    pub fn new(mail: MailAccess) -> Self {
        Self {
            mail,
            publisher: None,
        }
    }

    /// Announce every saved item on `publisher`'s bus.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Publisher) -> Self {
        self.publisher = Some(publisher);
        self
    }

    /// The message `raw` names, found in any agent's mail.
    async fn reply_target(&self, raw: &str) -> Result<ReplyTarget, ToolError> {
        let item_ref =
            parse_item_ref(raw, self.mail.own_agent(), InboxKind::User).map_err(|e| {
                ToolError::InvalidArguments(format!(
                    "in_reply_to '{raw}' is not a valid inbox ref: {e}"
                ))
            })?;
        let view = self.mail.view();
        let index = MailIndex::load(view.directory.as_ref()).await;
        let Some(thread) = index.thread_containing(&item_ref) else {
            let mut message = format!(
                "in_reply_to: no mail message {item_ref} was found. Use user_inbox_list or inbox_read to find the ref of the message to answer."
            );
            if !view.every_agent {
                message.push(' ');
                message.push_str(OWN_INBOX_ONLY_NOTE);
            }
            return Err(ToolError::InvalidArguments(message));
        };
        Ok(ReplyTarget {
            item_ref,
            subject: thread.subject().to_string(),
        })
    }

    /// Tell the bus the item `item_id` was saved. A failure to announce
    /// leaves the item saved: the hub still sees the file appear.
    async fn announce(&self, item_id: &str) {
        let Some(publisher) = &self.publisher else {
            return;
        };
        if let Err(e) = publisher
            .publish(
                topics::UserInbox,
                UserInboxAddedEvent {
                    item_id: item_id.to_string(),
                },
            )
            .await
        {
            tracing::warn!(error = %e, item_id, "failed to announce a new user inbox item");
        }
    }
}

/// The message a reply answers: its ref and its thread's subject.
struct ReplyTarget {
    item_ref: inbox::InboxRef,
    subject: String,
}

#[async_trait]
impl Tool for UserInboxAddTool {
    fn name(&self) -> &'static str {
        "user_inbox_add"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Add a new item to the user's inbox. Use this to explicitly send notes, reminders, or items for the human user to review later. Set in_reply_to to answer a message in its mail thread instead of starting a new one.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A short summary of the item. Required unless in_reply_to is set; a reply without one is titled \"Re: <thread subject>\"."
                    },
                    "body": {
                        "type": "string",
                        "description": "The detailed content of the item"
                    },
                    "in_reply_to": {
                        "type": "string",
                        "description": "Optional. The ref (<agent>/<user|agent>/<id>, as inbox_read and user_inbox_list show it) of the message this item answers, which puts it in that message's thread. A bare id names an item in your own user inbox."
                    },
                    "attachments": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional. Paths to files you have already written to disk that should be attached to this item — e.g. a report, screenshot, or export a background task produced. Each file is copied into the item's own storage, so it's safe even if the source file is later moved or deleted. Omit or leave empty if there's nothing to attach."
                    }
                },
                "required": ["body"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let body = crate::require_str(&arguments, "body")?;
        let reply = match arguments.get("in_reply_to").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => Some(self.reply_target(raw).await?),
            _ => None,
        };
        let given_title = arguments
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty());
        let title = match (given_title, &reply) {
            (Some(title), _) => title.to_string(),
            (None, Some(reply)) => reply_subject(&reply.subject),
            (None, None) => {
                return Err(ToolError::InvalidArguments(
                    "title is required unless in_reply_to is set".to_string(),
                ));
            }
        };

        let attachment_paths: Vec<PathBuf> = arguments
            .get("attachments")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();

        let layout = self.mail.own_layout();
        let user_inbox_dir = layout.user_inbox_dir();
        tokio::fs::create_dir_all(&user_inbox_dir)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, dir = %user_inbox_dir.display(), "failed to create the user inbox directory");
                ToolError::Execution(format!(
                    "failed to add item to user inbox: could not create {}: {e}",
                    user_inbox_dir.display()
                ))
            })?;
        let (filename, failures) = inbox::add_user_inbox_item(
            &user_inbox_dir,
            &layout.user_inbox_attachments_dir(),
            inbox::NewUserInboxItem {
                title: &title,
                body,
                source: "agent",
                in_reply_to: reply.as_ref().map(|r| &r.item_ref),
                attachments: &attachment_paths,
            },
            self.mail.own_timezone(),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, attachment_count = attachment_paths.len(), "failed to add item to user inbox");
            ToolError::Execution(format!("failed to add item to user inbox: {e}"))
        })?;

        let id = filename.trim_end_matches(".json");
        self.announce(id).await;
        let succeeded = attachment_paths.len() - failures.len();
        let mut message = if attachment_paths.is_empty() {
            format!("Added item to user inbox with ID: {id}")
        } else {
            format!("Added item to user inbox with ID: {id} ({succeeded} attachment(s) copied)")
        };
        if let Some(reply) = &reply {
            _ = write!(
                message,
                "\nIn reply to {} in the thread \"{}\", titled \"{title}\".",
                reply.item_ref, reply.subject
            );
        }
        if !failures.is_empty() {
            message
                .push_str("\n\nSome attachments could not be copied (the item was still added):");
            for failure in &failures {
                message.push_str("\n- ");
                message.push_str(failure);
            }
        }
        Ok(ToolResult::success(message))
    }
}

// ─── user_inbox_list ────────────────────────────────────────────────────────

/// The most items one `user_inbox_list` call shows.
const USER_INBOX_LIST_CAP: usize = 100;

/// Tool for listing every agent's user-inbox items.
pub struct UserInboxListTool {
    mail: MailAccess,
}

impl UserInboxListTool {
    /// Create a new `UserInboxListTool` for the agent `mail` belongs to.
    #[must_use]
    pub fn new(mail: MailAccess) -> Self {
        Self { mail }
    }
}

/// One line of the listing for `message`, an agent's message to the user.
fn list_line(agent: &str, message: &MailMessage) -> String {
    let mut status = vec![if message.read { "read" } else { "unread" }];
    if message.archived {
        status.push("archived");
    }
    let mut line = format!(
        "  [{}] {} — {} ({agent}, {})",
        status.join(", "),
        message.item_ref,
        message.title,
        message.at.format(TIME_FORMAT)
    );
    if let Some(parent) = &message.in_reply_to {
        _ = write!(line, ", in reply to {parent}");
    }
    line
}

#[async_trait]
impl Tool for UserInboxListTool {
    fn name(&self) -> &'static str {
        "user_inbox_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: format!(
                "List the items every agent has added to the user's inbox, newest first: each item's ref, title, agent, time, read and archived state, and the message it answers. Read-only. Check it before adding an item so you don't repeat what an agent already sent; read one with inbox_read and its ref. Shows at most {USER_INBOX_LIST_CAP} items."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "description": "Only this agent's items (its folder name, the first part of a ref)."
                    },
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include items the user archived (default false)."
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let agent_filter = arguments
            .get("agent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|a| !a.is_empty());
        let include_archived = arguments
            .get("include_archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let view = self.mail.view();
        let mut notes = Vec::new();
        if !view.every_agent {
            notes.push(OWN_INBOX_ONLY_NOTE.to_string());
        }
        if let Some(agent) = agent_filter
            && view.directory.mailbox(agent).is_none()
        {
            let mut message = format!("there is no agent named '{agent}'");
            for note in &notes {
                message.push_str("\n\n");
                message.push_str(note);
            }
            return Ok(ToolResult::error(message));
        }

        let index = MailIndex::load(view.directory.as_ref()).await;
        for problem in index.problems() {
            notes.push(format!(
                "Note: {}'s inbox at {} could not be read, so its items are missing: {}",
                problem.agent,
                problem.dir.display(),
                problem.error
            ));
        }
        let mut items: Vec<(&str, &MailMessage)> = index
            .threads(&ThreadFilter::default())
            .flat_map(MailThread::messages)
            .filter_map(|message| match &message.from {
                MailSender::Agent(agent) => Some((agent.as_str(), message)),
                MailSender::User => None,
            })
            .filter(|(agent, _)| agent_filter.is_none_or(|wanted| wanted == *agent))
            .filter(|(_, message)| include_archived || !message.archived)
            .collect();
        items.sort_by(|(_, a), (_, b)| b.at.cmp(&a.at).then_with(|| a.item_ref.cmp(&b.item_ref)));

        let total = items.len();
        let mut lines = Vec::new();
        if total == 0 {
            lines.push("No user inbox items found.".to_string());
        } else {
            lines.push(format!("{total} user inbox item(s), newest first:"));
            lines.extend(
                items
                    .iter()
                    .take(USER_INBOX_LIST_CAP)
                    .map(|(agent, message)| list_line(agent, message)),
            );
            if total > USER_INBOX_LIST_CAP {
                lines.push(format!(
                    "Showing the {USER_INBOX_LIST_CAP} newest of {total} items. Narrow the list with agent to see older ones."
                ));
            }
        }
        for note in notes {
            lines.push(String::new());
            lines.push(note);
        }
        Ok(ToolResult::success(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests;

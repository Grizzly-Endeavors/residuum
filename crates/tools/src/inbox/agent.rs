//! The agent-inbox tools: list, read, archive and restore this agent's
//! inbox items. `inbox_read` also reads any agent's item by ref, with the
//! earlier messages of its mail thread.

use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use residuum_agent::Tool;
use residuum_inference::{ToolDefinition, ToolError, ToolResult};
use residuum_workspace::inbox::mail::{
    MailCursor, MailIndex, MailMessage, MailSender, is_user_mail_source,
};
use residuum_workspace::inbox::{self, InboxKind, InboxRef};

use super::mail::{
    LocateError, MailAccess, OWN_INBOX_ONLY_NOTE, TIME_FORMAT, locate, parse_item_ref,
};
use super::paging::{
    DEFAULT_LIMIT, MAX_LIMIT, PageRequest, before_schema, limit_schema, more_note, query_schema,
};

// ─── inbox_list ─────────────────────────────────────────────────────────────

/// The parameters of `inbox_list` a next-page call repeats.
const LIST_FILTERS: &str = "query, unread_only and archived";

/// Tool for listing this agent's agent-inbox items.
pub struct InboxListTool {
    mail: MailAccess,
}

impl InboxListTool {
    /// Create a new `InboxListTool` for the agent `mail` belongs to.
    #[must_use]
    pub fn new(mail: MailAccess) -> Self {
        Self { mail }
    }
}

#[async_trait]
impl Tool for InboxListTool {
    fn name(&self) -> &'static str {
        "inbox_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: format!(
                "List inbox items, newest first. Shows unread/read status, title, source, and timestamp for each item. Find an item by its words with query. Lists {DEFAULT_LIMIT} items at a time (limit, at most {MAX_LIMIT}); when more match, the result ends with the before cursor that lists the next page."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "unread_only": {
                        "type": "boolean",
                        "description": "Only show unread items (default false). Ignored when archived is true — archived items are always read."
                    },
                    "archived": {
                        "type": "boolean",
                        "description": "List archived items instead of active ones (default false). Use with inbox_restore to bring an item back."
                    },
                    "query": query_schema(),
                    "limit": limit_schema(),
                    "before": before_schema()
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let request = PageRequest::parse(&arguments)?;
        let archived = arguments
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let unread_only = !archived
            && arguments
                .get("unread_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);

        let layout = self.mail.own_layout();
        let dir = if archived {
            layout.agent_inbox_archive_dir()
        } else {
            layout.agent_inbox_dir()
        };
        let items = inbox::list_items(&dir).await.map_err(|e| {
            tracing::error!(error = %e, archived, "failed to list inbox items");
            ToolError::Execution(format!("failed to list inbox items: {e}"))
        })?;

        let kind = if archived { "archived" } else { "inbox" };
        let tz = self.mail.own_timezone();
        let mut entries = Vec::new();
        let mut unlistable = Vec::new();
        for (filename, item) in items {
            let unwanted = (unread_only && item.read)
                || request
                    .query()
                    .is_some_and(|query| !item.matches_text(query));
            if unwanted {
                continue;
            }
            match InboxRef::new(self.mail.own_agent(), InboxKind::Agent, &filename) {
                Ok(item_ref) => {
                    let at = residuum_core::time::local_to_instant(tz, item.timestamp);
                    entries.push((MailCursor::new(at, item_ref), (filename, item)));
                }
                Err(e) => {
                    tracing::warn!(item = %filename, error = %e, "inbox item has no valid ref; left out of the listing");
                    unlistable.push(filename);
                }
            }
        }

        let page = request.page(entries);
        let mut lines: Vec<String> = Vec::new();
        if page.items.is_empty() {
            lines.push(request.empty_message(kind));
        } else {
            lines.push(request.header(page.total, kind));
            for (filename, item) in &page.items {
                let status = if item.read { "read" } else { "unread" };
                let ts = item.timestamp.format(TIME_FORMAT);
                lines.push(format!(
                    "  [{status}] {filename} — {} ({}, {ts})",
                    item.title, item.source
                ));
            }
            if let Some(next) = &page.next {
                lines.push(more_note(page.items.len(), page.total, next, LIST_FILTERS));
            }
        }
        if !unlistable.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "Note: {} {kind} item(s) were left out because their file names can't be used as an item id: {}.",
                unlistable.len(),
                unlistable.join(", ")
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── inbox_read ─────────────────────────────────────────────────────────────

/// Tool for reading a single inbox item, with the earlier messages of its
/// mail thread. Reading an item of this agent's own active agent inbox marks
/// it read.
pub struct InboxReadTool {
    mail: MailAccess,
}

impl InboxReadTool {
    /// Create a new `InboxReadTool` for the agent `mail` belongs to.
    #[must_use]
    pub fn new(mail: MailAccess) -> Self {
        Self { mail }
    }
}

/// Who wrote `message`, as the thread listing names them.
fn sender_line(message: &MailMessage) -> String {
    match &message.from {
        MailSender::User if message.to.is_empty() => "the user".to_string(),
        MailSender::User => format!("the user, to {}", message.to.join(", ")),
        MailSender::Agent(agent) => agent.clone(),
    }
}

/// The item's own lines: its ref, header fields, attachments and body.
/// Attachment paths of another agent's item are absolute, since they are
/// relative to that agent's workspace.
fn item_section(item_ref: &InboxRef, located: &super::mail::LocatedItem, own: bool) -> String {
    let item = &located.item;
    let mut output = format!(
        "Ref: {item_ref}\nTitle: {}\nSource: {}\nTime: {}\n",
        item.title,
        item.source,
        item.timestamp.format(TIME_FORMAT)
    );
    if located.archived {
        output.push_str("Archived: yes\n");
    }
    if let Some(parent) = &item.in_reply_to {
        _ = writeln!(output, "In reply to: {parent}");
    }
    if !item.attachments.is_empty() {
        let paths: Vec<String> = item
            .attachments
            .iter()
            .map(|p| {
                if own {
                    p.display().to_string()
                } else {
                    located.workspace_dir.join(p).display().to_string()
                }
            })
            .collect();
        _ = writeln!(output, "Attachments: {}", paths.join(", "));
    }
    _ = write!(output, "\n{}", item.body);
    output
}

/// The thread lines for the message `item_ref` names: the earlier messages,
/// oldest first, and the refs of the later ones. Empty when the item is not
/// mail or is the thread's only message.
fn thread_section(index: &MailIndex, item_ref: &InboxRef) -> String {
    let Some(thread) = index.thread_containing(item_ref) else {
        return String::new();
    };
    if thread.message_count() < 2 {
        return String::new();
    }
    let messages: Vec<&MailMessage> = thread.messages().collect();
    let position = messages
        .iter()
        .position(|m| m.copies.contains(item_ref))
        .unwrap_or_default();
    let (earlier, rest) = messages.split_at(position);
    let later: Vec<String> = rest
        .iter()
        .skip(1)
        .map(|m| m.item_ref.to_string())
        .collect();

    let mut output = format!(
        "\n\nThread \"{}\": message {} of {}.",
        thread.subject(),
        position + 1,
        messages.len()
    );
    if !earlier.is_empty() {
        _ = write!(output, "\n\nEarlier messages, oldest first:");
        for message in earlier {
            _ = write!(
                output,
                "\n\n--- From {} · {} · Ref: {}\nTitle: {}\n{}",
                sender_line(message),
                message.at.format(TIME_FORMAT),
                message.item_ref,
                message.title,
                message.body
            );
        }
    }
    if !later.is_empty() {
        _ = write!(
            output,
            "\n\nLater messages (read with inbox_read): {}",
            later.join(", ")
        );
    }
    output
}

#[async_trait]
impl Tool for InboxReadTool {
    fn name(&self) -> &'static str {
        "inbox_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Read a single inbox item: an item of your own agent inbox by filename stem, or any agent's agent-inbox or user-inbox item by its ref. Shows the item's ref and full content and, when it is part of a mail thread with the user, the thread's earlier messages oldest first. Marks an item of your own agent inbox as read; never changes another agent's item or a user-inbox item.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Filename stem of an item in your own agent inbox (without .json extension), or a full ref <agent>/<user|agent>/<id> as user_inbox_list and inbox_read show it"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let raw = crate::require_str(&arguments, "id")?;
        let item_ref =
            parse_item_ref(raw, self.mail.own_agent(), InboxKind::Agent).map_err(|e| {
                ToolError::InvalidArguments(format!("'{raw}' is not an inbox item id or ref: {e}"))
            })?;
        let own = item_ref.agent() == self.mail.own_agent();

        let view = self.mail.view();
        let mut located = locate(view.directory.as_ref(), &item_ref)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, item = %item_ref, "failed to read inbox item");
                let mut message = format!("failed to read inbox item '{raw}': {e}");
                if matches!(e, LocateError::UnknownAgent(_)) && !view.every_agent {
                    message.push(' ');
                    message.push_str(OWN_INBOX_ONLY_NOTE);
                }
                ToolError::Execution(message)
            })?;

        if own && item_ref.kind() == InboxKind::Agent && !located.archived {
            located.item = inbox::mark_read(&located.items_dir, item_ref.id())
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, item = %item_ref, "failed to mark inbox item read");
                    ToolError::Execution(format!("failed to read inbox item '{raw}': {e}"))
                })?;
        }

        let mut output = item_section(&item_ref, &located, own);
        // An agent-inbox item that isn't user mail (a pulse, webhook or
        // artifact item) is in no thread, so there is no mail to load for it.
        let can_be_mail =
            item_ref.kind() == InboxKind::User || is_user_mail_source(&located.item.source);
        if can_be_mail {
            let index = MailIndex::load(view.directory.as_ref()).await;
            output.push_str(&thread_section(&index, &item_ref));
        }
        Ok(ToolResult::success(output))
    }
}

// ─── inbox_archive ──────────────────────────────────────────────────────────

/// Tool for archiving inbox items.
pub struct InboxArchiveTool {
    inbox_dir: PathBuf,
    archive_dir: PathBuf,
}

impl InboxArchiveTool {
    /// Create a new `InboxArchiveTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf, archive_dir: PathBuf) -> Self {
        Self {
            inbox_dir,
            archive_dir,
        }
    }
}

#[async_trait]
impl Tool for InboxArchiveTool {
    fn name(&self) -> &'static str {
        "inbox_archive"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Archive one or more inbox items by filename stem. Moves them to the archive directory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filename stems of inbox items to archive"
                    }
                },
                "required": ["ids"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let ids = arguments
            .get("ids")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidArguments("ids is required".to_string()))?;

        if ids.is_empty() {
            return Ok(ToolResult::error("ids must not be empty"));
        }

        let mut archived = Vec::new();
        let mut errors = Vec::new();

        for id_val in ids {
            let Some(id) = id_val.as_str() else {
                errors.push("non-string value in ids array".to_string());
                continue;
            };

            match inbox::archive_item(&self.inbox_dir, &self.archive_dir, id).await {
                Ok(()) => archived.push(id.to_string()),
                Err(e) => {
                    tracing::warn!(error = %e, id = %id, "failed to archive inbox item");
                    errors.push(format!("{id}: {e}"));
                }
            }
        }

        let mut parts: Vec<String> = Vec::new();
        if !archived.is_empty() {
            parts.push(format!(
                "Archived {} item(s): {}",
                archived.len(),
                archived.join(", ")
            ));
        }
        if !errors.is_empty() {
            parts.push(format!(
                "Failed to archive {} item(s): {}",
                errors.len(),
                errors.join("; ")
            ));
        }
        let output = parts.join("\n");

        if errors.is_empty() {
            Ok(ToolResult::success(output))
        } else if archived.is_empty() {
            Ok(ToolResult::error(output))
        } else {
            // Partial success — report as success with error details
            Ok(ToolResult::success(output))
        }
    }
}

// ─── inbox_restore ──────────────────────────────────────────────────────────

/// Tool for restoring archived inbox items back to the active inbox.
pub struct InboxRestoreTool {
    inbox_dir: PathBuf,
    archive_dir: PathBuf,
}

impl InboxRestoreTool {
    /// Create a new `InboxRestoreTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf, archive_dir: PathBuf) -> Self {
        Self {
            inbox_dir,
            archive_dir,
        }
    }
}

#[async_trait]
impl Tool for InboxRestoreTool {
    fn name(&self) -> &'static str {
        "inbox_restore"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Restore one or more archived inbox items by filename stem. Moves them back to the active inbox. Use inbox_list with archived=true to find the filename stem to restore.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filename stems of archived inbox items to restore"
                    }
                },
                "required": ["ids"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let ids = arguments
            .get("ids")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidArguments("ids is required".to_string()))?;

        if ids.is_empty() {
            return Ok(ToolResult::error("ids must not be empty"));
        }

        let mut restored = Vec::new();
        let mut errors = Vec::new();

        for id_val in ids {
            let Some(id) = id_val.as_str() else {
                errors.push("non-string value in ids array".to_string());
                continue;
            };

            match inbox::restore_item(&self.archive_dir, &self.inbox_dir, id).await {
                Ok(()) => restored.push(id.to_string()),
                Err(e) => {
                    tracing::warn!(error = %e, id = %id, "failed to restore inbox item");
                    errors.push(format!("{id}: {e}"));
                }
            }
        }

        let mut parts: Vec<String> = Vec::new();
        if !restored.is_empty() {
            parts.push(format!(
                "Restored {} item(s): {}",
                restored.len(),
                restored.join(", ")
            ));
        }
        if !errors.is_empty() {
            parts.push(format!(
                "Failed to restore {} item(s): {}",
                errors.len(),
                errors.join("; ")
            ));
        }
        let output = parts.join("\n");

        if errors.is_empty() {
            Ok(ToolResult::success(output))
        } else if restored.is_empty() {
            Ok(ToolResult::error(output))
        } else {
            // Partial success — report as success with error details
            Ok(ToolResult::success(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::test_fixture::{Item, MailHub, listed_ids, next_cursor, pages};
    use residuum_config::WorkspaceLayout;

    /// The workspace of an agent named `atlas` under `dir`, and its mail
    /// access with no hub directory.
    fn atlas(dir: &tempfile::TempDir) -> (WorkspaceLayout, MailAccess) {
        let layout = WorkspaceLayout::new(dir.path().join("atlas"));
        let mail = MailAccess::new(None, &layout, chrono_tz::UTC);
        (layout, mail)
    }

    /// What `tool` answers to `arguments`.
    async fn output_of(tool: &InboxListTool, arguments: serde_json::Value) -> String {
        tool.execute(arguments).await.unwrap().output
    }

    #[test]
    fn definitions_have_matching_names() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, mail) = atlas(&dir);
        let inbox_dir = layout.agent_inbox_dir();
        let archive = layout.agent_inbox_archive_dir();

        let list = InboxListTool::new(mail.clone());
        assert_eq!(list.definition().name, list.name());
        assert_eq!(list.name(), "inbox_list");

        let read = InboxReadTool::new(mail);
        assert_eq!(read.definition().name, read.name());
        assert_eq!(read.name(), "inbox_read");

        let archive_tool = InboxArchiveTool::new(inbox_dir.clone(), archive.clone());
        assert_eq!(archive_tool.definition().name, archive_tool.name());
        assert_eq!(archive_tool.name(), "inbox_archive");

        let restore_tool = InboxRestoreTool::new(inbox_dir, archive);
        assert_eq!(restore_tool.definition().name, restore_tool.name());
        assert_eq!(restore_tool.name(), "inbox_restore");
    }

    #[tokio::test]
    async fn inbox_add_list_read_archive_flow() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, mail) = atlas(&dir);
        let inbox_dir = layout.agent_inbox_dir();
        let archive_dir = layout.agent_inbox_archive_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        // Add an item via quick_add directly
        let filename = residuum_workspace::inbox::quick_add(
            &inbox_dir,
            "test item",
            "test body content",
            "test",
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let filename = filename.trim_end_matches(".json").to_string();

        // List items
        let list_tool = InboxListTool::new(mail.clone());
        let list_result = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!list_result.is_error, "list should succeed");
        assert!(
            list_result.output.contains("[unread]"),
            "should show unread: {}",
            list_result.output
        );
        assert!(
            list_result.output.contains("test item"),
            "should show title: {}",
            list_result.output
        );

        // Read item (marks as read)
        let read_tool = InboxReadTool::new(mail);
        let read_result = read_tool
            .execute(serde_json::json!({"id": filename}))
            .await
            .unwrap();
        assert!(!read_result.is_error, "read should succeed");
        assert!(
            read_result.output.contains("test body content"),
            "should show body: {}",
            read_result.output
        );

        // List again — should show as read
        let list_after_read = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            list_after_read.output.contains("[read]"),
            "should now show read: {}",
            list_after_read.output
        );

        // List unread only — should be empty
        let list_unread = list_tool
            .execute(serde_json::json!({"unread_only": true}))
            .await
            .unwrap();
        assert!(
            list_unread.output.contains("No inbox items"),
            "unread_only should be empty: {}",
            list_unread.output
        );

        // Archive
        let archive_tool = InboxArchiveTool::new(inbox_dir.clone(), archive_dir.clone());
        let archive_result = archive_tool
            .execute(serde_json::json!({"ids": [filename]}))
            .await
            .unwrap();
        assert!(!archive_result.is_error, "archive should succeed");
        assert!(
            archive_result.output.contains("Archived 1 item(s)"),
            "should confirm archive: {}",
            archive_result.output
        );

        // List after archive — should be empty
        let list_after_archive = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            list_after_archive.output.contains("No inbox items"),
            "should be empty after archive: {}",
            list_after_archive.output
        );
    }

    #[tokio::test]
    async fn inbox_list_archived_and_restore_flow() {
        let dir = tempfile::tempdir().unwrap();
        let (layout, mail) = atlas(&dir);
        let inbox_dir = layout.agent_inbox_dir();
        let archive_dir = layout.agent_inbox_archive_dir();
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let filename = residuum_workspace::inbox::quick_add(
            &inbox_dir,
            "test item",
            "test body content",
            "test",
            chrono_tz::UTC,
        )
        .await
        .unwrap();
        let filename = filename.trim_end_matches(".json").to_string();

        let archive_tool = InboxArchiveTool::new(inbox_dir.clone(), archive_dir.clone());
        archive_tool
            .execute(serde_json::json!({"ids": [filename.clone()]}))
            .await
            .unwrap();

        let list_tool = InboxListTool::new(mail);

        // Archived items are visible via inbox_list(archived=true) …
        let list_archived = list_tool
            .execute(serde_json::json!({"archived": true}))
            .await
            .unwrap();
        assert!(
            list_archived.output.contains("test item"),
            "archived listing should show the item: {}",
            list_archived.output
        );

        // … and restoring it brings it back to the active inbox.
        let restore_tool = InboxRestoreTool::new(inbox_dir.clone(), archive_dir.clone());
        let restore_result = restore_tool
            .execute(serde_json::json!({"ids": [filename]}))
            .await
            .unwrap();
        assert!(!restore_result.is_error, "restore should succeed");
        assert!(
            restore_result.output.contains("Restored 1 item(s)"),
            "should confirm restore: {}",
            restore_result.output
        );

        let list_after_restore = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            list_after_restore.output.contains("test item"),
            "restored item should be back in the active inbox: {}",
            list_after_restore.output
        );
        let list_archived_after_restore = list_tool
            .execute(serde_json::json!({"archived": true}))
            .await
            .unwrap();
        assert!(
            list_archived_after_restore
                .output
                .contains("No archived items"),
            "archive should be empty after restore: {}",
            list_archived_after_restore.output
        );
    }

    #[tokio::test]
    async fn inbox_list_pages_visit_every_item_once_even_when_times_tie() {
        let hub = MailHub::new(&["atlas"]);
        for (id, hour) in [
            ("item-0", 8),
            ("item-1", 8),
            ("item-2", 8),
            ("item-3", 9),
            ("item-4", 9),
            ("item-5", 10),
            ("item-6", 10),
        ] {
            hub.write(Item::user_mail("atlas", id, "t", "b", hour))
                .await;
        }
        let tool = InboxListTool::new(hub.access("atlas"));

        let outputs = pages(&tool, serde_json::json!({"limit": 3})).await;

        let ids: Vec<Vec<String>> = outputs.iter().map(|text| listed_ids(text)).collect();
        assert_eq!(
            ids,
            [
                vec!["item-5", "item-6", "item-3"],
                vec!["item-4", "item-0", "item-1"],
                vec!["item-2"],
            ],
            "newest first, a tie by id, each item on exactly one page"
        );
        let [first, second, last] = outputs.as_slice() else {
            panic!("three pages: {outputs:?}");
        };
        assert!(
            first.starts_with("7 inbox item(s), newest first:"),
            "{first}"
        );
        assert!(
            first.contains("Showing 3 of 7. For the next page, call again with before=\""),
            "{first}"
        );
        assert!(
            first.contains("the same query, unread_only and archived."),
            "{first}"
        );
        assert!(
            second.starts_with("4 inbox item(s) after the cursor, newest first:"),
            "{second}"
        );
        assert!(second.contains("Showing 3 of 4."), "{second}");
        assert!(
            last.starts_with("1 inbox item(s) after the cursor, newest first:"),
            "{last}"
        );
        assert!(
            !last.contains("Showing"),
            "the last page has no next: {last}"
        );
    }

    #[tokio::test]
    async fn inbox_list_cursor_reads_the_stored_time_in_the_agents_timezone() {
        let hub = MailHub::new(&["atlas"]);
        hub.write(Item::user_mail("atlas", "early", "t", "b", 9))
            .await;
        hub.write(Item::user_mail("atlas", "late", "t", "b", 10))
            .await;
        let mail = MailAccess::new(None, &hub.layout("atlas"), chrono_tz::America::New_York);
        let tool = InboxListTool::new(mail);

        let out = tool
            .execute(serde_json::json!({"limit": 1}))
            .await
            .unwrap()
            .output;

        assert_eq!(listed_ids(&out), ["late"], "{out}");
        // 10:00 in New York on 2026-02-25 is 15:00 UTC.
        assert_eq!(
            next_cursor(&out).as_deref(),
            Some("1772031600000:atlas/agent/late"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn inbox_list_lists_fifty_by_default_and_clamps_the_limit_to_two_hundred() {
        let hub = MailHub::new(&["atlas"]);
        let total = MAX_LIMIT + 5;
        for n in 0..total {
            hub.write(Item::user_mail(
                "atlas",
                &format!("item-{n:03}"),
                "t",
                "b",
                8,
            ))
            .await;
        }
        let tool = InboxListTool::new(hub.access("atlas"));

        for (arguments, listed) in [
            (serde_json::json!({}), DEFAULT_LIMIT),
            (serde_json::json!({"limit": 3}), 3),
            (serde_json::json!({"limit": "2"}), 2),
            (serde_json::json!({"limit": MAX_LIMIT}), MAX_LIMIT),
            (serde_json::json!({"limit": 100_000}), MAX_LIMIT),
        ] {
            let out = tool.execute(arguments.clone()).await.unwrap().output;
            assert_eq!(listed_ids(&out).len(), listed, "{arguments}: {out}");
            assert!(
                out.contains(&format!("Showing {listed} of {total}.")),
                "{arguments}: {out}"
            );
        }
    }

    #[tokio::test]
    async fn inbox_list_refuses_a_zero_limit_a_bad_limit_and_a_foreign_cursor() {
        let hub = MailHub::new(&["atlas"]);
        let tool = InboxListTool::new(hub.access("atlas"));
        for (arguments, expected) in [
            (serde_json::json!({"limit": 0}), "limit must be at least 1"),
            (
                serde_json::json!({"limit": 1.5}),
                "limit must be a whole number",
            ),
            (
                serde_json::json!({"before": "nonsense"}),
                "the before cursor isn't one this list returned",
            ),
            (
                serde_json::json!({"before": "later:atlas/agent/x"}),
                "the before cursor isn't one this list returned",
            ),
        ] {
            let result = tool.execute(arguments.clone()).await;
            let Err(ToolError::InvalidArguments(message)) = result else {
                panic!("{arguments} is refused: {result:?}");
            };
            assert!(message.contains(expected), "{arguments}: {message}");
        }
    }

    #[tokio::test]
    async fn inbox_list_query_matches_title_body_and_attachment_filenames_ignoring_case() {
        let hub = MailHub::new(&["atlas"]);
        hub.write(Item::user_mail("atlas", "a1", "Budget review", "b", 8))
            .await;
        hub.write(Item::user_mail("atlas", "a2", "Notes", "the BUDGET is final", 9).read())
            .await;
        hub.write(
            Item::user_mail("atlas", "a3", "Files", "see attached", 10)
                .with_attachment("budget-2026.xlsx"),
        )
        .await;
        hub.write(Item::user_mail("atlas", "a4", "Other", "nothing", 11))
            .await;
        hub.write(Item::user_mail("atlas", "old1", "Old Budget", "b", 7).archived())
            .await;
        let tool = InboxListTool::new(hub.access("atlas"));

        let out = output_of(&tool, serde_json::json!({"query": "bUdGeT"})).await;
        assert_eq!(listed_ids(&out), ["a3", "a2", "a1"], "{out}");
        assert!(
            out.starts_with("3 inbox item(s) match, newest first:"),
            "{out}"
        );

        let unread = output_of(
            &tool,
            serde_json::json!({"query": "budget", "unread_only": true}),
        )
        .await;
        assert_eq!(listed_ids(&unread), ["a3", "a1"], "{unread}");

        let archived = output_of(
            &tool,
            serde_json::json!({"query": "budget", "archived": true}),
        )
        .await;
        assert_eq!(listed_ids(&archived), ["old1"], "{archived}");
        assert!(
            archived.starts_with("1 archived item(s) match, newest first:"),
            "{archived}"
        );

        let outputs = pages(&tool, serde_json::json!({"query": "budget", "limit": 2})).await;
        let ids: Vec<Vec<String>> = outputs.iter().map(|text| listed_ids(text)).collect();
        assert_eq!(ids, [vec!["a3", "a2"], vec!["a1"]]);

        let none = output_of(&tool, serde_json::json!({"query": "zebra"})).await;
        assert_eq!(none, "No inbox items match \"zebra\".");

        let blank = output_of(&tool, serde_json::json!({"query": "   "})).await;
        assert_eq!(listed_ids(&blank), ["a4", "a3", "a2", "a1"], "{blank}");
    }

    #[tokio::test]
    async fn inbox_list_says_which_items_have_names_it_cannot_list() {
        let hub = MailHub::new(&["atlas"]);
        hub.write(Item::user_mail("atlas", "fine", "Fine", "b", 9))
            .await;
        hub.write(Item::user_mail("atlas", "odd..name", "Odd", "b", 10))
            .await;
        let tool = InboxListTool::new(hub.access("atlas"));

        let out = tool.execute(serde_json::json!({})).await.unwrap().output;

        assert_eq!(listed_ids(&out), ["fine"], "{out}");
        assert!(
            out.ends_with("Note: 1 inbox item(s) were left out because their file names can't be used as an item id: odd..name."),
            "{out}"
        );
    }

    #[tokio::test]
    async fn inbox_restore_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let archive_dir = dir.path().join("archive/inbox");

        let tool = InboxRestoreTool::new(dir.path().to_path_buf(), archive_dir);
        let result = tool
            .execute(serde_json::json!({"ids": ["nonexistent"]}))
            .await
            .unwrap();
        assert!(result.is_error, "should error on nonexistent item");
        assert!(
            result.output.contains("Failed"),
            "should mention failure: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn inbox_restore_empty_ids() {
        let dir = tempfile::tempdir().unwrap();
        let archive_dir = dir.path().join("archive/inbox");

        let tool = InboxRestoreTool::new(dir.path().to_path_buf(), archive_dir);
        let result = tool.execute(serde_json::json!({"ids": []})).await.unwrap();
        assert!(result.is_error, "empty ids should error");
    }

    #[tokio::test]
    async fn inbox_archive_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let archive_dir = dir.path().join("archive/inbox");

        let tool = InboxArchiveTool::new(dir.path().to_path_buf(), archive_dir);
        let result = tool
            .execute(serde_json::json!({"ids": ["nonexistent"]}))
            .await
            .unwrap();
        assert!(result.is_error, "should error on nonexistent item");
        assert!(
            result.output.contains("Failed"),
            "should mention failure: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn inbox_archive_empty_ids() {
        let dir = tempfile::tempdir().unwrap();
        let archive_dir = dir.path().join("archive/inbox");

        let tool = InboxArchiveTool::new(dir.path().to_path_buf(), archive_dir);
        let result = tool.execute(serde_json::json!({"ids": []})).await.unwrap();
        assert!(result.is_error, "empty ids should error");
    }

    #[tokio::test]
    async fn inbox_read_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mail) = atlas(&dir);
        let tool = InboxReadTool::new(mail);
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing id should error");
    }

    #[tokio::test]
    async fn inbox_read_nonexistent_item() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mail) = atlas(&dir);
        let tool = InboxReadTool::new(mail);
        let result = tool
            .execute(serde_json::json!({"id": "does-not-exist"}))
            .await;
        let Err(ToolError::Execution(message)) = result else {
            panic!("reading a nonexistent item is an execution error: {result:?}");
        };
        assert!(
            message.contains("atlas/agent/does-not-exist") && message.contains("was found"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn inbox_read_rejects_a_malformed_ref() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mail) = atlas(&dir);
        let result = InboxReadTool::new(mail)
            .execute(serde_json::json!({"id": "atlas/letters/x"}))
            .await;
        assert!(
            matches!(result, Err(ToolError::InvalidArguments(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn inbox_read_by_stem_shows_the_ref_and_the_earlier_messages() {
        let hub = MailHub::new(&["atlas", "nova"]);
        hub.write(Item::agent_message(
            "nova",
            "plan",
            "Plan",
            "Draft attached.",
            9,
        ))
        .await;
        hub.write(
            Item::user_mail("atlas", "re-plan", "Re: Plan", "Looks good, ship it.", 10)
                .replying_to("nova/user/plan"),
        )
        .await;

        let result = InboxReadTool::new(hub.access("atlas"))
            .execute(serde_json::json!({"id": "re-plan"}))
            .await
            .unwrap();
        let out = &result.output;
        assert!(out.starts_with("Ref: atlas/agent/re-plan\n"), "{out}");
        assert!(out.contains("In reply to: nova/user/plan"), "{out}");
        assert!(out.contains("Looks good, ship it."), "{out}");
        assert!(out.contains("Thread \"Plan\": message 2 of 2."), "{out}");
        assert!(
            out.contains("--- From nova · 2026-02-25T09:00 · Ref: nova/user/plan\nTitle: Plan\nDraft attached."),
            "{out}"
        );
        assert!(
            hub.load("atlas", InboxKind::Agent, "re-plan").await.read,
            "own agent-inbox item is marked read"
        );
    }

    #[tokio::test]
    async fn inbox_read_by_another_agents_ref_marks_nothing_read() {
        let hub = MailHub::new(&["atlas", "nova"]);
        hub.write(Item::user_mail(
            "nova",
            "ask",
            "Question",
            "What is the status?",
            8,
        ))
        .await;
        hub.write(
            Item::agent_message("nova", "answer", "Re: Question", "All green.", 9)
                .replying_to("nova/agent/ask"),
        )
        .await;
        hub.write(
            Item::agent_message("atlas", "followup", "Re: Question", "Also green here.", 10)
                .replying_to("nova/user/answer"),
        )
        .await;

        let tool = InboxReadTool::new(hub.access("atlas"));
        let result = tool
            .execute(serde_json::json!({"id": "nova/user/answer"}))
            .await
            .unwrap();
        let out = &result.output;
        assert!(out.starts_with("Ref: nova/user/answer\n"), "{out}");
        assert!(out.contains("All green."), "{out}");
        assert!(
            out.contains("--- From the user, to nova · 2026-02-25T08:00 · Ref: nova/agent/ask\nTitle: Question\nWhat is the status?"),
            "{out}"
        );
        assert!(
            out.contains("Later messages (read with inbox_read): atlas/user/followup"),
            "{out}"
        );
        assert!(!hub.load("nova", InboxKind::User, "answer").await.read);

        // Another agent's agent-inbox item stays unread too.
        tool.execute(serde_json::json!({"id": "nova/agent/ask"}))
            .await
            .unwrap();
        assert!(!hub.load("nova", InboxKind::Agent, "ask").await.read);

        // So does this agent's own user-inbox item.
        tool.execute(serde_json::json!({"id": "atlas/user/followup"}))
            .await
            .unwrap();
        assert!(!hub.load("atlas", InboxKind::User, "followup").await.read);
    }

    #[tokio::test]
    async fn inbox_read_finds_an_archived_item() {
        let hub = MailHub::new(&["atlas", "nova"]);
        hub.write(Item::agent_message("nova", "old", "Old note", "From last week.", 7).archived())
            .await;
        let result = InboxReadTool::new(hub.access("atlas"))
            .execute(serde_json::json!({"id": "nova/user/old"}))
            .await
            .unwrap();
        assert!(result.output.contains("Archived: yes"), "{}", result.output);
        assert!(
            result.output.contains("From last week."),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn inbox_read_without_a_directory_reads_only_this_agent() {
        let hub = MailHub::new(&["atlas", "nova"]);
        hub.write(Item::agent_message("nova", "note", "Note", "Hi.", 9))
            .await;
        hub.write(Item::user_mail("atlas", "mine", "Mine", "For atlas.", 9))
            .await;
        let tool = InboxReadTool::new(hub.access_without_directory("atlas"));

        let mine = tool
            .execute(serde_json::json!({"id": "mine"}))
            .await
            .unwrap();
        assert!(mine.output.contains("For atlas."), "{}", mine.output);

        let Err(ToolError::Execution(message)) = tool
            .execute(serde_json::json!({"id": "nova/user/note"}))
            .await
        else {
            panic!("another agent's item is out of reach without a directory");
        };
        assert!(message.contains("no agent named 'nova'"), "{message}");
        assert!(message.contains(OWN_INBOX_ONLY_NOTE), "{message}");
    }
}

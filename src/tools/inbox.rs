//! Inbox management tools: list, read, and archive inbox items.

use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::inbox::{self, InboxItem};
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

// ─── inbox_list ─────────────────────────────────────────────────────────────

/// Tool for listing inbox items.
pub struct InboxListTool {
    inbox_dir: PathBuf,
    archive_dir: PathBuf,
}

impl InboxListTool {
    /// Create a new `InboxListTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf, archive_dir: PathBuf) -> Self {
        Self {
            inbox_dir,
            archive_dir,
        }
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
            description: "List inbox items. Shows unread/read status, title, source, and timestamp for each item.".to_string(),
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
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let archived = arguments
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let unread_only = !archived
            && arguments
                .get("unread_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);

        let dir = if archived {
            &self.archive_dir
        } else {
            &self.inbox_dir
        };
        let items = inbox::list_items(dir).await.map_err(|e| {
            tracing::error!(error = %e, archived, "failed to list inbox items");
            ToolError::Execution(format!("failed to list inbox items: {e}"))
        })?;

        let filtered: Vec<&(String, InboxItem)> = if unread_only {
            items.iter().filter(|(_, item)| !item.read).collect()
        } else {
            items.iter().collect()
        };

        if filtered.is_empty() {
            let label = if archived { "archived" } else { "inbox" };
            return Ok(ToolResult::success(format!("No {label} items found.")));
        }

        let mut lines: Vec<String> = Vec::new();
        let label = if archived { "archived" } else { "inbox" };
        lines.push(format!("{} {label} item(s):", filtered.len()));

        for (filename, item) in &filtered {
            let status = if item.read { "read" } else { "unread" };
            let ts = item.timestamp.format("%Y-%m-%dT%H:%M");
            lines.push(format!(
                "  [{status}] {filename} — {} ({}, {ts})",
                item.title, item.source
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

// ─── inbox_read ─────────────────────────────────────────────────────────────

/// Tool for reading a single inbox item (marks it as read).
pub struct InboxReadTool {
    inbox_dir: PathBuf,
}

impl InboxReadTool {
    /// Create a new `InboxReadTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf) -> Self {
        Self { inbox_dir }
    }
}

#[async_trait]
impl Tool for InboxReadTool {
    fn name(&self) -> &'static str {
        "inbox_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Read a single inbox item by filename stem. Marks the item as read and returns its full content.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Filename stem of the inbox item (without .json extension)"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let id = super::require_str(&arguments, "id")?;

        let item = inbox::mark_read(&self.inbox_dir, id).await.map_err(|e| {
            tracing::error!(error = %e, id = %id, "failed to read inbox item");
            ToolError::Execution(format!("failed to read inbox item '{id}': {e}"))
        })?;

        let ts = item.timestamp.format("%Y-%m-%dT%H:%M");
        let mut output = format!(
            "Title: {}\nSource: {}\nTime: {ts}\n",
            item.title, item.source
        );

        if !item.attachments.is_empty() {
            let paths: Vec<String> = item
                .attachments
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            _ = writeln!(output, "Attachments: {}", paths.join(", "));
        }

        _ = write!(output, "\n{}", item.body);

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

// ─── user_inbox_add ─────────────────────────────────────────────────────────

/// Tool for adding items to the user's inbox.
pub struct UserInboxAddTool {
    user_inbox_dir: PathBuf,
    user_inbox_attachments_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl UserInboxAddTool {
    /// Create a new `UserInboxAddTool`.
    #[must_use]
    pub fn new(
        user_inbox_dir: PathBuf,
        user_inbox_attachments_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            user_inbox_dir,
            user_inbox_attachments_dir,
            tz,
        }
    }
}

#[async_trait]
impl Tool for UserInboxAddTool {
    fn name(&self) -> &'static str {
        "user_inbox_add"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Add a new item to the user's inbox. Use this to explicitly send notes, reminders, or items for the human user to review later.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A short summary of the item"
                    },
                    "body": {
                        "type": "string",
                        "description": "The detailed content of the item"
                    },
                    "attachments": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional. Paths to files you have already written to disk that should be attached to this item — e.g. a report, screenshot, or export a background task produced. Each file is copied into the item's own storage, so it's safe even if the source file is later moved or deleted. Omit or leave empty if there's nothing to attach."
                    }
                },
                "required": ["title", "body"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let title = super::require_str(&arguments, "title")?;
        let body = super::require_str(&arguments, "body")?;

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

        let (filename, failures) = inbox::quick_add_with_attachments(
            &self.user_inbox_dir,
            &self.user_inbox_attachments_dir,
            title,
            body,
            "agent",
            self.tz,
            &attachment_paths,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, attachment_count = attachment_paths.len(), "failed to add item to user inbox");
            ToolError::Execution(format!("failed to add item to user inbox: {e}"))
        })?;

        let id = filename.trim_end_matches(".json");
        let succeeded = attachment_paths.len() - failures.len();
        let mut message = if attachment_paths.is_empty() {
            format!("Added item to user inbox with ID: {id}")
        } else {
            format!("Added item to user inbox with ID: {id} ({succeeded} attachment(s) copied)")
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_correct() {
        let dir = PathBuf::from("/tmp");
        let archive = PathBuf::from("/tmp/archive");

        assert_eq!(
            InboxListTool::new(dir.clone(), archive.clone()).name(),
            "inbox_list"
        );
        assert_eq!(InboxReadTool::new(dir.clone()).name(), "inbox_read");
        assert_eq!(
            InboxArchiveTool::new(dir.clone(), archive.clone()).name(),
            "inbox_archive"
        );
        assert_eq!(
            InboxRestoreTool::new(dir.clone(), archive).name(),
            "inbox_restore"
        );
        assert_eq!(
            UserInboxAddTool::new(dir.clone(), dir.join("attachments"), chrono_tz::UTC).name(),
            "user_inbox_add"
        );
    }

    #[test]
    fn definitions_have_matching_names() {
        let dir = PathBuf::from("/tmp");
        let archive = PathBuf::from("/tmp/archive");

        let list = InboxListTool::new(dir.clone(), archive.clone());
        assert_eq!(list.definition().name, list.name());

        let read = InboxReadTool::new(dir.clone());
        assert_eq!(read.definition().name, read.name());

        let archive_tool = InboxArchiveTool::new(dir.clone(), archive.clone());
        assert_eq!(archive_tool.definition().name, archive_tool.name());

        let restore_tool = InboxRestoreTool::new(dir.clone(), archive);
        assert_eq!(restore_tool.definition().name, restore_tool.name());

        let user_add = UserInboxAddTool::new(dir.clone(), dir.join("attachments"), chrono_tz::UTC);
        assert_eq!(user_add.definition().name, user_add.name());
    }

    #[tokio::test]
    async fn inbox_add_list_read_archive_flow() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let archive_dir = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        // Add an item via quick_add directly
        let filename = crate::inbox::quick_add(
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
        let list_tool = InboxListTool::new(inbox_dir.clone(), archive_dir.clone());
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
        let read_tool = InboxReadTool::new(inbox_dir.clone());
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
        let inbox_dir = dir.path().join("inbox");
        let archive_dir = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let filename = crate::inbox::quick_add(
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

        let list_tool = InboxListTool::new(inbox_dir.clone(), archive_dir.clone());

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
        let tool = InboxReadTool::new(dir.path().to_path_buf());
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing id should error");
    }

    #[tokio::test]
    async fn inbox_read_nonexistent_item() {
        let dir = tempfile::tempdir().unwrap();
        let tool = InboxReadTool::new(dir.path().to_path_buf());
        // id is provided but the file does not exist on disk — goes through mark_read,
        // which propagates as ToolError::Execution
        let result = tool
            .execute(serde_json::json!({"id": "does-not-exist"}))
            .await;
        assert!(
            result.is_err(),
            "reading nonexistent item should return ToolError"
        );
    }

    #[tokio::test]
    async fn user_inbox_add_without_attachments_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let user_inbox_dir = dir.path().join("inbox/user");
        let attachments_dir = dir.path().join("inbox/user/attachments");
        tokio::fs::create_dir_all(&user_inbox_dir).await.unwrap();

        let tool = UserInboxAddTool::new(
            user_inbox_dir.clone(),
            attachments_dir.clone(),
            chrono_tz::UTC,
        );
        let result = tool
            .execute(serde_json::json!({"title": "plain reminder", "body": "just text"}))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(
            result
                .output
                .starts_with("Added item to user inbox with ID: "),
            "confirmation message format should be unchanged: {}",
            result.output
        );
        assert!(
            !result.output.contains("attachment(s) copied"),
            "confirmation message should not report a copy count when none were given: {}",
            result.output
        );
        assert!(
            !attachments_dir.exists(),
            "no attachments directory should be created"
        );
    }

    #[tokio::test]
    async fn user_inbox_add_with_attachments_copies_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_inbox_dir = dir.path().join("inbox/user");
        let attachments_dir = dir.path().join("inbox/user/attachments");
        tokio::fs::create_dir_all(&user_inbox_dir).await.unwrap();

        let source_file = dir.path().join("export.csv");
        tokio::fs::write(&source_file, b"a,b,c").await.unwrap();

        let tool = UserInboxAddTool::new(
            user_inbox_dir.clone(),
            attachments_dir.clone(),
            chrono_tz::UTC,
        );
        let result = tool
            .execute(serde_json::json!({
                "title": "weekly export",
                "body": "attached",
                "attachments": [source_file.to_string_lossy()],
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed: {}", result.output);
        assert!(
            result.output.contains("1 attachment(s) copied"),
            "confirmation should report the attachment count: {}",
            result.output
        );

        let mut entries = tokio::fs::read_dir(&user_inbox_dir).await.unwrap();
        let mut found_copy = false;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                let item = crate::inbox::load_item(&entry.path()).await.unwrap();
                if item.title == "weekly export" {
                    assert_eq!(item.attachments.len(), 1);
                    found_copy = true;
                }
            }
        }
        assert!(found_copy, "saved item should be found in the inbox dir");
    }

    #[tokio::test]
    async fn user_inbox_add_missing_attachment_source_still_saves_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let user_inbox_dir = dir.path().join("inbox/user");
        let attachments_dir = dir.path().join("inbox/user/attachments");
        tokio::fs::create_dir_all(&user_inbox_dir).await.unwrap();

        let tool = UserInboxAddTool::new(user_inbox_dir.clone(), attachments_dir, chrono_tz::UTC);
        let result = tool
            .execute(serde_json::json!({
                "title": "broken",
                "body": "still saved",
                "attachments": ["/tmp/residuum_test_does_not_exist.bin"],
            }))
            .await
            .unwrap();

        // A file that can't be attached is reported, but never fails the
        // whole item — the item itself is still saved.
        assert!(!result.is_error, "{}", result.output);
        assert!(
            result.output.contains("could not be copied"),
            "the failure should be reported: {}",
            result.output
        );

        let mut entries = tokio::fs::read_dir(&user_inbox_dir).await.unwrap();
        let mut json_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                json_files.push(entry.path());
            }
        }
        assert_eq!(
            json_files.len(),
            1,
            "the item should be saved even though its attachment failed: {json_files:?}"
        );
    }
}

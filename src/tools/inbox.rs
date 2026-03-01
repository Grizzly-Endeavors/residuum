//! Inbox management tools: list, read, add, and archive inbox items.

use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::inbox::{self, InboxItem};
use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

// ─── inbox_list ─────────────────────────────────────────────────────────────

/// Tool for listing inbox items.
pub struct InboxListTool {
    inbox_dir: PathBuf,
}

impl InboxListTool {
    /// Create a new `InboxListTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf) -> Self {
        Self { inbox_dir }
    }
}

#[async_trait]
impl Tool for InboxListTool {
    fn name(&self) -> &'static str {
        "inbox_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "inbox_list".to_string(),
            description: "List inbox items. Shows unread/read status, title, source, and timestamp for each item.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "unread_only": {
                        "type": "boolean",
                        "description": "Only show unread items (default false)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let unread_only = arguments
            .get("unread_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let items = inbox::list_items(&self.inbox_dir)
            .await
            .map_err(|e| ToolError::Execution(format!("failed to list inbox items: {e}")))?;

        let filtered: Vec<&(String, InboxItem)> = if unread_only {
            items.iter().filter(|(_, item)| !item.read).collect()
        } else {
            items.iter().collect()
        };

        if filtered.is_empty() {
            return Ok(ToolResult::success("No inbox items found."));
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("{} inbox item(s):", filtered.len()));

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
            name: "inbox_read".to_string(),
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
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("id is required".to_string()))?;

        let item = inbox::mark_read(&self.inbox_dir, id)
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read inbox item '{id}': {e}")))?;

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

// ─── inbox_add ──────────────────────────────────────────────────────────────

/// Tool for adding a new inbox item.
pub struct InboxAddTool {
    inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
}

impl InboxAddTool {
    /// Create a new `InboxAddTool`.
    #[must_use]
    pub fn new(inbox_dir: PathBuf, tz: chrono_tz::Tz) -> Self {
        Self { inbox_dir, tz }
    }
}

#[async_trait]
impl Tool for InboxAddTool {
    fn name(&self) -> &'static str {
        "inbox_add"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "inbox_add".to_string(),
            description: "Add a new item to the inbox. Use this to save reminders, notes, or anything to deal with later.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short summary of the inbox item"
                    },
                    "body": {
                        "type": "string",
                        "description": "Full body text of the item"
                    },
                    "source": {
                        "type": "string",
                        "description": "Origin label (default: 'agent')"
                    }
                },
                "required": ["title", "body"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("title is required".to_string()))?;

        let body = arguments
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("body is required".to_string()))?;

        let source = arguments
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("agent");

        let filename = inbox::quick_add(&self.inbox_dir, title, body, source, self.tz)
            .await
            .map_err(|e| ToolError::Execution(format!("failed to save inbox item: {e}")))?;

        Ok(ToolResult::success(format!(
            "Added inbox item '{title}' as {filename}"
        )))
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
            name: "inbox_archive".to_string(),
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
                Err(e) => errors.push(format!("{id}: {e}")),
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn tool_names_correct() {
        let dir = PathBuf::from("/tmp");
        let archive = PathBuf::from("/tmp/archive");

        assert_eq!(InboxListTool::new(dir.clone()).name(), "inbox_list");
        assert_eq!(InboxReadTool::new(dir.clone()).name(), "inbox_read");
        assert_eq!(
            InboxAddTool::new(dir.clone(), chrono_tz::UTC).name(),
            "inbox_add"
        );
        assert_eq!(InboxArchiveTool::new(dir, archive).name(), "inbox_archive");
    }

    #[test]
    fn definitions_have_matching_names() {
        let dir = PathBuf::from("/tmp");
        let archive = PathBuf::from("/tmp/archive");

        let list = InboxListTool::new(dir.clone());
        assert_eq!(list.definition().name, list.name());

        let read = InboxReadTool::new(dir.clone());
        assert_eq!(read.definition().name, read.name());

        let add = InboxAddTool::new(dir.clone(), chrono_tz::UTC);
        assert_eq!(add.definition().name, add.name());

        let archive_tool = InboxArchiveTool::new(dir, archive);
        assert_eq!(archive_tool.definition().name, archive_tool.name());
    }

    #[tokio::test]
    async fn inbox_add_list_read_archive_flow() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        let archive_dir = dir.path().join("archive/inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        // Add an item
        let add_tool = InboxAddTool::new(inbox_dir.clone(), chrono_tz::UTC);
        let add_result = add_tool
            .execute(serde_json::json!({
                "title": "test item",
                "body": "test body content",
                "source": "test"
            }))
            .await
            .unwrap();
        assert!(
            !add_result.is_error,
            "add should succeed: {}",
            add_result.output
        );
        assert!(
            add_result.output.contains("Added inbox item"),
            "should confirm add: {}",
            add_result.output
        );

        // Extract filename from output
        let filename = add_result
            .output
            .split(" as ")
            .nth(1)
            .unwrap()
            .trim_end_matches(".json")
            .to_string();

        // List items
        let list_tool = InboxListTool::new(inbox_dir.clone());
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
    async fn inbox_add_missing_title() {
        let dir = tempfile::tempdir().unwrap();
        let tool = InboxAddTool::new(dir.path().to_path_buf(), chrono_tz::UTC);
        let result = tool.execute(serde_json::json!({"body": "no title"})).await;
        assert!(result.is_err(), "missing title should error");
    }

    #[tokio::test]
    async fn inbox_read_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let tool = InboxReadTool::new(dir.path().to_path_buf());
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing id should error");
    }
}

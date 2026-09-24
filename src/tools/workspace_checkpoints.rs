//! `workspace_history` / `workspace_restore`: agent-facing access to the
//! workspace checkpoint history (see `crate::checkpoints`).
//!
//! Scoped to the workspace checkpoint repository only — the config
//! repository (root `config.toml`, `providers.toml`, and the encrypted
//! secret/agent-key/A2A-key stores) is never reachable from either tool, so
//! these tools can't be used to read or restore something `PathPolicy`
//! already blocks the agent from touching directly.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger, RepoKind};
use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult, require_str};

const REPO: RepoKind = RepoKind::Workspace;

/// Rejects a caller-supplied relative path that could escape the workspace
/// root (`..` components or an absolute path).
fn validate_relative_path(path: &str) -> Result<(), ToolError> {
    let p = std::path::Path::new(path);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::InvalidArguments(format!(
            "path '{path}' must be relative to the workspace root and contain no '..' components"
        )));
    }
    Ok(())
}

/// Caller identity recorded on any checkpoint a tool call itself produces
/// (a restore or an undo).
fn agent_context(trigger: CheckpointTrigger, summary: impl Into<String>) -> CheckpointContext {
    CheckpointContext {
        address: "main".to_string(),
        run_id: None,
        turn_id: None,
        trigger,
        summary: summary.into(),
    }
}

// ─── workspace_history ───────────────────────────────────────────────────

/// Tool for listing checkpoints and showing what a checkpoint changed.
pub struct WorkspaceHistoryTool {
    engine: Arc<CheckpointEngine>,
}

impl WorkspaceHistoryTool {
    /// Create a new `WorkspaceHistoryTool`.
    #[must_use]
    pub fn new(engine: Arc<CheckpointEngine>) -> Self {
        Self { engine }
    }

    async fn list(&self, arguments: &Value) -> Result<ToolResult, ToolError> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(path) = &path {
            validate_relative_path(path)?;
        }
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok());

        let page = self
            .engine
            .list_checkpoints(REPO, path, None, limit)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        if page.items.is_empty() {
            return Ok(ToolResult::success("No checkpoints yet."));
        }

        let mut lines = vec![format!("{} checkpoint(s):", page.items.len())];
        for item in &page.items {
            lines.push(format!(
                "  {} [{}] {} — {} ({} path(s) changed)",
                short_id(&item.id),
                item.trigger.label(),
                item.address,
                item.summary,
                item.changed_path_count
            ));
        }
        Ok(ToolResult::success(lines.join("\n")))
    }

    async fn show(&self, arguments: &Value) -> Result<ToolResult, ToolError> {
        let id = require_str(arguments, "checkpoint_id")?.to_string();
        let detail = self
            .engine
            .show_checkpoint(REPO, id.clone())
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let mut lines = vec![format!(
            "Checkpoint {} [{}] {} — {}",
            short_id(&id),
            detail.summary.trigger.label(),
            detail.summary.address,
            detail.summary.summary
        )];
        if detail.changed_paths.is_empty() {
            lines.push("  (no changes)".to_string());
        }
        for change in &detail.changed_paths {
            lines.push(format!("  {:?} {}", change.kind, change.path));
        }
        Ok(ToolResult::success(lines.join("\n")))
    }
}

#[async_trait]
impl Tool for WorkspaceHistoryTool {
    fn name(&self) -> &'static str {
        "workspace_history"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List workspace checkpoints (recovery snapshots of workspace files, taken automatically at turn boundaries and before destructive actions), optionally filtered to those that changed a given path, or show what a specific checkpoint changed.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show"],
                        "description": "'list' to list checkpoints, 'show' to see what one checkpoint changed"
                    },
                    "path": {
                        "type": "string",
                        "description": "list only: restrict to checkpoints that changed this workspace-relative file or directory"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "list only: maximum checkpoints to return (default 50)"
                    },
                    "checkpoint_id": {
                        "type": "string",
                        "description": "show only: the checkpoint id to describe, from a previous 'list' call"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        match require_str(&arguments, "action")? {
            "list" => self.list(&arguments).await,
            "show" => self.show(&arguments).await,
            other => Err(ToolError::InvalidArguments(format!(
                "unknown action '{other}': expected 'list' or 'show'"
            ))),
        }
    }
}

// ─── workspace_restore ───────────────────────────────────────────────────

/// Tool for restoring a path to a checkpoint, or undoing a checkpoint's
/// changes.
pub struct WorkspaceRestoreTool {
    engine: Arc<CheckpointEngine>,
}

impl WorkspaceRestoreTool {
    /// Create a new `WorkspaceRestoreTool`.
    #[must_use]
    pub fn new(engine: Arc<CheckpointEngine>) -> Self {
        Self { engine }
    }

    async fn restore_path(&self, arguments: &Value) -> Result<ToolResult, ToolError> {
        let id = require_str(arguments, "checkpoint_id")?.to_string();
        let path = require_str(arguments, "path")?.to_string();
        validate_relative_path(&path)?;

        let outcome = self
            .engine
            .restore_path(
                REPO,
                id.clone(),
                path.clone(),
                agent_context(
                    CheckpointTrigger::Restore,
                    format!("agent restored {path} from checkpoint {}", short_id(&id)),
                ),
            )
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        Ok(ToolResult::success(format!(
            "Restored {} path(s) from checkpoint {}: {}",
            outcome.restored_paths.len(),
            short_id(&id),
            outcome.restored_paths.join(", ")
        )))
    }

    async fn undo_turn(&self, arguments: &Value) -> Result<ToolResult, ToolError> {
        let id = require_str(arguments, "checkpoint_id")?.to_string();

        let outcome = self
            .engine
            .undo_checkpoint(
                REPO,
                id.clone(),
                agent_context(
                    CheckpointTrigger::Undo,
                    format!("agent undid checkpoint {}", short_id(&id)),
                ),
            )
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let mut message = format!(
            "Reverted {} path(s) from checkpoint {}: {}.",
            outcome.reverted_paths.len(),
            short_id(&id),
            outcome.reverted_paths.join(", ")
        );
        if !outcome.skipped_paths.is_empty() {
            use std::fmt::Write as _;
            _ = write!(
                message,
                " Skipped {} path(s) changed again since: {}.",
                outcome.skipped_paths.len(),
                outcome.skipped_paths.join(", ")
            );
        }
        Ok(ToolResult::success(message))
    }
}

#[async_trait]
impl Tool for WorkspaceRestoreTool {
    fn name(&self) -> &'static str {
        "workspace_restore"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Restore a workspace file or directory to its content at a checkpoint (from workspace_history), or undo a checkpoint's own changes -- reverting each path it changed back to its content just before it, skipping any path that was changed again since so a later edit is never clobbered. Both actions checkpoint the result first, so a restore or undo can itself be undone.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["restore_path", "undo_turn"],
                        "description": "'restore_path' to restore one file/directory, 'undo_turn' to revert everything a checkpoint changed"
                    },
                    "checkpoint_id": {
                        "type": "string",
                        "description": "the checkpoint id, from workspace_history"
                    },
                    "path": {
                        "type": "string",
                        "description": "restore_path only: the workspace-relative file or directory to restore"
                    }
                },
                "required": ["action", "checkpoint_id"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        match require_str(&arguments, "action")? {
            "restore_path" => self.restore_path(&arguments).await,
            "undo_turn" => self.undo_turn(&arguments).await,
            other => Err(ToolError::InvalidArguments(format!(
                "unknown action '{other}': expected 'restore_path' or 'undo_turn'"
            ))),
        }
    }
}

/// First 12 hex characters of a checkpoint id, for compact display.
fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(dir: &std::path::Path) -> Arc<CheckpointEngine> {
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        Arc::new(
            CheckpointEngine::new(
                workspace,
                dir.join("config"),
                &dir.join("checkpoints"),
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_relative_path("../etc/passwd").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("wiki/../../secret").is_err());
        assert!(validate_relative_path("wiki/index.md").is_ok());
    }

    #[tokio::test]
    async fn list_reports_no_checkpoints_yet() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WorkspaceHistoryTool::new(engine(dir.path()));
        let result = tool
            .execute(serde_json::json!({"action": "list"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("No checkpoints"));
    }

    #[tokio::test]
    async fn restore_path_rejects_traversal_before_touching_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WorkspaceRestoreTool::new(engine(dir.path()));
        let result = tool
            .execute(serde_json::json!({
                "action": "restore_path",
                "checkpoint_id": "deadbeef",
                "path": "../outside"
            }))
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn history_and_restore_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let checkpoints_engine = Arc::new(
            CheckpointEngine::new(
                ws.clone(),
                dir.path().join("config"),
                &dir.path().join("checkpoints"),
                None,
            )
            .unwrap(),
        );

        std::fs::write(ws.join("notes.md"), "v1").unwrap();
        checkpoints_engine
            .checkpoint_workspace_before_action(agent_context(CheckpointTrigger::PreAction, "v1"))
            .await;

        let history = WorkspaceHistoryTool::new(Arc::clone(&checkpoints_engine));
        let list = history
            .execute(serde_json::json!({"action": "list"}))
            .await
            .unwrap();
        assert!(list.output.contains("checkpoint(s)"));

        std::fs::write(ws.join("notes.md"), "v2").unwrap();
        let restore = WorkspaceRestoreTool::new(Arc::clone(&checkpoints_engine));
        let page = checkpoints_engine
            .list_checkpoints(RepoKind::Workspace, None, None, None)
            .await
            .unwrap();
        let first_id = page.items.last().expect("one checkpoint").id.clone();

        let outcome = restore
            .execute(serde_json::json!({
                "action": "restore_path",
                "checkpoint_id": first_id,
                "path": "notes.md"
            }))
            .await
            .unwrap();
        assert!(!outcome.is_error, "{}", outcome.output);
        assert_eq!(std::fs::read_to_string(ws.join("notes.md")).unwrap(), "v1");
    }
}

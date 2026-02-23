//! Project management tools: activate, deactivate, create, archive, list.

use async_trait::async_trait;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::projects::activation::SharedProjectState;
use crate::projects::lifecycle;

use super::{Tool, ToolError, ToolResult};

// ─── project_activate ────────────────────────────────────────────────────────

/// Tool for activating a project context.
pub struct ProjectActivateTool {
    state: SharedProjectState,
}

impl ProjectActivateTool {
    /// Create a new `ProjectActivateTool`.
    #[must_use]
    pub fn new(state: SharedProjectState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for ProjectActivateTool {
    fn name(&self) -> &'static str {
        "project_activate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "project_activate".to_string(),
            description: "Activate a project context. Loads the project's overview, manifest, and configuration into the agent's context.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the project to activate (case-insensitive)"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("name is required".to_string()))?;

        let mut state = self.state.lock().await;
        match state.activate(name).await {
            Ok(active) => Ok(ToolResult::success(format!(
                "Activated project '{}'. Manifest: {} notes, {} references, {} workspace, {} skills files.",
                active.name,
                active.manifest.notes.len(),
                active.manifest.references.len(),
                active.manifest.workspace.len(),
                active.manifest.skills.len(),
            ))),
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

// ─── project_deactivate ──────────────────────────────────────────────────────

/// Tool for deactivating the current project context.
pub struct ProjectDeactivateTool {
    state: SharedProjectState,
    tz: chrono_tz::Tz,
}

impl ProjectDeactivateTool {
    /// Create a new `ProjectDeactivateTool`.
    #[must_use]
    pub fn new(state: SharedProjectState, tz: chrono_tz::Tz) -> Self {
        Self { state, tz }
    }
}

#[async_trait]
impl Tool for ProjectDeactivateTool {
    fn name(&self) -> &'static str {
        "project_deactivate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "project_deactivate".to_string(),
            description: "Deactivate the current project context. Requires a non-empty session summary log entry.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "log": {
                        "type": "string",
                        "description": "Session summary log entry (required, must not be empty)"
                    }
                },
                "required": ["log"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let log = arguments
            .get("log")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("log is required".to_string()))?;

        let now = crate::time::now_local(self.tz);
        let mut state = self.state.lock().await;

        match state.deactivate(log, now).await {
            Ok(name) => Ok(ToolResult::success(format!(
                "Deactivated project '{name}'. Log entry recorded."
            ))),
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

// ─── project_create ──────────────────────────────────────────────────────────

/// Tool for creating a new project.
pub struct ProjectCreateTool {
    state: SharedProjectState,
    tz: chrono_tz::Tz,
}

impl ProjectCreateTool {
    /// Create a new `ProjectCreateTool`.
    #[must_use]
    pub fn new(state: SharedProjectState, tz: chrono_tz::Tz) -> Self {
        Self { state, tz }
    }
}

#[async_trait]
impl Tool for ProjectCreateTool {
    fn name(&self) -> &'static str {
        "project_create"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "project_create".to_string(),
            description:
                "Create a new project with the standard directory structure and PROJECT.md."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Human-readable project name"
                    },
                    "description": {
                        "type": "string",
                        "description": "Brief summary of what this project covers"
                    },
                    "tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional list of tool names to associate with this project"
                    }
                },
                "required": ["name", "description"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("name is required".to_string()))?;

        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("description is required".to_string()))?;

        let tools: Vec<String> = arguments
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let today = crate::time::now_local(self.tz).date();
        let mut state = self.state.lock().await;
        let layout = state.layout().clone();

        match lifecycle::create_project(&layout, name, description, tools, today).await {
            Ok(path) => {
                state.rescan().await.map_err(|e| {
                    ToolError::Execution(format!("project created but rescan failed: {e}"))
                })?;
                Ok(ToolResult::success(format!(
                    "Created project '{name}' at {}",
                    path.display()
                )))
            }
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

// ─── project_archive ─────────────────────────────────────────────────────────

/// Tool for archiving a project.
pub struct ProjectArchiveTool {
    state: SharedProjectState,
    tz: chrono_tz::Tz,
}

impl ProjectArchiveTool {
    /// Create a new `ProjectArchiveTool`.
    #[must_use]
    pub fn new(state: SharedProjectState, tz: chrono_tz::Tz) -> Self {
        Self { state, tz }
    }
}

#[async_trait]
impl Tool for ProjectArchiveTool {
    fn name(&self) -> &'static str {
        "project_archive"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "project_archive".to_string(),
            description: "Archive a completed project. Updates frontmatter to archived status and moves it to the archive directory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the project to archive"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("name is required".to_string()))?;

        let mut state = self.state.lock().await;

        // Look up dir_name from the index
        let dir_name = state
            .index()
            .find_by_name(name)
            .map(|e| e.dir_name.clone())
            .ok_or_else(|| ToolError::Execution(format!("project '{name}' not found in index")))?;

        // If this is the active project, deactivate first
        if state
            .active_project_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            return Ok(ToolResult::error(
                "cannot archive the active project — deactivate it first".to_string(),
            ));
        }

        let today = crate::time::now_local(self.tz).date();
        let layout = state.layout().clone();

        match lifecycle::archive_project(&layout, &dir_name, today).await {
            Ok(()) => {
                state.rescan().await.map_err(|e| {
                    ToolError::Execution(format!("project archived but rescan failed: {e}"))
                })?;
                Ok(ToolResult::success(format!(
                    "Archived project '{name}'. Moved to archive/."
                )))
            }
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

// ─── project_list ────────────────────────────────────────────────────────────

/// Tool for listing all projects.
pub struct ProjectListTool {
    state: SharedProjectState,
}

impl ProjectListTool {
    /// Create a new `ProjectListTool`.
    #[must_use]
    pub fn new(state: SharedProjectState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for ProjectListTool {
    fn name(&self) -> &'static str {
        "project_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "project_list".to_string(),
            description: "List all projects and their status.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include archived projects in the list (default false)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let include_archived = arguments
            .get("include_archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let state = self.state.lock().await;
        let entries = state.index().entries();

        let filtered: Vec<_> = entries
            .iter()
            .filter(|e| include_archived || !e.is_archived)
            .collect();

        if filtered.is_empty() {
            return Ok(ToolResult::success("No projects found."));
        }

        let mut lines = Vec::with_capacity(filtered.len() + 1);
        lines.push(format!("{} project(s):", filtered.len()));

        for entry in filtered {
            let active_marker = if state.active_project_name().is_some_and(|n| n == entry.name) {
                " [ACTIVE]"
            } else {
                ""
            };
            lines.push(format!(
                "  [{}] {}{active_marker} — {}",
                entry.status, entry.name, entry.description
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::projects::activation::ProjectState;
    use crate::projects::scanner::ProjectIndex;
    use crate::workspace::bootstrap::ensure_workspace;
    use crate::workspace::layout::WorkspaceLayout;

    async fn setup() -> (tempfile::TempDir, SharedProjectState) {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));
        ensure_workspace(&layout).await.unwrap();
        let index = ProjectIndex::scan(&layout).await.unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(ProjectState::new(index, layout)));
        (dir, state)
    }

    #[test]
    fn tool_names() {
        let state = Arc::new(tokio::sync::Mutex::new(ProjectState::new(
            ProjectIndex::default(),
            WorkspaceLayout::new("/tmp/test"),
        )));

        assert_eq!(
            ProjectActivateTool::new(Arc::clone(&state)).name(),
            "project_activate",
            "activate tool name"
        );
        assert_eq!(
            ProjectDeactivateTool::new(Arc::clone(&state), chrono_tz::UTC).name(),
            "project_deactivate",
            "deactivate tool name"
        );
        assert_eq!(
            ProjectCreateTool::new(Arc::clone(&state), chrono_tz::UTC).name(),
            "project_create",
            "create tool name"
        );
        assert_eq!(
            ProjectArchiveTool::new(Arc::clone(&state), chrono_tz::UTC).name(),
            "project_archive",
            "archive tool name"
        );
        assert_eq!(
            ProjectListTool::new(state).name(),
            "project_list",
            "list tool name"
        );
    }

    #[tokio::test]
    async fn list_empty() {
        let (_dir, state) = setup().await;
        let tool = ProjectListTool::new(state);
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(!result.is_error, "should succeed");
        assert!(
            result.output.contains("No projects found"),
            "should show no projects"
        );
    }

    #[tokio::test]
    async fn create_and_list() {
        let (_dir, state) = setup().await;

        let create_tool = ProjectCreateTool::new(Arc::clone(&state), chrono_tz::UTC);
        let result = create_tool
            .execute(serde_json::json!({
                "name": "Test Project",
                "description": "A test"
            }))
            .await
            .unwrap();
        assert!(!result.is_error, "create should succeed: {}", result.output);

        let list_tool = ProjectListTool::new(Arc::clone(&state));
        let list_result = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(
            list_result.output.contains("Test Project"),
            "list should include created project"
        );
    }

    #[tokio::test]
    async fn activate_nonexistent() {
        let (_dir, state) = setup().await;
        let tool = ProjectActivateTool::new(state);
        let result = tool
            .execute(serde_json::json!({"name": "nonexistent"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for nonexistent project");
    }

    #[tokio::test]
    async fn deactivate_requires_log() {
        let (_dir, state) = setup().await;
        let tool = ProjectDeactivateTool::new(state, chrono_tz::UTC);
        let result = tool.execute(serde_json::json!({"log": ""})).await.unwrap();
        assert!(
            result.is_error,
            "should error for empty log: {}",
            result.output
        );
    }
}

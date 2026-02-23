//! Tool system for agent-invoked operations.

pub mod cron;
mod edit;
mod exec;
pub(crate) mod file_tracker;
mod line_hash;
pub mod memory_search;
pub mod path_policy;
pub mod projects;
mod read;
pub mod skills;
mod write;

pub use file_tracker::{FileTracker, SharedFileTracker};
pub use path_policy::{PathPolicy, SharedPathPolicy};

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::models::ToolDefinition;

/// Errors from tool execution.
#[derive(Error, Debug)]
pub enum ToolError {
    /// The requested tool was not found in the registry.
    #[error("unknown tool: {0}")]
    NotFound(String),

    /// Tool execution failed.
    #[error("tool execution failed: {0}")]
    Execution(String),

    /// Invalid arguments provided to the tool.
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// The output text from the tool.
    pub output: String,
    /// Whether the tool execution encountered an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful tool result.
    #[must_use]
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    /// Create an error tool result.
    #[must_use]
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

/// Shared tool filter, consulted by `ToolRegistry` to gate tools per-project.
pub type SharedToolFilter = Arc<RwLock<ToolFilter>>;

/// Controls which gated tools are visible and executable.
///
/// Some tools (e.g. `exec`) are "gated" — only available when the active
/// project opts in via its `tools` field. Core tools are always visible.
pub struct ToolFilter {
    /// Tool names that require an active project to opt in.
    gated: HashSet<&'static str>,
    /// Currently enabled gated tool names (set by active project's `tools` field).
    enabled: HashSet<String>,
}

impl ToolFilter {
    /// Create a new tool filter with the given set of gated tool names.
    #[must_use]
    pub fn new(gated: HashSet<&'static str>) -> Self {
        Self {
            gated,
            enabled: HashSet::new(),
        }
    }

    /// Create a new shared tool filter.
    #[must_use]
    pub fn new_shared(gated: HashSet<&'static str>) -> SharedToolFilter {
        Arc::new(RwLock::new(Self::new(gated)))
    }

    /// Enable a set of gated tools (called on project activation).
    pub fn enable(&mut self, tool_names: &[String]) {
        for name in tool_names {
            if self.gated.contains(name.as_str()) {
                self.enabled.insert(name.clone());
            }
        }
    }

    /// Clear all enabled gated tools (called on project deactivation).
    pub fn clear_enabled(&mut self) {
        self.enabled.clear();
    }

    /// Check whether a tool is available (either not gated, or gated and enabled).
    #[must_use]
    pub fn is_available(&self, name: &str) -> bool {
        !self.gated.contains(name) || self.enabled.contains(name)
    }
}

// TODO(phase-6): add runtime JSON Schema validation for third-party tools

/// Trait for tool implementations that the agent can invoke.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The unique name of this tool.
    fn name(&self) -> &'static str;

    /// The tool definition sent to the model.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError` if the arguments are invalid or execution fails.
    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError>;
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    #[must_use]
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Get tool definitions for sending to the model, filtered by the tool filter.
    #[must_use]
    pub fn definitions(&self, filter: &ToolFilter) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| filter.is_available(t.name()))
            .map(|t| t.definition())
            .collect()
    }

    /// Execute a tool by name with the given arguments, respecting the tool filter.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    pub async fn execute(
        &self,
        name: &str,
        arguments: Value,
        filter: &ToolFilter,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        if !filter.is_available(name) {
            return Ok(ToolResult::error(format!(
                "tool '{name}' is not available — activate a project that includes it"
            )));
        }

        tool.execute(arguments).await
    }

    /// Register the default set of tools (read, write, edit, exec).
    pub fn register_defaults(&mut self, tracker: SharedFileTracker, policy: SharedPathPolicy) {
        self.register(Box::new(read::ReadTool::new(Arc::clone(&tracker))));
        self.register(Box::new(write::WriteTool::new(
            Arc::clone(&tracker),
            Arc::clone(&policy),
        )));
        self.register(Box::new(edit::EditTool::new(tracker, policy)));
        self.register(Box::new(exec::ExecTool));
    }

    /// Register the `memory_search` tool with a shared index.
    pub fn register_search_tool(
        &mut self,
        index: std::sync::Arc<crate::memory::search::MemoryIndex>,
    ) {
        self.register(Box::new(memory_search::MemorySearchTool::new(index)));
    }

    /// Register project management tools.
    pub fn register_project_tools(
        &mut self,
        state: crate::projects::activation::SharedProjectState,
        path_policy: SharedPathPolicy,
        tool_filter: SharedToolFilter,
        mcp_registry: crate::mcp::SharedMcpRegistry,
        skill_state: crate::skills::SharedSkillState,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(projects::ProjectActivateTool::new(
            Arc::clone(&state),
            Arc::clone(&path_policy),
            Arc::clone(&tool_filter),
            Arc::clone(&mcp_registry),
            Arc::clone(&skill_state),
        )));
        self.register(Box::new(projects::ProjectDeactivateTool::new(
            Arc::clone(&state),
            path_policy,
            tool_filter,
            mcp_registry,
            skill_state,
            tz,
        )));
        self.register(Box::new(projects::ProjectCreateTool::new(
            Arc::clone(&state),
            tz,
        )));
        self.register(Box::new(projects::ProjectArchiveTool::new(
            Arc::clone(&state),
            tz,
        )));
        self.register(Box::new(projects::ProjectListTool::new(state)));
    }

    /// Register skill management tools (`skill_activate`, `skill_deactivate`).
    pub fn register_skill_tools(&mut self, state: crate::skills::SharedSkillState) {
        self.register(Box::new(skills::SkillActivateTool::new(Arc::clone(&state))));
        self.register(Box::new(skills::SkillDeactivateTool::new(state)));
    }

    /// Register cron management tools (`cron_add`, `cron_list`, `cron_update`, `cron_remove`).
    pub fn register_cron_tools(
        &mut self,
        store: std::sync::Arc<tokio::sync::Mutex<crate::cron::store::CronStore>>,
        notify: std::sync::Arc<tokio::sync::Notify>,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(cron::CronAddTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(cron::CronListTool::new(Arc::clone(&store))));
        self.register(Box::new(cron::CronUpdateTool::new(
            Arc::clone(&store),
            Arc::clone(&notify),
            tz,
        )));
        self.register(Box::new(cron::CronRemoveTool::new(store, notify)));
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn tool_result_success() {
        let result = ToolResult::success("output");
        assert!(!result.is_error, "success result should not be error");
        assert_eq!(result.output, "output", "output should match");
    }

    #[test]
    fn tool_result_error() {
        let result = ToolResult::error("failed");
        assert!(result.is_error, "error result should be error");
        assert_eq!(result.output, "failed", "output should match");
    }

    fn no_filter() -> ToolFilter {
        ToolFilter::new(HashSet::new())
    }

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry
            .execute("nonexistent", Value::Null, &no_filter())
            .await;
        assert!(result.is_err(), "should error on unknown tool");
        assert!(
            matches!(result.unwrap_err(), ToolError::NotFound(_)),
            "should be NotFound"
        );
    }

    #[test]
    fn registry_definitions_empty() {
        let registry = ToolRegistry::new();
        assert!(
            registry.definitions(&no_filter()).is_empty(),
            "empty registry should have no definitions"
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);
        let defs = registry.definitions(&no_filter());
        assert_eq!(defs.len(), 4, "should have read, write, edit, exec tools");
    }

    #[test]
    fn tool_filter_gating() {
        let mut filter = ToolFilter::new(HashSet::from(["exec"]));
        assert!(
            !filter.is_available("exec"),
            "exec should be gated and unavailable by default"
        );
        assert!(
            filter.is_available("read_file"),
            "ungated tools should always be available"
        );

        filter.enable(&["exec".to_string()]);
        assert!(
            filter.is_available("exec"),
            "exec should be available after enabling"
        );

        filter.clear_enabled();
        assert!(
            !filter.is_available("exec"),
            "exec should be unavailable after clearing"
        );
    }

    #[test]
    fn tool_filter_definitions_filtered() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        let filter_exec = ToolFilter::new(HashSet::from(["exec"]));
        let defs = registry.definitions(&filter_exec);
        assert_eq!(defs.len(), 3, "exec should be filtered out");
        assert!(
            defs.iter().all(|d| d.name != "exec"),
            "exec should not appear in definitions"
        );
    }

    #[tokio::test]
    async fn tool_filter_blocks_execution() {
        let mut registry = ToolRegistry::new();
        let policy = PathPolicy::new_shared(std::path::PathBuf::from("/tmp"));
        registry.register_defaults(FileTracker::new_shared(), policy);

        let filter_exec = ToolFilter::new(HashSet::from(["exec"]));
        let result = registry
            .execute(
                "exec",
                serde_json::json!({"command": "echo test"}),
                &filter_exec,
            )
            .await
            .unwrap();
        assert!(result.is_error, "gated tool should return error");
        assert!(
            result.output.contains("not available"),
            "error should mention unavailability"
        );
    }
}

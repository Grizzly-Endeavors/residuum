//! Tool system for agent-invoked operations.

pub mod cron;
pub mod daily_log;
mod edit;
mod exec;
pub(crate) mod file_tracker;
mod line_hash;
pub mod memory_search;
mod read;
mod write;

pub use file_tracker::{FileTracker, SharedFileTracker};

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

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

    /// Get all tool definitions for sending to the model.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Execute a tool by name with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tool.execute(arguments).await
    }

    /// Register the default set of tools (read, write, edit, exec).
    pub fn register_defaults(&mut self, tracker: SharedFileTracker) {
        self.register(Box::new(read::ReadTool::new(Arc::clone(&tracker))));
        self.register(Box::new(write::WriteTool::new(Arc::clone(&tracker))));
        self.register(Box::new(edit::EditTool::new(tracker)));
        self.register(Box::new(exec::ExecTool));
    }

    /// Register memory-related tools (`daily_log`).
    pub fn register_memory_tools(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
        tz: chrono_tz::Tz,
    ) {
        self.register(Box::new(daily_log::DailyLogTool::new(
            layout.memory_dir(),
            tz,
        )));
    }

    /// Register the `memory_search` tool with a shared index.
    pub fn register_search_tool(
        &mut self,
        index: std::sync::Arc<crate::memory::search::MemoryIndex>,
    ) {
        self.register(Box::new(memory_search::MemorySearchTool::new(index)));
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

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", Value::Null).await;
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
            registry.definitions().is_empty(),
            "empty registry should have no definitions"
        );
    }

    #[test]
    fn registry_with_defaults() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared());
        let defs = registry.definitions();
        assert_eq!(defs.len(), 4, "should have read, write, edit, exec tools");
    }
}

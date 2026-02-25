//! Tool system for agent-invoked operations.

pub mod background;
pub mod cron;
mod edit;
mod exec;
pub(crate) mod file_tracker;
pub mod inbox;
mod line_hash;
pub mod memory_get;
pub mod memory_search;
pub mod path_policy;
pub mod projects;
mod read;
mod registry;
pub mod skills;
mod write;

pub use file_tracker::{FileTracker, SharedFileTracker};
pub use path_policy::{PathPolicy, SharedPathPolicy};
pub use registry::ToolRegistry;

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
#[derive(Clone)]
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

#[cfg(test)]
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
}

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

/// Controls which tools are visible and executable.
///
/// Supports gated tools (only available when the active project opts in
/// via its `tools` field) and subagent preset restrictions via `denied_tools`
/// (permanently blocked regardless of project activation) or `allowed_tools`
/// (only listed tools are available, overrides all other logic).
///
/// Currently no tools are gated by default — all tools are always available.
#[derive(Clone)]
pub struct ToolFilter {
    /// Tool names that require an active project to opt in.
    gated: HashSet<&'static str>,
    /// Currently enabled gated tool names (set by active project's `tools` field).
    enabled: HashSet<String>,
    /// Tools permanently blocked by the subagent preset (`denied_tools`).
    preset_blocked: HashSet<String>,
    /// If set, ONLY these tools are available (`allowed_tools` preset restriction).
    preset_allowed_only: Option<HashSet<String>>,
}

impl ToolFilter {
    /// Create a new tool filter with the given set of gated tool names.
    #[must_use]
    pub fn new(gated: HashSet<&'static str>) -> Self {
        Self {
            gated,
            enabled: HashSet::new(),
            preset_blocked: HashSet::new(),
            preset_allowed_only: None,
        }
    }

    /// Create a new shared tool filter.
    #[must_use]
    pub fn new_shared(gated: HashSet<&'static str>) -> SharedToolFilter {
        Arc::new(RwLock::new(Self::new(gated)))
    }

    /// Create a new shared tool filter with additional preset-denied tools.
    #[must_use]
    pub fn new_shared_with_denied(
        gated: HashSet<&'static str>,
        denied: HashSet<String>,
    ) -> SharedToolFilter {
        Arc::new(RwLock::new(Self {
            gated,
            enabled: HashSet::new(),
            preset_blocked: denied,
            preset_allowed_only: None,
        }))
    }

    /// Create a new shared tool filter that only permits the listed tools.
    #[must_use]
    pub fn new_shared_allowed_only(allowed: HashSet<String>) -> SharedToolFilter {
        Arc::new(RwLock::new(Self {
            gated: HashSet::new(),
            enabled: HashSet::new(),
            preset_blocked: HashSet::new(),
            preset_allowed_only: Some(allowed),
        }))
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

    /// Check whether a tool is available.
    ///
    /// If the preset set `allowed_only`, only listed tools are available.
    /// Otherwise, preset-blocked tools are never available; all others follow
    /// the normal gated/enabled logic.
    #[must_use]
    pub fn is_available(&self, name: &str) -> bool {
        if let Some(allowed) = &self.preset_allowed_only {
            return allowed.contains(name);
        }
        if self.preset_blocked.contains(name) {
            return false;
        }
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
        let mut filter = ToolFilter::new(HashSet::from(["hypothetical_gated"]));
        assert!(
            !filter.is_available("hypothetical_gated"),
            "gated tool should be unavailable by default"
        );
        assert!(
            filter.is_available("read_file"),
            "ungated tools should always be available"
        );
        assert!(
            filter.is_available("exec"),
            "exec should always be available (not gated)"
        );

        filter.enable(&["hypothetical_gated".to_string()]);
        assert!(
            filter.is_available("hypothetical_gated"),
            "gated tool should be available after enabling"
        );

        filter.clear_enabled();
        assert!(
            !filter.is_available("hypothetical_gated"),
            "gated tool should be unavailable after clearing"
        );
    }

    #[tokio::test]
    async fn tool_filter_preset_denied() {
        let filter = ToolFilter::new_shared_with_denied(
            HashSet::new(),
            HashSet::from(["write_file".to_string()]),
        );
        let f = filter.read().await;
        assert!(
            !f.is_available("write_file"),
            "preset-denied tool should be unavailable"
        );
        assert!(
            f.is_available("exec"),
            "exec should be available (not gated)"
        );
        assert!(
            f.is_available("read_file"),
            "non-denied ungated tool should be available"
        );
    }

    #[tokio::test]
    async fn tool_filter_preset_allowed_only() {
        let filter = ToolFilter::new_shared_allowed_only(HashSet::from([
            "read_file".to_string(),
            "write_file".to_string(),
        ]));
        let f = filter.read().await;
        assert!(
            f.is_available("read_file"),
            "listed tool should be available"
        );
        assert!(
            f.is_available("write_file"),
            "listed tool should be available"
        );
        assert!(
            !f.is_available("exec"),
            "unlisted tool should be unavailable"
        );
        assert!(
            !f.is_available("edit_file"),
            "unlisted tool should be unavailable"
        );
    }
}

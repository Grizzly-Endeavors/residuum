//! Tool system for agent-invoked operations.

pub mod actions;
pub mod background;
mod edit;
mod exec;
pub(crate) mod file_bug_report;
pub(crate) mod file_tracker;
pub mod inbox;
mod line_hash;
pub(crate) mod list_conversations;
pub(crate) mod list_endpoints;
pub mod memory_get;
pub mod memory_search;
pub mod message_agent;
pub(crate) mod ollama_web_search;
pub mod path_policy;
mod read;
mod registry;
pub mod send_message;
pub mod skills;
pub(crate) mod submit_feedback;
pub(crate) mod switch_endpoint;
pub(crate) mod web_fetch;
mod write;

pub use file_tracker::{FileTracker, SharedFileTracker};
pub use path_policy::{PathPolicy, SharedPathPolicy};
pub use registry::ToolRegistry;

use std::ffi::OsString;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::inference::{ImageData, ToolDefinition};

/// Shared, reloadable effective `PATH` for spawned children.
///
/// Holds the tool-dir-prepended `PATH` (see [`crate::config::ToolsConfig::effective_path`]),
/// or `None` when no override applies. Read at spawn time by the `exec` tool and
/// MCP stdio spawner so config reloads take effect without rebuilding registries.
pub type SharedToolsPath = Arc<RwLock<Option<OsString>>>;

/// Errors from tool execution.
#[derive(Error, Debug, PartialEq)]
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
    /// Inline images returned by the tool (empty by default).
    pub images: Vec<ImageData>,
}

impl ToolResult {
    /// Create a successful tool result.
    #[must_use]
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images: vec![],
        }
    }

    /// Create a successful tool result with inline images.
    #[must_use]
    pub fn success_with_images(output: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images,
        }
    }

    /// Create an error tool result.
    #[must_use]
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            images: vec![],
        }
    }
}

pub(super) fn require_str<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, ToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("{field} is required")))
}

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
}

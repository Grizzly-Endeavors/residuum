//! Tool system for agent-invoked operations.

pub(crate) mod a2a_task_update;
pub mod actions;
mod agent_keys;
pub mod background;
mod edit;
mod exec;
pub(crate) mod file_bug_report;
pub(crate) mod file_tracker;
pub mod inbox;
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
pub use registry::{SubagentToolDeps, ToolRegistry};

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

    /// A model's tool call carried a name that isn't a plausible identifier
    /// (whitespace, angle brackets, or other markup). Real tool names are
    /// always `[a-zA-Z0-9_-]`, so this shape means the inference provider
    /// failed to parse the model's native tool-call syntax into the
    /// structured `tool_calls` field and leaked raw template markup into the
    /// name instead — dispatching it to the tool or MCP registry would only
    /// ever produce a confusing "unknown tool" lookup failure.
    #[error(
        "malformed tool name '{0}': the provider likely failed to parse your tool-call syntax \
         into structured JSON; retry using the exact tool name as plain text with arguments as \
         a JSON object"
    )]
    MalformedName(String),
}

/// Maximum length of a well-formed tool name.
///
/// Matches the `[a-zA-Z0-9_-]{1,64}` shape OpenAI-compatible APIs require of
/// function names, which every built-in and MCP tool name in this codebase
/// already satisfies.
const MAX_PLAUSIBLE_TOOL_NAME_LEN: usize = 64;

/// Number of characters kept when a malformed tool name is logged or reported
/// back to the model, so a large blob of leaked markup doesn't flood the log
/// or the transcript.
const TOOL_NAME_DISPLAY_LIMIT: usize = 80;

/// Whether `name` could plausibly be a real tool name.
///
/// Every built-in and MCP tool name in this codebase is `[a-zA-Z0-9_-]`, at
/// most 64 characters. Anything else — whitespace, angle brackets, control
/// characters — means a model's tool-call name field was not cleanly parsed
/// by the inference provider, not that a genuinely unknown tool was
/// requested.
#[must_use]
pub fn is_plausible_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PLAUSIBLE_TOOL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Truncate a raw, possibly-malformed tool name for safe logging or display.
#[must_use]
pub fn truncate_tool_name_for_display(name: &str) -> String {
    if name.chars().count() <= TOOL_NAME_DISPLAY_LIMIT {
        name.to_string()
    } else {
        let head: String = name.chars().take(TOOL_NAME_DISPLAY_LIMIT).collect();
        format!("{head}...")
    }
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

    #[test]
    fn plausible_tool_name_accepts_real_tool_names() {
        for name in [
            "write_file",
            "read_file",
            "skill_activate",
            "exec",
            "a",
            "a-b_c9",
        ] {
            assert!(is_plausible_tool_name(name), "'{name}' should be plausible");
        }
    }

    #[test]
    fn plausible_tool_name_rejects_leaked_glm_markup() {
        // Mirrors the malformed name Fireworks/vLLM have shipped for GLM tool
        // calls when the model's native `<arg_key>`/`<arg_value>` template
        // syntax isn't cleanly parsed into structured `tool_calls`.
        let name = "write_file\tcontent</arg_key><arg_value># Hello";
        assert!(
            !is_plausible_tool_name(name),
            "leaked markup should be rejected"
        );
    }

    #[test]
    fn plausible_tool_name_rejects_empty_and_oversized() {
        assert!(!is_plausible_tool_name(""), "empty name should be rejected");
        let too_long = "a".repeat(MAX_PLAUSIBLE_TOOL_NAME_LEN + 1);
        assert!(
            !is_plausible_tool_name(&too_long),
            "oversized name should be rejected"
        );
        let exactly_max = "a".repeat(MAX_PLAUSIBLE_TOOL_NAME_LEN);
        assert!(
            is_plausible_tool_name(&exactly_max),
            "max-length name should be accepted"
        );
    }

    #[test]
    fn plausible_tool_name_rejects_whitespace_and_angle_brackets() {
        for name in ["write file", "write\nfile", "<tool_call>", "write_file>"] {
            assert!(!is_plausible_tool_name(name), "'{name}' should be rejected");
        }
    }

    #[test]
    fn truncate_tool_name_leaves_short_names_untouched() {
        assert_eq!(truncate_tool_name_for_display("write_file"), "write_file");
    }

    #[test]
    fn truncate_tool_name_caps_long_names() {
        let raw = "x".repeat(200);
        let truncated = truncate_tool_name_for_display(&raw);
        assert_eq!(
            truncated.chars().count(),
            TOOL_NAME_DISPLAY_LIMIT + 3,
            "should keep the limit plus the ellipsis"
        );
        assert!(truncated.ends_with("..."), "should be marked as truncated");
    }

    #[test]
    fn malformed_name_error_is_actionable() {
        let err = ToolError::MalformedName("write_file\tcontent</arg_key>".to_string());
        let message = err.to_string();
        assert!(
            message.contains("provider likely failed to parse"),
            "message should explain the likely cause: {message}"
        );
        assert!(
            message.contains("retry using the exact tool name"),
            "message should tell the model how to recover: {message}"
        );
    }
}

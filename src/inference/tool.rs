//! Tool-call result and error types shared by every tool implementation,
//! the MCP bridge and the agent turn loop.

use thiserror::Error;

use super::types::ImageData;

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

    /// A tool call that did not run to completion because the turn was
    /// stopped — either skipped entirely, or interrupted mid-execution.
    ///
    /// Distinct from `error`: this is not a failure, so the transcript
    /// should read it as an intentional stop rather than something that
    /// went wrong.
    #[must_use]
    pub fn cancelled(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images: vec![],
        }
    }
}

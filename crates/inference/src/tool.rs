use thiserror::Error;

use super::types::ImageData;

#[derive(Error, Debug, PartialEq)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    NotFound(String),

    #[error("tool execution failed: {0}")]
    Execution(String),

    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error(
        "malformed tool name '{0}': the provider likely failed to parse your tool-call syntax \
         into structured JSON; retry using the exact tool name as plain text with arguments as \
         a JSON object"
    )]
    MalformedName(String),
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
    pub images: Vec<ImageData>,
}

impl ToolResult {
    #[must_use]
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images: vec![],
        }
    }

    #[must_use]
    pub fn success_with_images(output: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images,
        }
    }

    #[must_use]
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            images: vec![],
        }
    }

    #[must_use]
    pub fn cancelled(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            images: vec![],
        }
    }
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

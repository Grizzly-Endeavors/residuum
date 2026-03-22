//! Shell command execution tool for the agent.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Maximum output size from a command (100KB).
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Tool that executes shell commands.
pub struct ExecTool;

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Execute a shell command and return its output. Commands run via \
                          `sh -c` with a configurable timeout (default 120 seconds)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 120)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'command' argument".to_string())
            })?;

        let timeout_secs = arguments
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        tracing::debug!(command = %command, timeout_secs = %timeout_secs, "exec");

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            Command::new("sh").arg("-c").arg(command).output(),
        )
        .await;

        match result {
            Err(_elapsed) => Ok(ToolResult::error(format!(
                "command timed out after {timeout_secs} seconds"
            ))),
            Ok(Err(e)) => Ok(ToolResult::error(format!("failed to execute command: {e}"))),
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut result_text = String::new();

                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push('\n');
                    }
                    result_text.push_str("STDERR:\n");
                    result_text.push_str(&stderr);
                }

                // Truncate if too large (use floor_char_boundary to avoid panic on multi-byte)
                if result_text.len() > MAX_OUTPUT_BYTES {
                    result_text.truncate(result_text.floor_char_boundary(MAX_OUTPUT_BYTES));
                    result_text.push_str("\n... (output truncated)");
                }

                if output.status.success() {
                    if result_text.is_empty() {
                        result_text = "(no output)".to_string();
                    }
                    Ok(ToolResult::success(result_text))
                } else {
                    let code = output
                        .status
                        .code()
                        .map_or_else(|| "unknown".to_string(), |c| c.to_string());
                    if result_text.is_empty() {
                        Ok(ToolResult::error(format!(
                            "command exited with code {code}"
                        )))
                    } else {
                        Ok(ToolResult::error(format!(
                            "command exited with code {code}\n{result_text}"
                        )))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exec_simple_command() {
        let tool = ExecTool;
        let result = tool
            .execute(serde_json::json!({ "command": "echo hello" }))
            .await
            .unwrap();

        assert!(!result.is_error, "echo should succeed");
        assert!(
            result.output.contains("hello"),
            "output should contain echo text"
        );
    }

    #[tokio::test]
    async fn exec_failing_command() {
        let tool = ExecTool;
        let result = tool
            .execute(serde_json::json!({ "command": "false" }))
            .await
            .unwrap();

        assert!(result.is_error, "false command should be error result");
    }

    #[tokio::test]
    async fn exec_timeout() {
        let tool = ExecTool;
        let result = tool
            .execute(serde_json::json!({
                "command": "sleep 10",
                "timeout_secs": 1
            }))
            .await
            .unwrap();

        assert!(result.is_error, "timed out command should be error");
        assert!(
            result.output.contains("timed out"),
            "error should mention timeout"
        );
    }

    #[tokio::test]
    async fn exec_missing_command() {
        let tool = ExecTool;
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing command should return ToolError");
    }

    #[tokio::test]
    async fn exec_stderr_output() {
        let tool = ExecTool;
        let result = tool
            .execute(serde_json::json!({ "command": "echo error >&2" }))
            .await
            .unwrap();

        // The command succeeds (exit code 0) even with stderr output
        assert!(!result.is_error, "stderr-only with exit 0 is success");
        assert!(
            result.output.contains("STDERR"),
            "should label stderr output"
        );
    }

    #[test]
    fn exec_tool_definition() {
        let tool = ExecTool;
        assert_eq!(tool.name(), "exec", "tool name should match");
    }
}

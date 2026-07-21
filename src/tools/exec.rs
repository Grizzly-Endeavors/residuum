//! Shell command execution tool for the agent.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{SharedToolsPath, Tool, ToolError, ToolResult};
use crate::models::ToolDefinition;

/// Maximum output size from a command (100KB).
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Tool that executes shell commands.
pub struct ExecTool {
    /// Effective `PATH` (tool dirs prepended) applied to spawned children.
    /// `None` leaves the inherited process `PATH` untouched. Read per call so
    /// config reloads take effect without rebuilding the tool.
    tools_path: Option<SharedToolsPath>,
}

impl ExecTool {
    /// Create an exec tool.
    ///
    /// Pass the shared tools-`PATH` handle to prepend the configured tool
    /// directories to spawned commands' `PATH`; pass `None` to inherit the
    /// process `PATH` unchanged.
    #[must_use]
    pub fn new(tools_path: Option<SharedToolsPath>) -> Self {
        Self { tools_path }
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: format!(
                "Execute a shell command and return its output. Commands run via \
                 {} with a configurable timeout (default 120 seconds).",
                if cfg!(windows) { "`cmd /C`" } else { "`sh -c`" }
            ),
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

        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", command]);
            c
        };

        // Prepend configured tool dirs to the child's PATH (read live so config
        // reloads apply). Leaves PATH inherited when no override is configured.
        if let Some(handle) = &self.tools_path
            && let Some(path) = handle.read().await.as_ref()
        {
            cmd.env("PATH", path);
        }

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

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

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_resolves_binary_from_tools_path() {
        use std::os::unix::fs::PermissionsExt;

        // A uniquely-named script in a temp dir that is NOT on the base PATH.
        let dir = std::env::temp_dir().join(format!(
            "residuum-exec-tools-{}-{}",
            std::process::id(),
            "toolbox"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("residuum_only_in_tools_dir");
        std::fs::write(&script, "#!/bin/sh\necho tool-ran\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        // Effective PATH = tools dir prepended to the inherited PATH.
        let mut parts = vec![dir.clone()];
        if let Some(inherited) = std::env::var_os("PATH") {
            parts.extend(std::env::split_paths(&inherited));
        }
        let path = std::env::join_paths(parts).unwrap();
        let handle: SharedToolsPath = std::sync::Arc::new(tokio::sync::RwLock::new(Some(path)));

        // With the handle, the bare binary name resolves.
        let tool = ExecTool::new(Some(handle));
        let result = tool
            .execute(serde_json::json!({ "command": "residuum_only_in_tools_dir" }))
            .await
            .unwrap();
        assert!(
            !result.is_error,
            "binary in tools dir should resolve and run: {}",
            result.output
        );
        assert!(
            result.output.contains("tool-ran"),
            "output should be from the tools-dir script: {}",
            result.output
        );

        // Without the handle, the same bare name is not on PATH → fails.
        let bare = ExecTool::new(None);
        let missing = bare
            .execute(serde_json::json!({ "command": "residuum_only_in_tools_dir" }))
            .await
            .unwrap();
        assert!(
            missing.is_error,
            "binary should not resolve without the tools PATH: {}",
            missing.output
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn exec_simple_command() {
        let tool = ExecTool::new(None);
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
        let tool = ExecTool::new(None);
        let result = tool
            .execute(serde_json::json!({ "command": "false" }))
            .await
            .unwrap();

        assert!(result.is_error, "false command should be error result");
        assert!(
            result.output.contains("code"),
            "output should mention exit code: {}",
            result.output
        );
        assert!(
            result.output.chars().any(|c| c.is_ascii_digit()),
            "output should contain a numeric exit code: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn exec_timeout() {
        let tool = ExecTool::new(None);
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
        let tool = ExecTool::new(None);
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing command should return ToolError");
    }

    #[tokio::test]
    async fn exec_stderr_output() {
        let tool = ExecTool::new(None);
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

    #[tokio::test]
    async fn exec_output_truncated() {
        let tool = ExecTool::new(None);
        // Generate more than 100KB of output
        let result = tool
            .execute(serde_json::json!({
                "command": "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'x'"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "command should succeed");
        assert!(
            result.output.contains("(output truncated)"),
            "large output should be truncated: output len = {}",
            result.output.len()
        );
        assert!(
            result.output.len() < 200 * 1024,
            "truncated output should be smaller than raw output"
        );
    }

    #[test]
    fn exec_tool_definition() {
        let tool = ExecTool::new(None);
        assert_eq!(tool.name(), "exec", "tool name should match");
    }
}

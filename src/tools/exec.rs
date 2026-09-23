//! Shell command execution tool for the agent.

use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{SharedToolsPath, Tool, ToolError, ToolResult};
use crate::agent_keys::{
    AgentKeysSnapshot, KeyCreator, Redactor, SharedAgentKeys, env_var_for, validate_name,
};
use crate::inference::ToolDefinition;

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
    /// Agent key store backing the `keys` and `store_output_as` parameters.
    /// `None` makes both parameters fail with an explanation.
    agent_keys: Option<SharedAgentKeys>,
}

/// Where a minted key goes: the `store_output_as` parameter.
struct StoreTarget {
    name: String,
    description: Option<String>,
}

impl ExecTool {
    /// Create an exec tool.
    ///
    /// Pass the shared tools-`PATH` handle to prepend the configured tool
    /// directories to spawned commands' `PATH`; pass `None` to inherit the
    /// process `PATH` unchanged. Pass the agent key store to enable the
    /// `keys` and `store_output_as` parameters.
    #[must_use]
    pub fn new(tools_path: Option<SharedToolsPath>, agent_keys: Option<SharedAgentKeys>) -> Self {
        Self {
            tools_path,
            agent_keys,
        }
    }

    async fn agent_key_snapshot(&self) -> Result<Arc<AgentKeysSnapshot>, ToolResult> {
        let Some(keys) = &self.agent_keys else {
            return Err(ToolResult::error(
                "agent keys are not available in this context. Nothing was run.",
            ));
        };
        keys.snapshot().await.map_err(|e| {
            ToolResult::error(format!(
                "couldn't read the agent key store ({e}). Nothing was run."
            ))
        })
    }

    async fn redactor(&self) -> Redactor {
        match &self.agent_keys {
            Some(keys) => keys.redactor().await,
            None => Redactor::default(),
        }
    }

    /// Resolve `keys` to `(env var, value)` pairs, failing on any unknown name.
    async fn resolve_keys(&self, names: &[String]) -> Result<Vec<(String, String)>, ToolResult> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let snapshot = self.agent_key_snapshot().await?;
        let unknown: Vec<&str> = names
            .iter()
            .filter(|n| snapshot.store.value(n).is_none())
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            let available = snapshot.store.names();
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            return Err(ToolResult::error(format!(
                "unknown agent key(s): {}. Available: {available}. Nothing was run.",
                unknown.join(", ")
            )));
        }
        Ok(names
            .iter()
            .filter_map(|n| {
                snapshot
                    .store
                    .value(n)
                    .map(|v| (env_var_for(n), v.to_string()))
            })
            .collect())
    }

    /// Refuse a `store_output_as` target before running anything, so a
    /// minting command isn't run only to have its output discarded.
    async fn check_store_target(&self, target: &StoreTarget) -> Result<(), ToolResult> {
        validate_name(&target.name)
            .map_err(|e| ToolResult::error(format!("{e}. Nothing was run.")))?;
        let snapshot = self.agent_key_snapshot().await?;
        if snapshot.store.creator(&target.name) == Some(KeyCreator::User) {
            return Err(ToolResult::error(format!(
                "agent key '{}' was created by the user and can't be replaced; choose another \
                 name. Nothing was run.",
                target.name
            )));
        }
        Ok(())
    }

    /// Store a finished command's stdout as a key and report the outcome,
    /// never returning the stdout itself.
    async fn store_output(&self, target: &StoreTarget, output: &Output) -> ToolResult {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let value = stdout.trim_end_matches(['\r', '\n']);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = self
            .redactor()
            .await
            .with_entry(&target.name, value)
            .redact(&stderr)
            .into_owned();

        if !output.status.success() {
            return ToolResult::error(with_stderr(
                format!(
                    "command exited with code {}; nothing was stored and stdout was discarded",
                    exit_code_label(output)
                ),
                &stderr,
            ));
        }
        if value.is_empty() {
            return ToolResult::error(with_stderr(
                "command produced no stdout; nothing was stored".to_string(),
                &stderr,
            ));
        }
        let Some(keys) = &self.agent_keys else {
            return ToolResult::error("agent keys are not available in this context");
        };

        match keys
            .set(
                &target.name,
                value,
                target.description.as_deref(),
                KeyCreator::Agent,
            )
            .await
        {
            Ok(()) => ToolResult::success(with_stderr(
                format!(
                    "stored agent key '{name}' ({len} bytes). Use it with keys: [\"{name}\"] as ${var}.",
                    name = target.name,
                    len = value.len(),
                    var = env_var_for(&target.name)
                ),
                &stderr,
            )),
            Err(e) => {
                tracing::warn!(error = %e, key = %target.name, "failed to store minted agent key");
                ToolResult::error(with_stderr(
                    format!(
                        "command succeeded but its output was not stored: {e}. stdout was discarded."
                    ),
                    &stderr,
                ))
            }
        }
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
                 {} with a configurable timeout (default 120 seconds). Use `keys` to \
                 expose agent keys as environment variables (see agent_keys_list); use \
                 `store_output_as` to store stdout as a new agent key instead of returning it.",
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
                    },
                    "keys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent key names to expose to this command, each as its uppercased name (github_token -> $GITHUB_TOKEN). Values are redacted from all output."
                    },
                    "store_output_as": {
                        "type": "object",
                        "description": "Store the command's stdout as an agent key instead of returning it. Stored only when the command exits 0 with non-empty stdout; stderr is still returned.",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Key name: lowercase letters, digits, and underscores, starting with a letter"
                            },
                            "description": {
                                "type": "string",
                                "description": "What the key is and what it grants"
                            }
                        },
                        "required": ["name"]
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

        let key_names = parse_key_names(&arguments)?;
        let store_target = parse_store_target(&arguments)?;

        if let Some(target) = &store_target
            && let Err(refusal) = self.check_store_target(target).await
        {
            return Ok(refusal);
        }
        let key_env = match self.resolve_keys(&key_names).await {
            Ok(env) => env,
            Err(refusal) => return Ok(refusal),
        };

        let logged_command = self.redactor().await.redact(command).into_owned();
        tracing::debug!(
            command = %logged_command,
            timeout_secs = %timeout_secs,
            keys = ?key_names,
            store_output_as = store_target.as_ref().map(|t| t.name.as_str()),
            "exec"
        );

        let mut cmd = shell_command(command);

        // Prepend configured tool dirs to the child's PATH (read live so config
        // reloads apply). Leaves PATH inherited when no override is configured.
        if let Some(handle) = &self.tools_path
            && let Some(path) = handle.read().await.as_ref()
        {
            cmd.env("PATH", path);
        }
        for (var, value) in &key_env {
            cmd.env(var, value);
        }

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

        match result {
            Err(_elapsed) => Ok(ToolResult::error(format!(
                "command timed out after {timeout_secs} seconds"
            ))),
            Ok(Err(e)) => Ok(ToolResult::error(format!("failed to execute command: {e}"))),
            Ok(Ok(output)) => match &store_target {
                Some(target) => Ok(self.store_output(target, &output).await),
                None => Ok(format_output(&output)),
            },
        }
    }
}

/// Build the platform shell invocation for `command`.
fn shell_command(command: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    }
}

fn parse_key_names(arguments: &Value) -> Result<Vec<String>, ToolError> {
    let invalid =
        || ToolError::InvalidArguments("'keys' must be an array of key names".to_string());
    match arguments.get("keys") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(invalid))
            .collect(),
        Some(_) => Err(invalid()),
    }
}

fn parse_store_target(arguments: &Value) -> Result<Option<StoreTarget>, ToolError> {
    match arguments.get("store_output_as") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(obj)) => {
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolError::InvalidArguments("'store_output_as.name' is required".to_string())
                })?
                .to_string();
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Some(StoreTarget { name, description }))
        }
        Some(_) => Err(ToolError::InvalidArguments(
            "'store_output_as' must be an object with a 'name'".to_string(),
        )),
    }
}

fn exit_code_label(output: &Output) -> String {
    output
        .status
        .code()
        .map_or_else(|| "unknown".to_string(), |c| c.to_string())
}

/// Append a labelled stderr block to `message` when there is any.
fn with_stderr(message: String, stderr: &str) -> String {
    if stderr.is_empty() {
        return message;
    }
    truncate_output(format!("{message}\nSTDERR:\n{stderr}"))
}

fn truncate_output(mut text: String) -> String {
    // floor_char_boundary avoids splitting a multi-byte character.
    if text.len() > MAX_OUTPUT_BYTES {
        text.truncate(text.floor_char_boundary(MAX_OUTPUT_BYTES));
        text.push_str("\n... (output truncated)");
    }
    text
}

/// Combine stdout and stderr into the tool result for a normal run.
fn format_output(output: &Output) -> ToolResult {
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
    let mut result_text = truncate_output(result_text);

    if output.status.success() {
        if result_text.is_empty() {
            result_text = "(no output)".to_string();
        }
        ToolResult::success(result_text)
    } else {
        let code = exit_code_label(output);
        if result_text.is_empty() {
            ToolResult::error(format!("command exited with code {code}"))
        } else {
            ToolResult::error(format!("command exited with code {code}\n{result_text}"))
        }
    }
}

#[cfg(test)]
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
        let tool = ExecTool::new(Some(handle), None);
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
        let bare = ExecTool::new(None, None);
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
        let tool = ExecTool::new(None, None);
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
        let tool = ExecTool::new(None, None);
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
        let tool = ExecTool::new(None, None);
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
        let tool = ExecTool::new(None, None);
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing command should return ToolError");
    }

    #[tokio::test]
    async fn exec_stderr_output() {
        let tool = ExecTool::new(None, None);
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
        let tool = ExecTool::new(None, None);
        // Generate more than 100KB of output
        let command = if cfg!(windows) {
            "powershell -NoProfile -Command \"'x' * 204800\""
        } else {
            "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'x'"
        };
        let result = tool
            .execute(serde_json::json!({ "command": command }))
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

    #[cfg(unix)]
    mod agent_keys {
        use super::*;
        use crate::agent_keys::AgentKeys;

        async fn tool_with_key(dir: &std::path::Path) -> (ExecTool, SharedAgentKeys) {
            let keys = AgentKeys::new_shared(dir);
            keys.set("api_key", "sk-test-abcdef123", None, KeyCreator::User)
                .await
                .unwrap();
            (ExecTool::new(None, Some(Arc::clone(&keys))), keys)
        }

        #[tokio::test]
        async fn named_key_is_injected_as_env_var() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, _keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({
                    "command": "test \"$API_KEY\" = sk-test-abcdef123 && echo matched",
                    "keys": ["api_key"]
                }))
                .await
                .unwrap();
            assert!(
                !result.is_error,
                "command should succeed: {}",
                result.output
            );
            assert!(
                result.output.contains("matched"),
                "child should see the key value: {}",
                result.output
            );
        }

        #[tokio::test]
        async fn unnamed_keys_are_not_injected() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, _keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({ "command": "echo \"[${API_KEY:-unset}]\"" }))
                .await
                .unwrap();
            assert!(
                result.output.contains("[unset]"),
                "a key not named in `keys` must not reach the child: {}",
                result.output
            );
        }

        #[tokio::test]
        async fn unknown_key_fails_before_running() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, _keys) = tool_with_key(dir.path()).await;
            let marker = dir.path().join("ran");
            let result = tool
                .execute(serde_json::json!({
                    "command": format!("touch {}", marker.display()),
                    "keys": ["nope"]
                }))
                .await
                .unwrap();
            assert!(result.is_error, "unknown key should fail");
            assert!(
                result.output.contains("unknown agent key(s): nope")
                    && result.output.contains("Available: api_key"),
                "error should name the unknown key and the available ones: {}",
                result.output
            );
            assert!(!marker.exists(), "command must not run");
        }

        #[tokio::test]
        async fn keys_without_store_explain_unavailability() {
            let result = ExecTool::new(None, None)
                .execute(serde_json::json!({ "command": "true", "keys": ["api_key"] }))
                .await
                .unwrap();
            assert!(result.is_error, "should fail without a store");
            assert!(
                result.output.contains("not available"),
                "should say keys are unavailable: {}",
                result.output
            );
        }

        #[tokio::test]
        async fn store_output_as_saves_stdout_without_returning_it() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({
                    "command": "echo minted-token-xyz789; echo progress >&2",
                    "store_output_as": { "name": "minted", "description": "from test" }
                }))
                .await
                .unwrap();
            assert!(!result.is_error, "store should succeed: {}", result.output);
            assert!(
                !result.output.contains("minted-token-xyz789"),
                "stdout must never be returned: {}",
                result.output
            );
            assert!(
                result.output.contains("stored agent key 'minted'")
                    && result.output.contains("$MINTED"),
                "confirmation should name the key and env var: {}",
                result.output
            );
            assert!(
                result.output.contains("progress"),
                "stderr should still be returned: {}",
                result.output
            );
            let snap = keys.snapshot().await.unwrap();
            assert_eq!(
                snap.store.value("minted"),
                Some("minted-token-xyz789"),
                "trailing newline trimmed, value stored"
            );
            assert_eq!(
                snap.store.creator("minted"),
                Some(KeyCreator::Agent),
                "minted keys are agent-owned"
            );
        }

        #[tokio::test]
        async fn store_output_as_redacts_new_value_from_stderr() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, _keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({
                    "command": "echo leaked-token-4567; echo leaked-token-4567 >&2",
                    "store_output_as": { "name": "minted" }
                }))
                .await
                .unwrap();
            assert!(
                !result.output.contains("leaked-token-4567")
                    && result.output.contains("[agent-key:minted]"),
                "the minted value must be redacted from stderr too: {}",
                result.output
            );
        }

        #[tokio::test]
        async fn failed_command_stores_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({
                    "command": "echo partial-token-000; exit 3",
                    "store_output_as": { "name": "minted" }
                }))
                .await
                .unwrap();
            assert!(result.is_error, "non-zero exit should be an error");
            assert!(
                result.output.contains("nothing was stored")
                    && !result.output.contains("partial-token-000"),
                "should store nothing and discard stdout: {}",
                result.output
            );
            assert!(
                keys.snapshot()
                    .await
                    .unwrap()
                    .store
                    .value("minted")
                    .is_none(),
                "nothing should be stored"
            );
        }

        #[tokio::test]
        async fn store_over_user_key_is_refused_before_running() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, keys) = tool_with_key(dir.path()).await;
            let marker = dir.path().join("ran");
            let result = tool
                .execute(serde_json::json!({
                    "command": format!("touch {} && echo replacement-value", marker.display()),
                    "store_output_as": { "name": "api_key" }
                }))
                .await
                .unwrap();
            assert!(result.is_error, "overwriting a user key should fail");
            assert!(!marker.exists(), "command must not run");
            assert_eq!(
                keys.snapshot().await.unwrap().store.value("api_key"),
                Some("sk-test-abcdef123"),
                "user key must be untouched"
            );
        }
    }

    #[test]
    fn exec_tool_definition() {
        let tool = ExecTool::new(None, None);
        assert_eq!(tool.name(), "exec", "tool name should match");
    }
}

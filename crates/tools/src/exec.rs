//! Shell command execution tool for the agent.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::publish_notice;
use residuum_agent::Tool;
use residuum_bus::Publisher;
use residuum_checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger};
use residuum_config::agent_keys::{
    AgentKeysSnapshot, KeyCreator, Redactor, SharedAgentKeys, env_var_for, validate_name,
};
use residuum_config::{SharedToolsPath, apply_tools_path};
use residuum_core::process::{
    Capture, Captured, CapturedOutput, RunOutcome, run_with_timeout_and_cancel, shell_command,
};
use residuum_inference::ToolDefinition;
use residuum_inference::{ToolError, ToolResult};

/// Maximum size of the output shown from one command, notes included.
const MAX_OUTPUT_BYTES: usize = crate::PAGE_BUDGET;

/// Most of stderr shown, as its tail. Stderr is what a failing command says
/// last and most plainly, so it gets a quarter of the cap.
const STDERR_BUDGET_BYTES: usize = MAX_OUTPUT_BYTES / 4;

/// Space held back from the cap for the section labels and the omission notes
/// that surround the shown streams.
const NOTE_RESERVE_BYTES: usize = 256;

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
    /// Checkpoint engine, checkpointed before `store_output_as` mints a new
    /// agent key. `None` means minting isn't checkpointed.
    checkpoints: Option<Arc<CheckpointEngine>>,
    /// Publishes a notice when `store_output_as` overwrites a key the user
    /// created. `None` (e.g. in tests) means the overwrite still happens but
    /// goes unannounced.
    publisher: Option<Publisher>,
    /// The agent's workspace root. Every spawned command starts here
    /// (`Command::current_dir`) rather than inheriting the process's current
    /// directory, which is shared across every agent hosted in the same
    /// process.
    workspace_root: PathBuf,
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
    /// `keys` and `store_output_as` parameters. Every spawned command starts
    /// in `workspace_root`.
    #[must_use]
    pub fn new(
        tools_path: Option<SharedToolsPath>,
        agent_keys: Option<SharedAgentKeys>,
        checkpoints: Option<Arc<CheckpointEngine>>,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            tools_path,
            agent_keys,
            checkpoints,
            publisher: None,
            workspace_root,
        }
    }

    /// Publish a notice naming the key whenever `store_output_as` overwrites
    /// a key the user created.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Publisher) -> Self {
        self.publisher = Some(publisher);
        self
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
    /// minting command isn't run only to have its output discarded — a bad
    /// name is the only thing that still fails here. The agent may
    /// overwrite a key the user created (checkpointed and reported to the
    /// user; see [`Self::store_output`]).
    async fn check_store_target(&self, target: &StoreTarget) -> Result<(), ToolResult> {
        validate_name(&target.name)
            .map_err(|e| ToolResult::error(format!("{e}. Nothing was run.")))?;
        self.agent_key_snapshot().await?;
        Ok(())
    }

    /// Store a finished command's stdout as a key and report the outcome,
    /// never returning the stdout itself.
    async fn store_output(&self, target: &StoreTarget, output: &CapturedOutput) -> ToolResult {
        // Captured whole for a store, so the head is the entire stdout.
        let stdout = String::from_utf8_lossy(&output.stdout.head);
        let value = stdout.trim_end_matches(['\r', '\n']);
        let stderr = self
            .redactor()
            .await
            .with_entry(&target.name, value)
            .redact(&shown_stderr(&output.stderr))
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

        // Read before the write to know whether this overwrites a key the
        // user created — that's the only case worth telling the user
        // about; a failed snapshot just means the notice is skipped, not
        // that the mint itself fails.
        let overwrites_user_key = keys
            .snapshot()
            .await
            .is_ok_and(|s| s.store.creator(&target.name) == Some(KeyCreator::User));

        if let Some(checkpoints) = &self.checkpoints {
            checkpoints
                .checkpoint_config_before_write(CheckpointContext::system(
                    CheckpointTrigger::PreConfigWrite,
                    format!("exec minted agent key '{}'", target.name),
                ))
                .await;
        }

        match keys
            .set(
                &target.name,
                value,
                target.description.as_deref(),
                KeyCreator::Agent,
            )
            .await
        {
            Ok(warning) => {
                if overwrites_user_key {
                    publish_notice(
                        self.publisher.as_ref(),
                        format!(
                            "The agent replaced agent key '{}', which you created. Restore it \
                             from Settings → All agents → History if this wasn't intended.",
                            target.name
                        ),
                    )
                    .await;
                }
                let mut message = format!(
                    "stored agent key '{name}' ({len} bytes). Use it with keys: [\"{name}\"] as ${var}.",
                    name = target.name,
                    len = value.len(),
                    var = env_var_for(&target.name)
                );
                if let Some(warning) = warning {
                    _ = write!(message, " Warning: {warning}.");
                }
                ToolResult::success(with_stderr(message, &stderr))
            }
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

    /// Build the message for a command that timed out or was cancelled:
    /// `reason` plus whatever stdout/stderr it had already produced.
    ///
    /// When `store_output_as` was requested, stdout is discarded rather than
    /// shown — mirroring `store_output`'s failure path, stdout is never
    /// revealed once `store_output_as` is on the call, success or not — and
    /// the partial stdout is still folded into the redactor as a would-be
    /// secret value so it can't leak through stderr either.
    async fn captured_output_message(
        &self,
        reason: &str,
        output: &CapturedOutput,
        store_target: Option<&StoreTarget>,
    ) -> String {
        if let Some(target) = store_target {
            let stdout = String::from_utf8_lossy(&output.stdout.head);
            let stderr = self
                .redactor()
                .await
                .with_entry(&target.name, &stdout)
                .redact(&shown_stderr(&output.stderr))
                .into_owned();
            with_stderr(
                format!("{reason}; nothing was stored and stdout was discarded"),
                &stderr,
            )
        } else {
            format_captured_output(reason, output)
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
                 {} with a configurable timeout (default 120 seconds). Output over \
                 {MAX_OUTPUT_BYTES} bytes keeps the start of stdout and the end of stderr (and \
                 the end of stdout when the command fails) and says how many bytes were left \
                 out. Terminal colour codes are removed. Use `keys` to expose agent keys as \
                 environment variables (see agent_keys_list); use `store_output_as` to store \
                 stdout as a new agent key instead of returning it.",
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
        // No caller-supplied cancellation: an inert token that is never
        // cancelled behaves exactly like the old, non-racing `execute()`.
        self.execute_cancellable(arguments, &CancellationToken::new())
            .await
    }

    async fn execute_cancellable(
        &self,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, ToolError> {
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
        cmd.current_dir(&self.workspace_root);

        if let Some(handle) = &self.tools_path {
            apply_tools_path(&mut cmd, handle).await;
        }
        for (var, value) in &key_env {
            cmd.env(var, value);
        }

        // A store keeps stdout whole; otherwise stdout keeps its head and tail,
        // since a failing command's end is shown and its start gives context.
        let stdout_capture = if store_target.is_some() {
            Capture::All
        } else {
            Capture::HeadAndTail {
                head: MAX_OUTPUT_BYTES,
                tail: MAX_OUTPUT_BYTES,
            }
        };
        let (outcome, output) = match run_with_timeout_and_cancel(
            cmd,
            Duration::from_secs(timeout_secs),
            cancel,
            (stdout_capture, Capture::Tail(STDERR_BUDGET_BYTES)),
        )
        .await
        {
            Ok(pair) => pair,
            Err(e) => return Ok(ToolResult::error(format!("failed to execute command: {e}"))),
        };

        Ok(match outcome {
            RunOutcome::Finished => match &store_target {
                Some(target) => self.store_output(target, &output).await,
                None => format_output(&output),
            },
            RunOutcome::TimedOut => {
                let reason = format!(
                    "command timed out after {timeout_secs} seconds; its process tree was killed"
                );
                ToolResult::error(
                    self.captured_output_message(&reason, &output, store_target.as_ref())
                        .await,
                )
            }
            RunOutcome::Cancelled => {
                let reason = "the turn was stopped while this command was running; its \
                               process tree was killed"
                    .to_string();
                ToolResult::cancelled(
                    self.captured_output_message(&reason, &output, store_target.as_ref())
                        .await,
                )
            }
        })
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

fn exit_code_label(output: &CapturedOutput) -> String {
    output
        .status
        .code()
        .map_or_else(|| "unknown".to_string(), |c| c.to_string())
}

/// Build the message for a timed-out or cancelled command that has no
/// `store_output_as`: `reason` plus whatever stdout/stderr it produced
/// before it ended, labelled as partial rather than final.
fn format_captured_output(reason: &str, output: &CapturedOutput) -> String {
    let (stdout, stderr) = shown_streams(&output.stdout, &output.stderr, true);

    let mut text = reason.to_string();
    if !stdout.is_empty() {
        text.push_str("\nSTDOUT so far:\n");
        text.push_str(&stdout);
    }
    if !stderr.is_empty() {
        text.push_str("\nSTDERR so far:\n");
        text.push_str(&stderr);
    }
    text
}

/// Append a labelled stderr block to `message` when there is any.
fn with_stderr(message: String, stderr: &str) -> String {
    if stderr.is_empty() {
        return message;
    }
    format!("{message}\nSTDERR:\n{stderr}")
}

/// The text of stderr as a result shows it, before any redaction.
fn shown_stderr(stderr: &Captured) -> String {
    shape_stream("stderr", stderr, 0, STDERR_BUDGET_BYTES)
}

/// The text of each stream a result shows, within the output cap.
///
/// stderr shows its tail. stdout takes the rest of the cap: its head alone
/// when the command succeeded, and its head plus tail when it failed, with
/// the head getting a quarter of stdout's share.
fn shown_streams(stdout: &Captured, stderr: &Captured, failed: bool) -> (String, String) {
    let stderr_text = shown_stderr(stderr);
    let stdout_budget = MAX_OUTPUT_BYTES.saturating_sub(NOTE_RESERVE_BYTES + stderr_text.len());
    let (head, tail) = if failed {
        let head = stdout_budget / 4;
        (head, stdout_budget - head)
    } else {
        (stdout_budget, 0)
    };
    (shape_stream("stdout", stdout, head, tail), stderr_text)
}

/// A stream as shown: its first `head` bytes and last `tail` bytes, with a
/// note between them saying how many bytes were left out.
///
/// The kept bytes are one ordered stream: a capture fills its head before its
/// tail, so a stream shorter than the capture's head has an empty tail, and
/// the shown end is taken from wherever the stream ends. When the capture
/// dropped bytes, a side shorter than its budget lends what it doesn't use to
/// the other.
fn shape_stream(name: &str, stream: &Captured, head: usize, tail: usize) -> String {
    let joined;
    let (shown_head, shown_tail): (&[u8], &[u8]) = if stream.omitted == 0 {
        joined = [stream.head.as_slice(), stream.tail.as_slice()].concat();
        if joined.len() <= head + tail {
            (&joined, &[])
        } else {
            (prefix(&joined, head), suffix(&joined, tail))
        }
    } else {
        let shown_tail = suffix(&stream.tail, tail + head.saturating_sub(stream.head.len()));
        let shown_head = prefix(&stream.head, head + tail.saturating_sub(shown_tail.len()));
        (shown_head, shown_tail)
    };
    let omitted = stream.total_bytes() - shown_head.len() - shown_tail.len();

    let mut text = String::from_utf8_lossy(shown_head).into_owned();
    if omitted > 0 {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        _ = writeln!(text, "[… {omitted} bytes of {name} omitted …]");
    }
    text.push_str(&String::from_utf8_lossy(shown_tail));
    text
}

fn prefix(bytes: &[u8], len: usize) -> &[u8] {
    bytes.get(..len.min(bytes.len())).unwrap_or(bytes)
}

fn suffix(bytes: &[u8], len: usize) -> &[u8] {
    bytes
        .get(bytes.len().saturating_sub(len)..)
        .unwrap_or(bytes)
}

/// Combine stdout and stderr into the tool result for a normal run.
fn format_output(output: &CapturedOutput) -> ToolResult {
    let failed = !output.status.success();
    let (mut result_text, stderr) = shown_streams(&output.stdout, &output.stderr, failed);
    if !stderr.is_empty() {
        if !result_text.is_empty() {
            result_text.push('\n');
        }
        result_text.push_str("STDERR:\n");
        result_text.push_str(&stderr);
    }

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
    use residuum_core::testing::wait;

    /// A workspace root for tests that don't care where commands start —
    /// the system temp dir, which always exists.
    fn workspace_root() -> PathBuf {
        std::env::temp_dir()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_resolves_binary_from_tools_path() {
        // A uniquely-named link to `/bin/sh` in a temp dir that is NOT on the
        // base PATH. It is a link, not a script written here: executing a file
        // this process just wrote fails with "Text file busy" when a test on
        // another thread forks while the file is still open for writing,
        // because the forked child holds a copy of the descriptor until it
        // execs. `/bin/sh` is used rather than `/bin/echo` because uutils
        // coreutils (Ubuntu 26.04+) checks `argv[0]` against the binary name
        // and rejects custom symlink names.
        let dir = std::env::temp_dir().join(format!(
            "residuum-exec-tools-{}-{}",
            std::process::id(),
            "toolbox"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("residuum_only_in_tools_dir");
        std::fs::remove_file(&link).ok();
        std::os::unix::fs::symlink("/bin/sh", &link).unwrap();

        // Effective PATH = tools dir prepended to the inherited PATH.
        let mut parts = vec![dir.clone()];
        if let Some(inherited) = std::env::var_os("PATH") {
            parts.extend(std::env::split_paths(&inherited));
        }
        let path = std::env::join_paths(parts).unwrap();
        let handle: SharedToolsPath = std::sync::Arc::new(tokio::sync::RwLock::new(Some(path)));

        // With the handle, the bare binary name resolves.
        let tool = ExecTool::new(Some(handle), None, None, workspace_root());
        let result = tool
            .execute(
                serde_json::json!({ "command": "residuum_only_in_tools_dir -c 'echo tool-ran'" }),
            )
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
        let bare = ExecTool::new(None, None, None, workspace_root());
        let missing = bare
            .execute(
                serde_json::json!({ "command": "residuum_only_in_tools_dir -c 'echo tool-ran'" }),
            )
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
        let tool = ExecTool::new(None, None, None, workspace_root());
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

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_starts_in_the_workspace_root_not_the_process_cwd() {
        // The process's own current directory here is whatever `cargo test`
        // was invoked from, not this tempdir — so the command only sees
        // this directory if `current_dir` is set explicitly.
        let dir = tempfile::tempdir().unwrap();
        let expected = tokio::fs::canonicalize(dir.path()).await.unwrap();

        let tool = ExecTool::new(None, None, None, dir.path().to_path_buf());
        let result = tool
            .execute(serde_json::json!({ "command": "pwd" }))
            .await
            .unwrap();

        assert!(!result.is_error, "pwd should succeed: {}", result.output);
        let printed = tokio::fs::canonicalize(result.output.trim()).await.unwrap();
        assert_eq!(
            printed, expected,
            "the command should start in the configured workspace root"
        );
    }

    #[tokio::test]
    async fn exec_failing_command() {
        let tool = ExecTool::new(None, None, None, workspace_root());
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
        let tool = ExecTool::new(None, None, None, workspace_root());
        // cmd has no sleep; only Git's usr\bin provides one on PATH.
        let command = if cfg!(windows) {
            "powershell -NoProfile -Command \"Start-Sleep -Seconds 10\""
        } else {
            "sleep 10"
        };
        let result = tool
            .execute(serde_json::json!({
                "command": command,
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

    /// Poll (rather than sleep a guessed duration) until `pid` no longer
    /// exists, or fail after a bound — proving the process was actually
    /// killed, not merely abandoned.
    #[cfg(unix)]
    async fn wait_for_pid_to_die(pid: i32) {
        let Ok(pid_u32) = u32::try_from(pid) else {
            return;
        };
        wait::until_true("the killed process to be gone", || {
            !residuum_core::process::is_process_running(pid_u32)
        })
        .await;
    }

    #[cfg(unix)]
    async fn read_pid_file(path: &std::path::Path) -> i32 {
        wait::until("the pid file to be written", || async {
            let contents = tokio::fs::read_to_string(path).await.ok()?;
            contents.trim().parse().ok()
        })
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_timeout_kills_the_whole_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let tool = ExecTool::new(None, None, None, workspace_root());

        // The shell's own pid doubles as the process group id (see
        // `shell_command`), so reading it back is enough to check the
        // whole tree died, not just confirm *something* did.
        let call = tool.execute(serde_json::json!({
            "command": format!("echo $$ > {}; sleep 10", pid_path.display()),
            "timeout_secs": 1
        }));
        let pid = read_pid_file(&pid_path);
        let (result, pid) = tokio::join!(call, pid);
        let result = result.unwrap();

        assert!(result.is_error, "a timeout should be reported as an error");
        assert!(
            result.output.contains("timed out") && result.output.contains("process tree"),
            "message should explain what happened: {}",
            result.output
        );
        wait_for_pid_to_die(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_cancellation_kills_the_process_and_returns_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let tool = ExecTool::new(None, None, None, workspace_root());
        let cancel = CancellationToken::new();

        let command = format!(
            "echo already-ran; echo $$ > {}; sleep 10",
            pid_path.display()
        );
        let call = tool.execute_cancellable(serde_json::json!({ "command": command }), &cancel);

        let cancel_after_pid = async {
            let pid = read_pid_file(&pid_path).await;
            cancel.cancel();
            pid
        };

        let (result, pid) = tokio::join!(call, cancel_after_pid);
        let result = result.unwrap();

        assert!(
            !result.is_error,
            "a user-requested stop is not a tool failure: {}",
            result.output
        );
        assert!(
            result.output.contains("already-ran"),
            "output captured before the stop must be kept, not discarded: {}",
            result.output
        );
        assert!(
            result.output.contains("stopped") && result.output.contains("process tree"),
            "message should explain what happened: {}",
            result.output
        );
        wait_for_pid_to_die(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_future_dropped_before_completion_kills_grandchildren() {
        // Regression test for the drop-guard: dropping the turn future
        // (simulated here with `select!` dropping the losing branch) must
        // still kill the whole process tree, not just the immediate shell
        // that `kill_on_drop` alone would reach.
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("grandchild-pid");
        let tool = ExecTool::new(None, None, None, workspace_root());
        let cancel = CancellationToken::new();

        let command = format!("sleep 10 & echo $! > {}; wait", pid_path.display());

        let grandchild_pid = tokio::select! {
            result = tool.execute_cancellable(serde_json::json!({ "command": command }), &cancel) => {
                panic!(
                    "command should still be running when dropped, got: {:?}",
                    result.map(|r| r.output)
                );
            }
            pid = read_pid_file(&pid_path) => pid,
        };

        wait_for_pid_to_die(grandchild_pid).await;
    }

    #[tokio::test]
    async fn exec_missing_command() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing command should return ToolError");
    }

    #[tokio::test]
    async fn exec_stderr_output() {
        let tool = ExecTool::new(None, None, None, workspace_root());
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
    async fn exec_passes_quotes_through_to_the_shell() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let result = tool
            .execute(serde_json::json!({ "command": "echo \"hello  world\"" }))
            .await
            .unwrap();
        assert!(
            !result.is_error,
            "command should succeed: {}",
            result.output
        );
        // cmd's echo prints the quotes; sh's removes them. Either way the
        // double space inside the quotes must survive.
        assert!(
            result.output.contains("hello  world"),
            "got: {}",
            result.output
        );
    }

    fn captured(head: &[u8], omitted: usize, tail: &[u8]) -> Captured {
        Captured {
            head: head.to_vec(),
            tail: tail.to_vec(),
            omitted,
        }
    }

    #[test]
    fn shape_stream_shows_both_ends_with_a_note_between() {
        let stream = captured(b"abcdef", 4, b"wxyz");
        assert_eq!(
            shape_stream("stdout", &stream, 6, 4),
            "abcdef\n[… 4 bytes of stdout omitted …]\nwxyz"
        );
    }

    #[test]
    fn shape_stream_counts_bytes_cut_from_the_head_budget() {
        let stream = captured(b"abcdef", 0, b"");
        assert_eq!(
            shape_stream("stdout", &stream, 3, 0),
            "abc\n[… 3 bytes of stdout omitted …]\n"
        );
    }

    #[test]
    fn shape_stream_counts_bytes_cut_from_the_tail_budget() {
        let stream = captured(b"", 2, b"wxyz");
        assert_eq!(
            shape_stream("stderr", &stream, 0, 2),
            "[… 4 bytes of stderr omitted …]\nyz"
        );
    }

    #[test]
    fn shape_stream_without_omission_is_the_whole_stream() {
        let stream = captured(b"head", 0, b"tail");
        assert_eq!(shape_stream("stdout", &stream, 100, 100), "headtail");
        assert_eq!(shape_stream("stdout", &Captured::default(), 100, 100), "");
    }

    #[test]
    fn shown_streams_keep_only_the_head_of_stdout_on_success() {
        let stdout = captured(
            &vec![b'h'; MAX_OUTPUT_BYTES],
            5_000,
            &vec![b't'; MAX_OUTPUT_BYTES],
        );
        let (shown, _) = shown_streams(&stdout, &Captured::default(), false);
        assert!(shown.starts_with('h'));
        assert!(
            !shown.contains(&"t".repeat(8)),
            "the tail of a successful run is not shown"
        );
        assert!(shown.contains("bytes of stdout omitted"), "{shown}");
    }

    #[test]
    fn shown_streams_keep_the_tail_of_stdout_when_the_command_fails() {
        let stdout = captured(
            &vec![b'h'; MAX_OUTPUT_BYTES],
            5_000,
            &vec![b't'; MAX_OUTPUT_BYTES],
        );
        let (shown, _) = shown_streams(&stdout, &Captured::default(), true);
        assert!(shown.starts_with('h'));
        assert!(shown.ends_with('t'));
        assert!(shown.contains("bytes of stdout omitted"), "{shown}");
    }

    #[test]
    fn shown_streams_keep_the_tail_of_stderr_and_stay_within_the_cap() {
        let stdout = captured(
            &vec![b'h'; MAX_OUTPUT_BYTES],
            9_000,
            &vec![b't'; MAX_OUTPUT_BYTES],
        );
        let stderr = captured(b"", 1_000, b"final error\n");
        for failed in [false, true] {
            let (stdout_text, stderr_text) = shown_streams(&stdout, &stderr, failed);
            assert!(stderr_text.ends_with("final error\n"), "{stderr_text}");
            assert!(stderr_text.starts_with("[… 1000 bytes of stderr omitted …]"));
            assert!(
                stdout_text.len() + stderr_text.len() + "STDERR:\n".len() <= MAX_OUTPUT_BYTES,
                "shown output must fit the cap: stdout {} stderr {}",
                stdout_text.len(),
                stderr_text.len()
            );
        }
    }

    #[test]
    fn shown_streams_show_small_output_whole() {
        let stdout = captured(b"out\n", 0, b"");
        let stderr = captured(b"", 0, b"err\n");
        assert_eq!(
            shown_streams(&stdout, &stderr, true),
            ("out\n".to_string(), "err\n".to_string())
        );
    }

    /// At least `bytes` bytes of numbered lines, ending in a newline.
    fn numbered_lines(bytes: usize) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut text = String::new();
        let mut n = 0;
        while text.len() < bytes {
            n += 1;
            _ = writeln!(text, "line {n}");
        }
        text.into_bytes()
    }

    /// `stream` as exec's stdout capture keeps it: the head fills first, then
    /// the tail holds the latest bytes and anything between is dropped.
    fn as_stdout_capture(stream: &[u8]) -> Captured {
        let head = prefix(stream, MAX_OUTPUT_BYTES);
        let rest = stream.get(head.len()..).unwrap_or_default();
        let tail = suffix(rest, MAX_OUTPUT_BYTES);
        captured(head, rest.len() - tail.len(), tail)
    }

    fn last_line(stream: &[u8]) -> String {
        let text = String::from_utf8_lossy(stream);
        format!("{}\n", text.trim_end().lines().last().unwrap())
    }

    /// The budget `shown_streams` gives stdout when stderr is empty.
    const STDOUT_BUDGET: usize = MAX_OUTPUT_BYTES - NOTE_RESERVE_BYTES;

    #[test]
    fn a_failing_commands_stdout_keeps_its_end_in_every_size_range() {
        // Shorter than the capture's head (the tail is empty), between one
        // and two capture windows (nothing dropped), and longer than both
        // (the capture dropped its middle).
        for size in [MAX_OUTPUT_BYTES - 20, 60_000, 150_000] {
            let stream = numbered_lines(size);
            let (shown, _) = shown_streams(&as_stdout_capture(&stream), &Captured::default(), true);
            assert!(shown.starts_with("line 1\n"), "{size}: head kept");
            assert!(
                shown.ends_with(&last_line(&stream)),
                "{size}: stdout's last line is shown: …{}",
                shown.get(shown.len().saturating_sub(200)..).unwrap()
            );
            assert!(shown.contains("bytes of stdout omitted"), "{size}");
            assert!(
                shown.len() <= STDOUT_BUDGET + 64,
                "{size}: within the budget: {}",
                shown.len()
            );
            assert!(
                shown.len() >= STDOUT_BUDGET - 64,
                "{size}: the budget is used, not wasted: {}",
                shown.len()
            );
        }
    }

    #[test]
    fn a_failing_commands_stdout_that_fits_is_shown_whole() {
        let stream = numbered_lines(STDOUT_BUDGET / 2);
        let (shown, _) = shown_streams(&as_stdout_capture(&stream), &Captured::default(), true);
        assert_eq!(shown.as_bytes(), stream.as_slice());
    }

    #[test]
    fn a_succeeding_commands_stdout_keeps_only_its_start_in_every_size_range() {
        for size in [MAX_OUTPUT_BYTES - 20, 60_000, 150_000] {
            let stream = numbered_lines(size);
            let (shown, _) =
                shown_streams(&as_stdout_capture(&stream), &Captured::default(), false);
            let head = String::from_utf8_lossy(prefix(&stream, STDOUT_BUDGET));
            let note = format!(
                "[… {} bytes of stdout omitted …]\n",
                stream.len() - STDOUT_BUDGET
            );
            assert!(
                shown.starts_with(head.as_ref()),
                "{size}: the head fills the budget"
            );
            assert!(
                shown.ends_with(&note),
                "{size}: only the head, then the note"
            );
            assert!(
                shown.len() <= head.len() + 1 + note.len(),
                "{size}: nothing but the head and the note: {}",
                shown.len()
            );
        }
    }

    #[test]
    fn execs_largest_output_fits_under_the_history_limit() {
        let stdout = as_stdout_capture(&numbered_lines(500_000));
        let stderr = captured(b"", 90_000, &numbered_lines(STDERR_BUDGET_BYTES));
        for code in [0, 1] {
            let output = CapturedOutput {
                status: exit_status(code),
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            };
            let result = format_output(&output);
            assert!(
                result.output.chars().count() <= residuum_agent::HISTORY_RESULT_MAX_CHARS,
                "exit {code}: {} characters",
                result.output.chars().count()
            );
            let partial = format_captured_output(
                "command timed out after 120 seconds; its process tree was killed",
                &output,
            );
            assert!(
                partial.chars().count() <= residuum_agent::HISTORY_RESULT_MAX_CHARS,
                "timed out: {} characters",
                partial.chars().count()
            );
        }
    }

    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            std::os::unix::process::ExitStatusExt::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            std::os::windows::process::ExitStatusExt::from_raw(u32::try_from(code).unwrap())
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_failing_command_with_mid_sized_stdout_keeps_its_end() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        // About 60 KB of stdout: more than the cap, less than the capture's
        // head and tail together.
        let result = tool
            .execute(serde_json::json!({ "command": "seq 1 11000; exit 1" }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(
            result
                .output
                .starts_with("command exited with code 1\n1\n2\n"),
            "{}",
            result.output
        );
        assert!(result.output.contains("bytes of stdout omitted"));
        assert!(
            result.output.ends_with("\n10999\n11000\n"),
            "stdout's end is kept: …{}",
            result
                .output
                .get(result.output.len().saturating_sub(200)..)
                .unwrap()
        );
        assert!(result.output.len() <= MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn exec_large_output_is_cut_to_the_cap() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let command = if cfg!(windows) {
            "powershell -NoProfile -Command \"'x' * 204800\""
        } else {
            "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'x'"
        };
        let result = tool
            .execute(serde_json::json!({ "command": command }))
            .await
            .unwrap();

        assert!(!result.is_error, "{}", result.output);
        assert!(
            result.output.contains("bytes of stdout omitted"),
            "large output is cut with a note: output len = {}",
            result.output.len()
        );
        assert!(
            result.output.len() <= MAX_OUTPUT_BYTES,
            "{}",
            result.output.len()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_keeps_the_start_of_large_stdout_and_the_end_of_large_stderr() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let result = tool
            .execute(serde_json::json!({
                "command": "seq 1 100000; seq 1 30000 >&2"
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "{}", result.output);
        assert!(result.output.starts_with("1\n2\n3\n"), "stdout head kept");
        assert!(
            !result.output.contains("\n99999\n"),
            "stdout middle dropped"
        );
        assert!(result.output.contains("bytes of stdout omitted"));
        assert!(
            result.output.ends_with("30000\n"),
            "stderr's last line kept"
        );
        assert!(result.output.contains("bytes of stderr omitted"));
        assert!(result.output.len() <= MAX_OUTPUT_BYTES);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_failing_command_keeps_the_end_of_stdout() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let result = tool
            .execute(serde_json::json!({
                "command": "seq 1 100000; echo boom >&2; exit 3"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(
            result
                .output
                .starts_with("command exited with code 3\n1\n2\n"),
            "{}",
            result.output
        );
        assert!(result.output.contains("\n100000\n"), "stdout's end is kept");
        assert!(result.output.contains("bytes of stdout omitted"));
        assert!(
            result.output.ends_with("STDERR:\nboom\n"),
            "{}",
            result.output
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_removes_ansi_colour_codes_from_output() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        let result = tool
            .execute(serde_json::json!({
                "command": "printf '\\033[31mred\\033[0m\\n'; printf '\\033[1mbold\\033[0m\\n' >&2"
            }))
            .await
            .unwrap();

        assert_eq!(result.output, "red\n\nSTDERR:\nbold\n");
    }

    #[cfg(unix)]
    mod agent_keys {
        use super::*;
        use residuum_config::agent_keys::AgentKeys;

        async fn tool_with_key(dir: &std::path::Path) -> (ExecTool, SharedAgentKeys) {
            let keys = AgentKeys::new_shared(dir);
            keys.set("api_key", "sk-test-abcdef123", None, KeyCreator::User)
                .await
                .unwrap();
            (
                ExecTool::new(None, Some(Arc::clone(&keys)), None, workspace_root()),
                keys,
            )
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
            let result = ExecTool::new(None, None, None, workspace_root())
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
        async fn store_output_as_short_value_is_stored_with_a_warning() {
            let dir = tempfile::tempdir().unwrap();
            let (tool, keys) = tool_with_key(dir.path()).await;
            let result = tool
                .execute(serde_json::json!({
                    "command": "echo -n short",
                    "store_output_as": { "name": "minted" }
                }))
                .await
                .unwrap();
            assert!(
                !result.is_error,
                "a short value should still be stored: {}",
                result.output
            );
            assert!(
                result.output.contains("Warning:") && result.output.contains("redact"),
                "should warn that a short value can't be redacted reliably: {}",
                result.output
            );
            let snap = keys.snapshot().await.unwrap();
            assert_eq!(snap.store.value("minted"), Some("short"));
        }

        #[tokio::test]
        async fn store_output_as_checkpoints_the_config_repo_before_storing() {
            let dir = tempfile::tempdir().unwrap();
            let keys = AgentKeys::new_shared(dir.path());
            let checkpoints_dir = dir.path().join("checkpoints");
            let engine = std::sync::Arc::new(
                residuum_checkpoints::CheckpointEngine::new(
                    "test-agent",
                    dir.path().join("workspace"),
                    &residuum_config::paths::TeamPaths::new(dir.path().to_path_buf().join("team")),
                    dir.path().join("agent-config"),
                    dir.path(),
                    &checkpoints_dir,
                    None,
                )
                .unwrap(),
            );
            // A first write with nothing preexisting has nothing to
            // checkpoint (the pre-write snapshot would be empty either way),
            // so store a first key without an engine attached, then attach
            // one and store a second key -- that second write's pre-write
            // checkpoint has real prior state (the first key) to capture.
            keys.set("first", "value-one-abc123", None, KeyCreator::Agent)
                .await
                .unwrap();
            let tool = ExecTool::new(
                None,
                Some(keys),
                Some(std::sync::Arc::clone(&engine)),
                workspace_root(),
            );

            let result = tool
                .execute(serde_json::json!({
                    "command": "echo minted-token-checkpoint",
                    "store_output_as": { "name": "minted" }
                }))
                .await
                .unwrap();
            assert!(!result.is_error, "store should succeed: {}", result.output);

            let page = engine
                .list_checkpoints(residuum_checkpoints::RepoKind::Hub, None, None, None, None)
                .await
                .unwrap();
            assert_eq!(
                page.items.len(),
                1,
                "minting a key through exec should checkpoint the config repo's prior state"
            );
            assert_eq!(
                page.items.first().unwrap().trigger,
                residuum_checkpoints::CheckpointTrigger::PreConfigWrite
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
        async fn store_over_user_key_replaces_it() {
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
            assert!(
                !result.is_error,
                "the agent may overwrite a key the user created: {}",
                result.output
            );
            assert!(marker.exists(), "command should run");
            assert_eq!(
                keys.snapshot().await.unwrap().store.value("api_key"),
                Some("replacement-value"),
                "user key should be replaced"
            );
            assert_eq!(
                keys.snapshot().await.unwrap().store.creator("api_key"),
                Some(KeyCreator::Agent)
            );
        }

        #[tokio::test]
        async fn store_over_user_key_publishes_a_notice() {
            use residuum_bus::{NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics};

            let dir = tempfile::tempdir().unwrap();
            let keys = AgentKeys::new_shared(dir.path());
            keys.set("api_key", "sk-test-abcdef123", None, KeyCreator::User)
                .await
                .unwrap();
            let bus_handle = residuum_bus::spawn_broker();
            let mut notices: residuum_bus::Subscriber<NoticeEvent> = bus_handle
                .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
                .await
                .unwrap();
            let tool = ExecTool::new(None, Some(keys), None, workspace_root())
                .with_publisher(bus_handle.publisher());

            let result = tool
                .execute(serde_json::json!({
                    "command": "echo replacement-value",
                    "store_output_as": { "name": "api_key" }
                }))
                .await
                .unwrap();
            assert!(
                !result.is_error,
                "overwrite should succeed: {}",
                result.output
            );

            let notice = notices
                .recv()
                .await
                .unwrap()
                .expect("a notice was published");
            assert!(
                notice.message.contains("api_key") && notice.message.contains("created"),
                "notice should name the key and that the user created it: {}",
                notice.message
            );
        }
    }

    #[test]
    fn exec_tool_definition() {
        let tool = ExecTool::new(None, None, None, workspace_root());
        assert_eq!(tool.name(), "exec", "tool name should match");
    }
}

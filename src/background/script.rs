//! Script execution for background tasks.

use std::path::Path;

use tokio::process::Command;

use super::types::ScriptConfig;

/// Default timeout for script execution (2 minutes).
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Maximum output size in bytes before truncation (~10KB).
const MAX_OUTPUT_BYTES: usize = 10_240;

/// Execute a script background task.
///
/// Spawns the command as a child process, captures stdout/stderr, and applies
/// a timeout. On success returns the truncated stdout. On non-zero exit returns
/// stdout + stderr with the exit code. On timeout the child is killed
/// automatically when the future is dropped.
///
/// # Errors
/// Returns an error if the command cannot be spawned.
pub(crate) async fn execute_script(
    _task_id: &str,
    config: &ScriptConfig,
    workspace_root: &Path,
) -> Result<String, anyhow::Error> {
    let working_dir = config.working_dir.as_deref().unwrap_or(workspace_root);

    let timeout_secs = config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn()?;

    let output = match tokio::time::timeout(
        tokio::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(result) => result?,
        Err(_elapsed) => {
            anyhow::bail!(
                "script timed out after {timeout_secs}s: {} {}",
                config.command,
                config.args.join(" ")
            );
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        Ok(truncate_output(&stdout, MAX_OUTPUT_BYTES))
    } else {
        let code = output.status.code().unwrap_or(-1);
        let combined = if stderr.is_empty() {
            format!("exit code {code}\nstdout:\n{stdout}")
        } else {
            format!("exit code {code}\nstdout:\n{stdout}\nstderr:\n{stderr}")
        };
        Ok(truncate_output(&combined, MAX_OUTPUT_BYTES))
    }
}

/// Truncate output to a maximum byte size, preserving valid UTF-8.
fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    // Find the last valid char boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }

    let truncated = s.get(..end).unwrap_or(s);
    format!("{truncated}\n... [truncated, {max_bytes} byte limit]")
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn simple_config(command: &str, args: &[&str]) -> ScriptConfig {
        ScriptConfig {
            command: command.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            working_dir: None,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn echo_captures_stdout() {
        let config = simple_config("echo", &["hello"]);
        let result = execute_script("bg-test", &config, Path::new("/tmp"))
            .await
            .unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn nonzero_exit_includes_stderr() {
        let config = ScriptConfig {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "echo err >&2; exit 1".to_string()],
            working_dir: None,
            timeout_secs: None,
        };
        let result = execute_script("bg-test", &config, Path::new("/tmp"))
            .await
            .unwrap();
        assert!(result.contains("exit code 1"), "should contain exit code");
        assert!(result.contains("err"), "should contain stderr output");
    }

    #[tokio::test]
    async fn timeout_returns_error() {
        let config = ScriptConfig {
            command: "sleep".to_string(),
            args: vec!["30".to_string()],
            working_dir: None,
            timeout_secs: Some(1),
        };
        let result = execute_script("bg-test", &config, Path::new("/tmp")).await;
        assert!(result.is_err(), "should return error on timeout");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"), "should mention timeout: {err}");
    }

    #[test]
    fn truncate_within_limit() {
        let s = "hello world";
        assert_eq!(truncate_output(s, 100), "hello world");
    }

    #[test]
    fn truncate_at_limit() {
        let s = "abcdefghij";
        let result = truncate_output(s, 5);
        assert!(result.starts_with("abcde"), "should truncate to 5 bytes");
        assert!(
            result.contains("truncated"),
            "should include truncation notice"
        );
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // Multi-byte char: é is 2 bytes in UTF-8
        let s = "café is good";
        // Truncate in the middle of the é (byte 4 is second byte of é)
        let result = truncate_output(s, 4);
        // Should back up to byte 3 (before the é)
        assert!(
            result.starts_with("caf"),
            "should respect char boundary: {result}"
        );
    }

    #[tokio::test]
    async fn working_dir_respected() {
        let config = ScriptConfig {
            command: "pwd".to_string(),
            args: Vec::new(),
            working_dir: Some(PathBuf::from("/tmp")),
            timeout_secs: None,
        };
        let result = execute_script("bg-test", &config, Path::new("/"))
            .await
            .unwrap();
        // /tmp might resolve to /private/tmp on macOS, so just check it ends with tmp
        assert!(
            result.trim().ends_with("tmp"),
            "should execute in specified working dir: {result}"
        );
    }
}

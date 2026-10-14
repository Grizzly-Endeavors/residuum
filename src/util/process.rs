//! Cross-platform process tree management and command invocation helpers.
//!
//! Provides utilities to spawn processes in their own process groups, kill process trees
//! across Unix and Windows, safely handle Windows `.cmd` batch file invocations with argv,
//! track process liveness, and run a shell command under a timeout and a stop.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use nix::sys::signal::{Signal, kill, killpg};
#[cfg(unix)]
use nix::unistd::Pid;

/// Inspect whether an operating system process with the given PID is currently running.
#[must_use]
pub fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid_i32) = i32::try_from(pid) else {
            return false;
        };
        match kill(Pid::from_raw(pid_i32), None) {
            Ok(()) => {
                #[cfg(target_os = "linux")]
                {
                    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                        && let Some(rest) = stat.rfind(')').and_then(|idx| stat.get(idx + 2..))
                        && rest.starts_with('Z')
                    {
                        return false;
                    }
                }
                true
            }
            Err(nix::errno::Errno::ESRCH) => false,
            Err(_) => true,
        }
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match output {
            Ok(out) => {
                let s = String::from_utf8_lossy(&out.stdout);
                s.contains(&pid.to_string())
            }
            Err(_) => false,
        }
    }
}

/// Asynchronously kill every process in the tree rooted at `pid`.
///
/// On Unix, sends `SIGKILL` to the process group (`pid == pgid`) created by [`configure_process_group`].
/// On Windows, runs `taskkill /T /F /PID <pid>`.
///
/// A no-op if `pid` is `None`.
pub async fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };

    #[cfg(unix)]
    {
        let Ok(pid_i32) = i32::try_from(pid) else {
            tracing::warn!(pid, "pid doesn't fit a pid_t, skipping process-group kill");
            return;
        };
        let result = crate::util::spawn_blocking_in_span(move || {
            killpg(Pid::from_raw(pid_i32), Signal::SIGKILL)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, pid, "failed to kill command's process group");
            }
            Err(e) => {
                tracing::warn!(error = %e, pid, "process-group kill task panicked");
            }
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = tokio::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .kill_on_drop(true)
            .output()
            .await
        {
            tracing::warn!(error = %e, pid, "failed to kill command's process tree");
        }
    }
}

/// Synchronously kill every process in the tree rooted at `pid`.
///
/// Suitable for `Drop` implementations where asynchronous execution is impossible.
///
/// A no-op if `pid` is `None`.
pub fn kill_process_tree_sync(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };

    #[cfg(unix)]
    {
        let Ok(pid_i32) = i32::try_from(pid) else {
            tracing::warn!(
                pid,
                "pid doesn't fit a pid_t, skipping process-group kill on drop"
            );
            return;
        };
        if let Err(e) = killpg(Pid::from_raw(pid_i32), Signal::SIGKILL) {
            tracing::warn!(error = %e, pid, "failed to kill process group on drop");
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output()
        {
            tracing::warn!(error = %e, pid, "failed to kill process tree on drop");
        }
    }
}

/// RAII guard that kills a process tree if dropped while still armed.
pub struct ProcessTreeGuard {
    pid: Option<u32>,
}

impl ProcessTreeGuard {
    /// Create a new guard for `pid`.
    #[must_use]
    pub const fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    /// Disarm the guard when the child process has completed or exited normally.
    pub fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        kill_process_tree_sync(self.pid);
    }
}

/// Configure process isolation so spawned commands run in their own process group.
pub fn configure_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        let _ = cmd;
    }
}

/// Pure helper to resolve the effective executable and argument list for a command.
///
/// On Windows, Rust std (>= 1.77) natively escapes arguments and spawns `cmd.exe`
/// under the hood when invoked on `.cmd` or `.bat` paths without needing a hand-rolled
/// `cmd.exe /d /s /c` layer that could break paths with spaces (e.g. `C:\Program Files\nodejs\npm.cmd`).
#[must_use]
pub fn build_platform_command_spec(
    program: &Path,
    args: &[&str],
    _is_windows: bool,
) -> (PathBuf, Vec<String>) {
    (
        program.to_path_buf(),
        args.iter().copied().map(String::from).collect(),
    )
}

/// Create a [`tokio::process::Command`] with argv and process group configured.
///
/// Spawns the specified program with argv, isolated in its own process group on Unix.
#[must_use]
pub fn create_argv_command(program: &Path, args: &[&str]) -> tokio::process::Command {
    let (target_bin, target_args) = build_platform_command_spec(program, args, cfg!(windows));
    let mut cmd = tokio::process::Command::new(target_bin);
    cmd.args(&target_args);
    configure_process_group(&mut cmd);
    cmd
}

/// Build the platform shell invocation for `command`: `sh -c` on Unix,
/// `cmd /S /C` on Windows.
#[must_use]
pub(crate) fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        // Puts the shell (pid == pgid) in its own process group instead of
        // inheriting residuum's, so killing the group on a timeout or a
        // stop (see `kill_process_tree`) only ever reaches this command's
        // own tree, never residuum's process group.
        c.process_group(0);
        c
    }
    // Passed raw because cmd.exe doesn't parse the backslash-escaped quotes
    // that normal argument quoting produces. `/S` strips only the outer pair
    // of quotes, so the command runs exactly as written, including one that
    // starts with a quoted path.
    #[cfg(windows)]
    {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/S", "/C"]).raw_arg(format!("\"{command}\""));
        c
    }
}

/// How a spawned command's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    /// The command exited on its own before the timeout or a stop.
    Finished,
    /// The timeout elapsed; the process tree was killed.
    TimedOut,
    /// The caller's cancellation token fired while the command was running;
    /// the process tree was killed.
    Cancelled,
}

/// How the race in [`run_with_timeout_and_cancel`] resolved, before the
/// process tree has necessarily been killed or reaped yet.
enum Wait {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

/// Spawn `cmd`, race it against `timeout` and `cancel`, and return whatever
/// output it produced along with how the run ended.
///
/// Each pipe keeps at most `capture_limit` bytes; anything past that is
/// still read, so the command never blocks on a full pipe, and discarded.
/// A caller that needs to know whether output was cut asks for one byte
/// more than it will show and checks the length.
///
/// On a timeout or a stop, [`kill_process_tree`] kills the whole process
/// tree rather than only the immediate shell, and whatever stdout/stderr the
/// command had already produced is still returned — the pipes reach EOF
/// once every process holding their write end has exited, which the kill
/// guarantees, so draining them afterward always yields whatever was
/// captured before the command ended, partial or not.
///
/// # Errors
/// Returns an error when the command can't be spawned, its exit status
/// can't be read, or a pipe read fails.
pub(crate) async fn run_with_timeout_and_cancel(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    cancel: &CancellationToken,
    capture_limit: usize,
) -> io::Result<(RunOutcome, Output)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let pid = child.id();
    // Guards the window between spawn and the point below where the
    // process tree has already been killed or the child already reaped —
    // covers the case where this future itself is dropped (not cancelled
    // through the `select!` below), e.g. the caller's turn future being
    // dropped while this is still suspended awaiting the child.
    let mut tree_guard = ProcessTreeGuard::new(pid);
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("spawned command's stdout was not piped"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("spawned command's stderr was not piped"))?;

    // Drained on their own tasks, running concurrently with the wait below
    // for as long as it takes — a command that writes more than the OS
    // pipe buffer holds would otherwise block on that write forever, since
    // nothing would be reading until after `child.wait()` resolves.
    let stdout_task = spawn_pipe_reader(stdout_pipe, capture_limit);
    let stderr_task = spawn_pipe_reader(stderr_pipe, capture_limit);

    let wait = tokio::select! {
        biased;
        () = cancel.cancelled() => Wait::Cancelled,
        () = tokio::time::sleep(timeout) => Wait::TimedOut,
        status = child.wait() => Wait::Exited(status?),
    };

    let (outcome, status) = match wait {
        Wait::Exited(status) => {
            // The child already exited on its own; nothing left to kill.
            tree_guard.disarm();
            (RunOutcome::Finished, status)
        }
        Wait::TimedOut => {
            kill_process_tree(pid).await;
            tree_guard.disarm();
            (RunOutcome::TimedOut, child.wait().await?)
        }
        Wait::Cancelled => {
            kill_process_tree(pid).await;
            tree_guard.disarm();
            (RunOutcome::Cancelled, child.wait().await?)
        }
    };

    Ok((
        outcome,
        Output {
            status,
            stdout: join_pipe_reader(stdout_task).await?,
            stderr: join_pipe_reader(stderr_task).await?,
        },
    ))
}

/// Spawn a task that reads a child's pipe to EOF, keeping the first `keep`
/// bytes and discarding the rest.
fn spawn_pipe_reader<R>(mut pipe: R, keep: usize) -> tokio::task::JoinHandle<io::Result<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    crate::util::spawn_in_span(async move {
        let mut buf = Vec::new();
        let mut chunk = vec![0_u8; 8 * 1024];
        loop {
            let read = pipe.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            let room = keep.saturating_sub(buf.len());
            if let Some(kept) = chunk.get(..read.min(room)) {
                buf.extend_from_slice(kept);
            }
        }
        Ok(buf)
    })
}

/// Await a [`spawn_pipe_reader`] task, surfacing a task panic as an I/O
/// error the same way a read failure would be.
async fn join_pipe_reader(
    task: tokio::task::JoinHandle<io::Result<Vec<u8>>>,
) -> io::Result<Vec<u8>> {
    task.await
        .map_err(|e| io::Error::other(format!("pipe reader task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_command_spec_unix_preserves_program() {
        let prog = Path::new("/usr/local/bin/atk");
        let (exe, args) =
            build_platform_command_spec(prog, &["provision", "--env", "residuum"], false);
        assert_eq!(exe, prog);
        assert_eq!(args, vec!["provision", "--env", "residuum"]);
    }

    #[test]
    fn platform_command_spec_windows_exe_preserves_program() {
        let prog = Path::new(r"C:\Program Files\nodejs\node.exe");
        let (exe, args) = build_platform_command_spec(prog, &["--version"], true);
        assert_eq!(exe, prog);
        assert_eq!(args, vec!["--version"]);
    }

    #[test]
    fn platform_command_spec_windows_cmd_with_spaces_preserves_program() {
        let prog = Path::new(r"C:\Program Files\nodejs\atk.cmd");
        let (exe, args) =
            build_platform_command_spec(prog, &["auth", "list", "m365", "path with spaces"], true);
        assert_eq!(exe, prog);
        assert_eq!(args, vec!["auth", "list", "m365", "path with spaces"]);
    }

    #[test]
    fn platform_command_spec_windows_bat_preserves_program() {
        let prog = Path::new(r"C:\Program Files\nodejs\npm.bat");
        let (exe, args) = build_platform_command_spec(prog, &["--version"], true);
        assert_eq!(exe, prog);
        assert_eq!(args, vec!["--version"]);
    }

    #[test]
    fn process_tree_guard_disarm_prevents_action() {
        let mut guard = ProcessTreeGuard::new(Some(99999));
        guard.disarm();
        assert_eq!(guard.pid, None);
    }

    #[cfg(unix)]
    #[test]
    fn is_process_running_detects_current_process() {
        let current_pid = std::process::id();
        assert!(is_process_running(current_pid));
    }
}

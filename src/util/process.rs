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
    /// The command exited on its own, and everything holding its output
    /// pipes let go of them, before the timeout or a stop.
    Finished,
    /// The timeout elapsed, either while the command ran or while a process
    /// it left behind still held its output open; the process tree was
    /// killed.
    TimedOut,
    /// The caller's cancellation token fired, either while the command ran
    /// or while a process it left behind still held its output open; the
    /// process tree was killed.
    Cancelled,
}

/// How much of a pipe [`run_with_timeout_and_cancel`] keeps. Whatever
/// isn't kept is still read, so the command never blocks on a full pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    /// Everything.
    All,
    /// The first `n` bytes. Ask for one byte more than will be shown to
    /// learn whether output was cut.
    Head(usize),
    /// The last `n` bytes, where a failing command's final error usually is.
    Tail(usize),
}

/// How long the pipes are still read after a killed command's process
/// tree is gone. A process that escaped the kill can hold the pipes open
/// for as long as it lives: one that left the process group on Unix, or,
/// on Windows, a descendant `taskkill /T` can no longer find once the shell
/// that started it has exited. Past this, whatever was read is returned.
const KILLED_PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

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
/// The timeout and `cancel` stay in force until both output pipes close,
/// not only until the shell exits: a command that starts a background
/// process which keeps its output open (`some-daemon & echo started`) is
/// stopped at the timeout like any other. On a timeout or a stop,
/// [`kill_process_tree`] kills the whole process tree rather than only the
/// immediate shell, and whatever stdout/stderr the command had produced is
/// still returned (see [`KILLED_PIPE_DRAIN_GRACE`] for a process the kill
/// can't reach).
///
/// # Errors
/// Returns an error when the command can't be spawned, its exit status
/// can't be read, or a pipe read fails.
pub(crate) async fn run_with_timeout_and_cancel(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    cancel: &CancellationToken,
    (stdout_capture, stderr_capture): (Capture, Capture),
) -> io::Result<(RunOutcome, Output)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let deadline = tokio::time::Instant::now() + timeout;
    let mut child = cmd.spawn()?;
    let pid = child.id();
    // Kills the process tree if this future is dropped (not cancelled
    // through `cancel`) before the run has fully ended, the pipe drain
    // included — e.g. the caller's turn future being dropped mid-command.
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
    let stop_readers = CancellationToken::new();
    let stdout_task = spawn_pipe_reader(stdout_pipe, stdout_capture, stop_readers.clone());
    let stderr_task = spawn_pipe_reader(stderr_pipe, stderr_capture, stop_readers.clone());
    let mut readers = std::pin::pin!(async move {
        let stdout = join_pipe_reader(stdout_task).await;
        let stderr = join_pipe_reader(stderr_task).await;
        (stdout, stderr)
    });

    let wait = tokio::select! {
        biased;
        () = cancel.cancelled() => Wait::Cancelled,
        () = tokio::time::sleep_until(deadline) => Wait::TimedOut,
        status = child.wait() => Wait::Exited(status?),
    };

    let mut drained = None;
    let (outcome, status) = match wait {
        Wait::Exited(status) => {
            // The shell is gone, but anything it started in the background
            // may still hold the pipes; the deadline and the stop still apply.
            let stopped = tokio::select! {
                biased;
                () = cancel.cancelled() => Some(RunOutcome::Cancelled),
                () = tokio::time::sleep_until(deadline) => Some(RunOutcome::TimedOut),
                output = &mut readers => {
                    drained = Some(output);
                    None
                }
            };
            match stopped {
                None => (RunOutcome::Finished, status),
                // On Unix the process group outlives its leader while any
                // member lives, so this still reaches what the shell left.
                Some(outcome) => {
                    kill_process_tree(pid).await;
                    (outcome, status)
                }
            }
        }
        Wait::TimedOut => {
            kill_process_tree(pid).await;
            (RunOutcome::TimedOut, child.wait().await?)
        }
        Wait::Cancelled => {
            kill_process_tree(pid).await;
            (RunOutcome::Cancelled, child.wait().await?)
        }
    };

    let (stdout, stderr) = match drained {
        Some(output) => output,
        None => {
            tokio::select! {
                biased;
                output = &mut readers => output,
                () = tokio::time::sleep(KILLED_PIPE_DRAIN_GRACE) => {
                    stop_readers.cancel();
                    (&mut readers).await
                }
            }
        }
    };
    tree_guard.disarm();

    Ok((
        outcome,
        Output {
            status,
            stdout: stdout?,
            stderr: stderr?,
        },
    ))
}

/// One pipe's captured bytes, kept per its [`Capture`].
struct CaptureBuffer {
    capture: Capture,
    bytes: Vec<u8>,
}

impl CaptureBuffer {
    fn new(capture: Capture) -> Self {
        Self {
            capture,
            bytes: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        match self.capture {
            Capture::All => self.bytes.extend_from_slice(chunk),
            Capture::Head(keep) => {
                let room = keep.saturating_sub(self.bytes.len());
                if let Some(kept) = chunk.get(..chunk.len().min(room)) {
                    self.bytes.extend_from_slice(kept);
                }
            }
            Capture::Tail(keep) => {
                self.bytes.extend_from_slice(chunk);
                // Trimmed in batches so each byte is moved O(1) times.
                if self.bytes.len() > keep.saturating_mul(2) {
                    let excess = self.bytes.len() - keep;
                    self.bytes.drain(..excess);
                }
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if let Capture::Tail(keep) = self.capture
            && self.bytes.len() > keep
        {
            let excess = self.bytes.len() - keep;
            self.bytes.drain(..excess);
        }
        self.bytes
    }
}

/// Spawn a task that reads a child's pipe to EOF, keeping what `capture`
/// asks for. Once `stop` fires it still takes whatever is ready to read,
/// then returns instead of waiting for more.
fn spawn_pipe_reader<R>(
    mut pipe: R,
    capture: Capture,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<io::Result<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    crate::util::spawn_in_span(async move {
        let mut buffer = CaptureBuffer::new(capture);
        let mut chunk = vec![0_u8; 8 * 1024];
        loop {
            let read = tokio::select! {
                biased;
                read = pipe.read(&mut chunk) => read?,
                () = stop.cancelled() => break,
            };
            if read == 0 {
                break;
            }
            if let Some(bytes) = chunk.get(..read) {
                buffer.push(bytes);
            }
        }
        Ok(buffer.finish())
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

    // ── capture ──────────────────────────────────────────────────────────

    fn captured(capture: Capture, chunks: &[&[u8]]) -> Vec<u8> {
        let mut buffer = CaptureBuffer::new(capture);
        for chunk in chunks {
            buffer.push(chunk);
        }
        buffer.finish()
    }

    #[test]
    fn capture_all_keeps_everything() {
        assert_eq!(captured(Capture::All, &[b"abc", b"def"]), b"abcdef");
    }

    #[test]
    fn capture_head_keeps_the_first_bytes() {
        assert_eq!(
            captured(Capture::Head(4), &[b"abc", b"def", b"ghi"]),
            b"abcd"
        );
    }

    #[test]
    fn capture_tail_keeps_the_last_bytes() {
        let noise = vec![b'x'; 10_000];
        assert_eq!(
            captured(Capture::Tail(5), &[&noise, b"final", b" error"]),
            b"error"
        );
        assert_eq!(captured(Capture::Tail(64), &[b"short"]), b"short");
    }

    // ── a background process left holding the output ─────────────────────

    /// Wait until the shell whose pid `pid_file` holds has been reaped, which
    /// only `child.wait()` inside `run_with_timeout_and_cancel` does — so the
    /// run has seen the shell exit and is now waiting on the pipes alone.
    #[cfg(target_os = "linux")]
    async fn shell_reaped(pid_file: &std::path::Path) {
        let pid: u32 = crate::testing::wait::until("the shell's pid", || async {
            tokio::fs::read_to_string(pid_file)
                .await
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .await;
        crate::testing::wait::until_true("the shell to be reaped", || {
            !std::path::Path::new(&format!("/proc/{pid}")).exists()
        })
        .await;
    }

    #[cfg(target_os = "linux")]
    async fn background_pid_dies(pid_file: &std::path::Path) {
        let pid: u32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        crate::testing::wait::until_true("the background process to be killed", || {
            !is_process_running(pid)
        })
        .await;
    }

    #[cfg(target_os = "linux")]
    fn leaves_a_background_child(dir: &std::path::Path) -> tokio::process::Command {
        shell_command(&format!(
            "echo $$ > '{shell}'; sleep 600 & echo $! > '{bg}'; echo hi",
            shell = dir.join("shell.pid").display(),
            bg = dir.join("bg.pid").display(),
        ))
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_stop_ends_a_run_whose_background_child_holds_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let run = run_with_timeout_and_cancel(
            leaves_a_background_child(dir.path()),
            Duration::from_secs(3600),
            &cancel,
            (Capture::All, Capture::All),
        );
        let stop = async {
            shell_reaped(&dir.path().join("shell.pid")).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(
            crate::testing::wait::guarded("the stopped run to return", run),
            stop
        );
        let (outcome, output) = result.unwrap();
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hi");
        background_pid_dies(&dir.path().join("bg.pid")).await;
    }

    // Once the shell has been reaped, the clock is paused: with only the
    // pipes left to wait on, tokio jumps straight to the run's deadline.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_timeout_ends_a_run_whose_background_child_holds_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let mut run = std::pin::pin!(run_with_timeout_and_cancel(
            leaves_a_background_child(dir.path()),
            Duration::from_secs(600),
            &cancel,
            (Capture::All, Capture::All),
        ));
        let shell_pid = dir.path().join("shell.pid");
        tokio::select! {
            biased;
            () = shell_reaped(&shell_pid) => {}
            result = &mut run => panic!("the run ended before its shell was reaped: {result:?}"),
        }
        tokio::time::pause();
        let (outcome, output) = crate::testing::clock::within(Duration::from_secs(3600), &mut run)
            .await
            .expect("the run ends at its timeout")
            .unwrap();
        assert_eq!(outcome, RunOutcome::TimedOut);
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hi");
        background_pid_dies(&dir.path().join("bg.pid")).await;
    }
}

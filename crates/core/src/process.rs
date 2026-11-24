use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::util::ansi::AnsiStripper;

#[cfg(unix)]
use nix::sys::signal::{Signal, kill, killpg};
#[cfg(unix)]
use nix::unistd::Pid;

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

pub struct ProcessTreeGuard {
    pid: Option<u32>,
}

impl ProcessTreeGuard {
    #[must_use]
    pub const fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    pub fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        kill_process_tree_sync(self.pid);
    }
}

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

#[must_use]
pub fn create_argv_command(program: &Path, args: &[&str]) -> tokio::process::Command {
    let (target_bin, target_args) = build_platform_command_spec(program, args, cfg!(windows));
    let mut cmd = tokio::process::Command::new(target_bin);
    cmd.args(&target_args);
    configure_process_group(&mut cmd);
    cmd
}

#[must_use]
pub fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Finished,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    All,
    Head(usize),
    Tail(usize),
    HeadAndTail { head: usize, tail: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captured {
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub omitted: usize,
}

impl Captured {
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.head.len() + self.omitted + self.tail.len()
    }
}

#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: Captured,
    pub stderr: Captured,
}

const KILLED_PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

enum Wait {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

pub async fn run_with_timeout_and_cancel(
    mut cmd: tokio::process::Command,
    timeout: Duration,
    cancel: &CancellationToken,
    (stdout_capture, stderr_capture): (Capture, Capture),
) -> io::Result<(RunOutcome, CapturedOutput)> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let deadline = tokio::time::Instant::now() + timeout;
    let mut child = cmd.spawn()?;
    let pid = child.id();
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
        CapturedOutput {
            status,
            stdout: stdout?,
            stderr: stderr?,
        },
    ))
}

struct CaptureBuffer {
    head_cap: usize,
    tail_cap: usize,
    stripper: AnsiStripper,
    plain: Vec<u8>,
    captured: Captured,
}

impl CaptureBuffer {
    fn new(capture: Capture) -> Self {
        let (head_cap, tail_cap) = match capture {
            Capture::All => (usize::MAX, 0),
            Capture::Head(n) => (n, 0),
            Capture::Tail(n) => (0, n),
            Capture::HeadAndTail { head, tail } => (head, tail),
        };
        Self {
            head_cap,
            tail_cap,
            stripper: AnsiStripper::default(),
            plain: Vec::new(),
            captured: Captured::default(),
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.plain.clear();
        self.stripper.strip_into(chunk, &mut self.plain);
        let room = self.head_cap.saturating_sub(self.captured.head.len());
        let (to_head, to_tail) = self.plain.split_at(room.min(self.plain.len()));
        self.captured.head.extend_from_slice(to_head);
        self.captured.tail.extend_from_slice(to_tail);
        if self.captured.tail.len() > self.tail_cap.saturating_mul(2) {
            self.trim_tail();
        }
    }

    fn trim_tail(&mut self) {
        let excess = self.captured.tail.len().saturating_sub(self.tail_cap);
        self.captured.tail.drain(..excess);
        self.captured.omitted += excess;
    }

    fn finish(mut self) -> Captured {
        if self.captured.tail.len() > self.tail_cap {
            self.trim_tail();
        }
        self.captured
    }
}

fn spawn_pipe_reader<R>(
    mut pipe: R,
    capture: Capture,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<io::Result<Captured>>
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

async fn join_pipe_reader(
    task: tokio::task::JoinHandle<io::Result<Captured>>,
) -> io::Result<Captured> {
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

    fn captured(capture: Capture, chunks: &[&[u8]]) -> Captured {
        let mut buffer = CaptureBuffer::new(capture);
        for chunk in chunks {
            buffer.push(chunk);
        }
        buffer.finish()
    }

    #[test]
    fn capture_all_keeps_everything() {
        assert_eq!(
            captured(Capture::All, &[b"abc", b"def"]),
            Captured {
                head: b"abcdef".to_vec(),
                tail: Vec::new(),
                omitted: 0,
            }
        );
    }

    #[test]
    fn capture_head_keeps_the_first_bytes() {
        assert_eq!(
            captured(Capture::Head(4), &[b"abc", b"def", b"ghi"]),
            Captured {
                head: b"abcd".to_vec(),
                tail: Vec::new(),
                omitted: 5,
            }
        );
    }

    #[test]
    fn capture_tail_keeps_the_last_bytes() {
        let noise = vec![b'x'; 10_000];
        assert_eq!(
            captured(Capture::Tail(5), &[&noise, b"final", b" error"]),
            Captured {
                head: Vec::new(),
                tail: b"error".to_vec(),
                omitted: 10_006,
            }
        );
        assert_eq!(
            captured(Capture::Tail(64), &[b"short"]),
            Captured {
                head: Vec::new(),
                tail: b"short".to_vec(),
                omitted: 0,
            }
        );
    }

    #[test]
    fn capture_head_and_tail_keeps_both_ends() {
        assert_eq!(
            captured(
                Capture::HeadAndTail { head: 3, tail: 4 },
                &[b"abc", b"defgh", b"ijkl"]
            ),
            Captured {
                head: b"abc".to_vec(),
                tail: b"ijkl".to_vec(),
                omitted: 5,
            }
        );
    }

    #[test]
    fn capture_head_and_tail_keeps_short_output_once() {
        assert_eq!(
            captured(Capture::HeadAndTail { head: 3, tail: 4 }, &[b"abcde"]),
            Captured {
                head: b"abc".to_vec(),
                tail: b"de".to_vec(),
                omitted: 0,
            }
        );
    }

    #[test]
    fn capture_strips_escape_sequences_before_counting() {
        assert_eq!(
            captured(Capture::Head(4), &[b"\x1b[3", b"1mab\x1b[0m", b"cdef"]),
            Captured {
                head: b"abcd".to_vec(),
                tail: Vec::new(),
                omitted: 2,
            }
        );
    }

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
        assert_eq!(String::from_utf8_lossy(&output.stdout.head).trim(), "hi");
        background_pid_dies(&dir.path().join("bg.pid")).await;
    }

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
        assert_eq!(String::from_utf8_lossy(&output.stdout.head).trim(), "hi");
        background_pid_dies(&dir.path().join("bg.pid")).await;
    }
}

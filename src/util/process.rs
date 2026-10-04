//! Cross-platform process tree management and command invocation helpers.
//!
//! Provides utilities to spawn processes in their own process groups, kill process trees
//! across Unix and Windows, safely handle Windows `.cmd` batch file invocations with argv,
//! and track process liveness.

use std::path::{Path, PathBuf};

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

/// Pure helper to resolve the effective executable and argument list for a command,
/// taking into account Windows `.cmd` or `.bat` shims.
#[must_use]
pub fn build_platform_command_spec(
    program: &Path,
    args: &[&str],
    is_windows: bool,
) -> (PathBuf, Vec<String>) {
    if is_windows {
        let is_batch = program
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
        if is_batch {
            let mut full_args = Vec::with_capacity(args.len() + 4);
            full_args.push("/d".to_string());
            full_args.push("/s".to_string());
            full_args.push("/c".to_string());
            full_args.push(program.to_string_lossy().to_string());
            full_args.extend(args.iter().copied().map(String::from));
            return (PathBuf::from("cmd.exe"), full_args);
        }
    }
    (
        program.to_path_buf(),
        args.iter().copied().map(String::from).collect(),
    )
}

/// Create a [`tokio::process::Command`] with argv and process group configured.
///
/// On Windows, if `program` points to a `.cmd` or `.bat` script, it is invoked via
/// `cmd.exe /d /s /c <script> <args...>` without shell string interpolation.
#[must_use]
pub fn create_argv_command(program: &Path, args: &[&str]) -> tokio::process::Command {
    let (target_bin, target_args) = build_platform_command_spec(program, args, cfg!(windows));
    let mut cmd = tokio::process::Command::new(target_bin);
    cmd.args(&target_args);
    configure_process_group(&mut cmd);
    cmd
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
    fn platform_command_spec_windows_cmd_routes_via_cmd_exe() {
        let prog = Path::new(r"C:\tools\atk.cmd");
        let (exe, args) = build_platform_command_spec(prog, &["auth", "list", "m365"], true);
        assert_eq!(exe, Path::new("cmd.exe"));
        assert_eq!(
            args,
            vec![
                "/d",
                "/s",
                "/c",
                r"C:\tools\atk.cmd",
                "auth",
                "list",
                "m365"
            ]
        );
    }

    #[test]
    fn platform_command_spec_windows_bat_routes_via_cmd_exe() {
        let prog = Path::new(r"C:\tools\npm.bat");
        let (exe, args) = build_platform_command_spec(prog, &["--version"], true);
        assert_eq!(exe, Path::new("cmd.exe"));
        assert_eq!(
            args,
            vec!["/d", "/s", "/c", r"C:\tools\npm.bat", "--version"]
        );
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

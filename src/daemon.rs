use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::util::FatalError;

fn ensure_parent_dir(path: &Path) -> Result<(), FatalError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FatalError::Gateway(format!(
                "failed to create pid file directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    Ok(())
}

/// The lock file that guards `pid_path`: the same path with a `.lock`
/// extension (`residuum.pid` → `residuum.lock`).
///
/// The lock lives in its own file because Windows locks are mandatory: a lock
/// on the PID file itself would stop other processes (`residuum stop`) from
/// reading the PID.
fn lock_path_for(pid_path: &Path) -> PathBuf {
    pid_path.with_extension("lock")
}

pub struct PidFileLock {
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
}

pub fn acquire_pid_lock(pid_path: &Path) -> Result<PidFileLock, FatalError> {
    ensure_parent_dir(pid_path)?;
    let lock_path = lock_path_for(pid_path);

    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| {
            FatalError::Gateway(format!(
                "failed to open lock file {}: {e}",
                lock_path.display()
            ))
        })?;

    let rw_lock = Box::leak(Box::new(fd_lock::RwLock::new(file)));

    let guard = rw_lock.try_write().map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            FatalError::Gateway(format!(
                "another instance is already running (lock held on {})",
                lock_path.display()
            ))
        } else {
            FatalError::Gateway(format!("failed to lock {}: {e}", lock_path.display()))
        }
    })?;

    let pid = std::process::id();
    std::fs::write(pid_path, pid.to_string()).map_err(|e| {
        FatalError::Gateway(format!(
            "failed to write pid file {}: {e}",
            pid_path.display()
        ))
    })?;

    tracing::debug!(path = %lock_path.display(), pid, "acquired daemon lock");

    Ok(PidFileLock { _guard: guard })
}

pub fn is_pid_locked(pid_path: &Path) -> Result<bool, FatalError> {
    let lock_path = lock_path_for(pid_path);
    if is_file_locked(&lock_path)? {
        return Ok(true);
    }
    if lock_path.exists() {
        Ok(false)
    } else {
        is_file_locked(pid_path)
    }
}

fn is_file_locked(path: &Path) -> Result<bool, FatalError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(FatalError::Gateway(format!(
                "failed to open {}: {e}",
                path.display()
            )));
        }
    };

    let mut rw_lock = fd_lock::RwLock::new(file);
    match rw_lock.try_write() {
        Ok(_guard) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(FatalError::Gateway(format!(
            "failed to probe lock on {}: {e}",
            path.display()
        ))),
    }
}

pub fn write_crash_note(msg: &str) {
    let path = crate::config::default_hub_dir().map_or_else(
        |_| std::env::temp_dir().join("residuum-crash.log"),
        |hub_dir| crate::config::HubPaths::new(hub_dir).crash_log(),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{}: {msg}", chrono::Utc::now())
        })
        .ok();
}

pub fn pid_file_path() -> Result<PathBuf, FatalError> {
    crate::config::default_hub_dir().map(|hub_dir| crate::config::HubPaths::new(hub_dir).pid_file())
}

pub fn read_pid_file(path: &Path) -> Result<u32, FatalError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FatalError::Gateway(format!("failed to read pid file {}: {e}", path.display()))
    })?;
    content
        .trim()
        .parse::<u32>()
        .map_err(|e| FatalError::Gateway(format!("invalid pid in {}: {e}", path.display())))
}

pub fn remove_pid_file(path: &Path) -> Result<(), FatalError> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            tracing::debug!(path = %path.display(), "removed pid file");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(FatalError::Gateway(format!(
            "failed to remove pid file {}: {e}",
            path.display()
        ))),
    }
}

#[must_use]
#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let Ok(nix_pid) = i32::try_from(pid).map(Pid::from_raw) else {
        tracing::warn!(pid, "PID out of i32 range; cannot check process status");
        return false;
    };
    match kill(nix_pid, None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(nix::errno::Errno::EPERM) => {
            tracing::warn!(pid, "got EPERM checking process; assuming running");
            true
        }
        Err(e) => {
            tracing::warn!(pid, error = %e, "unexpected error checking process status");
            false
        }
    }
}

#[must_use]
#[cfg(windows)]
#[expect(unsafe_code, reason = "Win32 FFI for process management")]
pub fn is_process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: OpenProcess returns a valid handle or null. We close it before returning.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut exit_code: u32 = 0;
    // SAFETY: handle is valid (non-null check above), exit_code is a valid pointer.
    let ok = unsafe { GetExitCodeProcess(handle, &raw mut exit_code) };
    unsafe { CloseHandle(handle) };
    ok != 0 && exit_code == STILL_ACTIVE as u32
}

/// Send a termination signal to the process with the given PID.
///
/// On Unix this sends SIGTERM; on Windows this calls `TerminateProcess`.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the signal/termination cannot be sent.
#[cfg(unix)]
pub fn send_sigterm(pid: u32) -> Result<(), FatalError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(
        i32::try_from(pid)
            .map_err(|e| FatalError::Gateway(format!("pid {pid} out of range for signal: {e}")))?,
    );

    kill(nix_pid, Signal::SIGTERM)
        .map_err(|e| FatalError::Gateway(format!("failed to send SIGTERM to pid {pid}: {e}")))?;
    tracing::debug!(pid, "sent SIGTERM");
    Ok(())
}

/// Send a termination signal to the process with the given PID.
///
/// On Unix this sends SIGTERM; on Windows this calls `TerminateProcess`.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the signal/termination cannot be sent.
#[cfg(windows)]
#[expect(unsafe_code, reason = "Win32 FFI for process management")]
pub fn send_sigterm(pid: u32) -> Result<(), FatalError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: OpenProcess returns a valid handle or null.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(FatalError::Gateway(format!(
            "failed to open process {pid} for termination"
        )));
    }
    // SAFETY: handle is valid (non-null check above).
    let ok = unsafe { TerminateProcess(handle, 1) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(FatalError::Gateway(format!(
            "failed to terminate process {pid}"
        )));
    }
    tracing::debug!(pid, "terminated process");
    Ok(())
}

pub const READINESS_TIMEOUT: Duration = Duration::from_secs(60);

#[must_use]
pub fn ready_file_path(hub_dir: &Path) -> PathBuf {
    crate::config::HubPaths::new(hub_dir).ready_file()
}

pub fn write_ready_file(hub_dir: &Path) {
    let path = ready_file_path(hub_dir);
    if let Err(e) = std::fs::write(&path, std::process::id().to_string()) {
        tracing::warn!(path = %path.display(), error = %e, "failed to write gateway readiness marker");
    }
}

pub fn remove_ready_file(hub_dir: &Path) {
    let path = ready_file_path(hub_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove gateway readiness marker");
    }
}

#[must_use]
pub fn startup_error_path(hub_dir: &Path) -> PathBuf {
    crate::config::HubPaths::new(hub_dir).startup_error_file()
}

pub fn write_startup_error(hub_dir: &Path, message: &str) {
    let path = startup_error_path(hub_dir);
    if let Err(e) = std::fs::write(&path, message) {
        tracing::warn!(path = %path.display(), error = %e, "failed to write startup error marker");
    }
}

pub fn clear_startup_error(hub_dir: &Path) {
    let path = startup_error_path(hub_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove startup error marker");
    }
}

#[must_use]
pub fn read_startup_error(hub_dir: &Path) -> Option<String> {
    std::fs::read_to_string(startup_error_path(hub_dir)).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessOutcome {
    Ready,
    ProcessExited,
    TimedOut,
}

pub fn wait_for_ready(
    ready_path: &Path,
    timeout: Duration,
    poll_interval: Duration,
    mut still_running: impl FnMut() -> bool,
) -> ReadinessOutcome {
    let start = Instant::now();
    loop {
        if ready_path.exists() {
            return ReadinessOutcome::Ready;
        }
        if !still_running() {
            return ReadinessOutcome::ProcessExited;
        }
        if start.elapsed() > timeout {
            return ReadinessOutcome::TimedOut;
        }
        std::thread::sleep(poll_interval);
    }
}

#[must_use]
pub fn stderr_log_path(hub_dir: &Path) -> PathBuf {
    crate::config::HubPaths::new(hub_dir)
        .logs_dir()
        .join("serve.stderr.log")
}

pub fn spawn_gateway_process(
    exe: &Path,
    args: &[String],
    hub_dir: &Path,
) -> std::io::Result<std::process::Child> {
    let log_dir = crate::config::HubPaths::new(hub_dir).logs_dir();
    std::fs::create_dir_all(&log_dir)?;
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log_path(hub_dir))?;

    std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_file)
        .spawn()
}

/// Replace the current process image with `exe` and `args`, with stdin closed,
/// stdout discarded, and stderr appended to [`stderr_log_path`].
///
/// Used by `commands::serve::foreground::relaunch_with_watchdog` on Unix so
/// handing off to the update-rollback watchdog keeps the same PID — preventing
/// service managers like systemd (`KillMode=control-group`) from seeing the
/// main process exit and killing the cgroup mid-update.
///
/// Only returns if preparing the log file or `exec()` fails.
#[cfg(unix)]
#[must_use]
pub fn exec_gateway_process(exe: &Path, args: &[String], hub_dir: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;

    let log_dir = crate::config::HubPaths::new(hub_dir).logs_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        return e;
    }
    let stderr_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log_path(hub_dir))
    {
        Ok(file) => file,
        Err(e) => return e,
    };

    std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_file)
        .exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_detected_as_running() {
        let pid = std::process::id();
        assert!(is_process_running(pid));
    }

    #[test]
    fn nonexistent_pid_is_not_running() {
        // Windows has no `true` outside Git's usr\bin, so exit through cmd.
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit 0"])
                .spawn()
                .unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        assert!(!is_process_running(pid));
    }

    #[test]
    fn pid_overflow_returns_false() {
        assert!(!is_process_running(u32::MAX));
    }

    #[test]
    fn pid_lock_acquire_writes_pid() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        let content = std::fs::read_to_string(&pid_path).unwrap();
        assert_eq!(
            content.trim().parse::<u32>().unwrap(),
            std::process::id(),
            "lock file should contain current PID"
        );
    }

    #[test]
    fn pid_lock_detected_as_held() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        assert!(
            is_pid_locked(&pid_path).unwrap(),
            "lock should be detected as held"
        );
    }

    #[test]
    fn pid_lock_stale_file_detected() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");

        std::fs::write(&pid_path, "99999").unwrap();

        assert!(
            !is_pid_locked(&pid_path).unwrap(),
            "unlocked pid file should be detected as stale"
        );
    }

    #[test]
    fn pid_lock_missing_file_not_locked() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("nonexistent.pid");

        assert!(
            !is_pid_locked(&pid_path).unwrap(),
            "missing file should not be detected as locked"
        );
    }

    #[test]
    fn pid_lock_second_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();

        let result = acquire_pid_lock(&pid_path);
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("already running"),
            "unexpected error: {err_msg}"
        );
    }

    #[test]
    fn pid_file_is_readable_while_the_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("residuum.pid");
        let _lock = acquire_pid_lock(&pid_path).unwrap();
        assert_eq!(read_pid_file(&pid_path).unwrap(), std::process::id());
        assert!(dir.path().join("residuum.lock").exists());
    }

    #[test]
    fn pid_lock_released_lock_file_is_not_locked() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("residuum.pid");
        drop(acquire_pid_lock(&pid_path).unwrap());
        assert!(!is_pid_locked(&pid_path).unwrap());
    }

    #[test]
    fn pid_lock_detects_a_daemon_that_locks_the_pid_file_itself() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("residuum.pid");
        std::fs::write(&pid_path, "1234").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pid_path)
            .unwrap();
        let mut legacy = fd_lock::RwLock::new(file);
        let _guard = legacy.try_write().unwrap();
        assert!(is_pid_locked(&pid_path).unwrap());
    }

    #[test]
    fn read_pid_file_parses_pid_with_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pid");
        std::fs::write(&path, "1234\n").unwrap();
        assert_eq!(read_pid_file(&path).unwrap(), 1234);
    }

    #[test]
    fn read_pid_file_missing_file_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.pid");
        assert!(read_pid_file(&path).is_err());
    }

    #[test]
    fn read_pid_file_invalid_content_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        assert!(read_pid_file(&path).is_err());
    }

    #[test]
    fn remove_pid_file_succeeds_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pid");
        std::fs::write(&path, "1234").unwrap();
        remove_pid_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn remove_pid_file_succeeds_silently_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.pid");
        assert!(remove_pid_file(&path).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pid_1_is_detected_as_running() {
        assert!(is_process_running(1));
    }

    #[test]
    fn ready_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!ready_file_path(dir.path()).exists());
        write_ready_file(dir.path());
        assert!(ready_file_path(dir.path()).exists());
        remove_ready_file(dir.path());
        assert!(!ready_file_path(dir.path()).exists());
    }

    #[test]
    fn remove_ready_file_succeeds_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        remove_ready_file(dir.path());
    }

    #[test]
    fn startup_error_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_startup_error(dir.path()), None);
        write_startup_error(
            dir.path(),
            "gateway error: failed to bind to 127.0.0.1:7700",
        );
        assert_eq!(
            read_startup_error(dir.path()).as_deref(),
            Some("gateway error: failed to bind to 127.0.0.1:7700")
        );
        clear_startup_error(dir.path());
        assert_eq!(read_startup_error(dir.path()), None);
    }

    #[test]
    fn wait_for_ready_reports_ready_once_marker_appears() {
        let dir = tempfile::tempdir().unwrap();
        let path = ready_file_path(dir.path());
        std::fs::write(&path, "123").unwrap();
        let outcome = wait_for_ready(
            &path,
            Duration::from_secs(1),
            Duration::from_millis(10),
            || true,
        );
        assert_eq!(outcome, ReadinessOutcome::Ready);
    }

    #[test]
    fn wait_for_ready_reports_process_exited_when_still_running_goes_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = ready_file_path(dir.path());
        let outcome = wait_for_ready(
            &path,
            Duration::from_secs(5),
            Duration::from_millis(10),
            || false,
        );
        assert_eq!(outcome, ReadinessOutcome::ProcessExited);
    }

    #[test]
    fn wait_for_ready_reports_timed_out_when_nothing_happens() {
        let dir = tempfile::tempdir().unwrap();
        let path = ready_file_path(dir.path());
        let outcome = wait_for_ready(
            &path,
            Duration::from_millis(30),
            Duration::from_millis(10),
            || true,
        );
        assert_eq!(outcome, ReadinessOutcome::TimedOut);
    }

    #[test]
    fn spawn_gateway_process_redirects_stderr_to_the_log_file() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let (shell, args) = ("cmd", ["/C".to_string(), "echo boom 1>&2".to_string()]);
        #[cfg(not(windows))]
        let (shell, args) = ("sh", ["-c".to_string(), "echo boom 1>&2".to_string()]);
        let mut child =
            spawn_gateway_process(std::path::Path::new(shell), &args, dir.path()).unwrap();
        child.wait().unwrap();
        let logged = std::fs::read_to_string(stderr_log_path(dir.path())).unwrap();
        assert!(
            logged.contains("boom"),
            "stderr should land in the log file, got: {logged}"
        );
    }
}

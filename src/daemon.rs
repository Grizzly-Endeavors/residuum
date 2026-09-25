//! Daemon utilities for backgrounding the gateway process.
//!
//! Provides PID file management, process detection, signal sending,
//! file locking, and the readiness/startup-error markers a startup attempt
//! leaves for whoever is waiting on it (the CLI reporting `serve`'s outcome,
//! or the update-rollback watchdog deciding whether to roll back).

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

/// Holds an exclusive lock on the daemon's lock file for its lifetime.
///
/// When this value is dropped (or the process exits for any reason including
/// SIGKILL), the OS releases the lock. Other processes can detect a live
/// daemon by attempting a non-blocking lock on the same file.
///
/// The inner guard has a `'static` lifetime because the backing `RwLock` is
/// heap-allocated via `Box::leak` — a deliberate one-time leak for a
/// process-lifetime singleton.
pub struct PidFileLock {
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
}

/// Take the daemon lock for `pid_path` and write the current PID to it.
///
/// The returned [`PidFileLock`] must be held for the entire daemon lifetime.
/// If another process already holds the lock, this returns an error.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the lock file cannot be opened, the lock
/// is already held, or the PID cannot be written.
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

    // Leak the RwLock to get a 'static lifetime for the guard.
    // This is a one-time allocation for a process-lifetime singleton.
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

/// Check whether a live daemon holds the lock for `pid_path`.
///
/// Returns `true` if the lock is held (process is alive), `false` if it can
/// be acquired (process is dead or never started).
///
/// # Errors
///
/// Returns `FatalError::Gateway` if a lock file exists but cannot be opened
/// or probed.
pub fn is_pid_locked(pid_path: &Path) -> Result<bool, FatalError> {
    let lock_path = lock_path_for(pid_path);
    if is_file_locked(&lock_path)? {
        return Ok(true);
    }
    // Daemons started by older binaries have no lock file and lock the PID
    // file itself; one of those is still running if that lock is held.
    if lock_path.exists() {
        Ok(false)
    } else {
        is_file_locked(pid_path)
    }
}

/// Whether another process holds an exclusive lock on `path`. A missing file
/// is not locked.
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
        // Acquired: nobody holds it. The guard drops immediately, releasing
        // this probe's lock.
        Ok(_guard) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(FatalError::Gateway(format!(
            "failed to probe lock on {}: {e}",
            path.display()
        ))),
    }
}

/// Write a diagnostic message to the crash log.
///
/// Used for errors that occur before tracing is initialized
/// or when the tracing subsystem itself fails. Messages are
/// appended to `~/.residuum/crash.log` (falls back to
/// `/tmp/residuum-crash.log` if the home directory is unavailable).
pub fn write_crash_note(msg: &str) {
    let path = dirs::home_dir().map_or_else(
        || std::env::temp_dir().join("residuum-crash.log"),
        |h| h.join(".residuum").join("crash.log"),
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

/// Return the path to the PID file: `~/.residuum/residuum.pid`.
///
/// # Errors
///
/// Returns `FatalError::Config` if the home directory cannot be determined.
pub fn pid_file_path() -> Result<PathBuf, FatalError> {
    dirs::home_dir()
        .map(|h| h.join(".residuum").join("residuum.pid"))
        .ok_or_else(|| FatalError::Config("could not determine home directory".to_string()))
}

/// Read a PID from the given file path.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the file cannot be read or parsed.
pub fn read_pid_file(path: &Path) -> Result<u32, FatalError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        FatalError::Gateway(format!("failed to read pid file {}: {e}", path.display()))
    })?;
    content
        .trim()
        .parse::<u32>()
        .map_err(|e| FatalError::Gateway(format!("invalid pid in {}: {e}", path.display())))
}

/// Remove the PID file at the given path.
///
/// Silently succeeds if the file does not exist.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if removal fails for a reason other than
/// the file not existing.
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

/// Check whether a process with the given PID is currently running.
#[must_use]
#[cfg(unix)]
pub fn is_process_running(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let Ok(nix_pid) = i32::try_from(pid).map(Pid::from_raw) else {
        tracing::warn!(pid, "PID out of i32 range; cannot check process status");
        return false;
    };
    // Signal 0 checks process existence without sending a signal.
    // Returns Ok if the process exists and we have permission to signal it.
    // Returns ESRCH if no such process, EPERM if it exists but we lack permission.
    // EPERM means the process is running, but since we own the daemon this shouldn't occur.
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

/// Check whether a process with the given PID is currently running.
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

// ── Readiness and startup-error markers ──────────────────────────────

/// How long a waiter (the CLI reporting `serve`'s outcome, or the
/// update-rollback watchdog) gives a startup attempt to become healthy
/// before giving up.
pub const READINESS_TIMEOUT: Duration = Duration::from_secs(60);

/// Path to the marker a gateway process writes once it has finished
/// initializing (providers, workspace) and its HTTP listener is bound and
/// accepting connections. Its absence means the process is still starting,
/// never started, or has exited.
#[must_use]
pub fn ready_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("residuum.ready")
}

/// Write the readiness marker for the current process.
///
/// Best-effort: a failure here just means a waiter treats startup as
/// never becoming ready, which times out visibly rather than reporting
/// success silently — it never affects the running gateway.
pub fn write_ready_file(config_dir: &Path) {
    let path = ready_file_path(config_dir);
    if let Err(e) = std::fs::write(&path, std::process::id().to_string()) {
        tracing::warn!(path = %path.display(), error = %e, "failed to write gateway readiness marker");
    }
}

/// Remove the readiness marker, e.g. before a fresh startup attempt or on
/// shutdown, so a later waiter never reads a marker left by a previous run.
///
/// Best-effort, matching [`write_ready_file`]: a failure just means a stale
/// marker might linger, which a fresh attempt clears again before it
/// matters.
pub fn remove_ready_file(config_dir: &Path) {
    let path = ready_file_path(config_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove gateway readiness marker");
    }
}

/// Path to the plain-language message a failed startup attempt leaves
/// behind, read by the CLI process that spawned it (or the update-rollback
/// watchdog) to report why.
#[must_use]
pub fn startup_error_path(config_dir: &Path) -> PathBuf {
    config_dir.join("residuum.startup-error")
}

/// Record why a startup attempt failed, for whoever is waiting on it.
///
/// Best-effort, matching [`write_ready_file`]: on failure the waiter just
/// falls back to its own generic message.
pub fn write_startup_error(config_dir: &Path, message: &str) {
    let path = startup_error_path(config_dir);
    if let Err(e) = std::fs::write(&path, message) {
        tracing::warn!(path = %path.display(), error = %e, "failed to write startup error marker");
    }
}

/// Clear a startup error left by an earlier attempt, e.g. before a fresh one.
pub fn clear_startup_error(config_dir: &Path) {
    let path = startup_error_path(config_dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "failed to remove startup error marker");
    }
}

/// Read the plain-language message a failed startup attempt left behind,
/// if any.
#[must_use]
pub fn read_startup_error(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(startup_error_path(config_dir)).ok()
}

/// How a wait for gateway readiness ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessOutcome {
    /// The readiness marker appeared before the process exited or the
    /// timeout elapsed.
    Ready,
    /// The watched process exited before becoming ready.
    ProcessExited,
    /// Neither readiness nor exit was observed within the timeout.
    TimedOut,
}

/// Poll for the readiness marker at `ready_path`, checking `still_running`
/// on every tick so a process that exits early is reported as such instead
/// of running out the full timeout. Purely synchronous (blocking sleep) so
/// it has no dependency on an async runtime being present.
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

/// Where a daemon- or watchdog-spawned gateway process's stderr is appended,
/// so a panic or a pre-tracing startup error is never silently dropped.
#[must_use]
pub fn stderr_log_path(config_dir: &Path) -> PathBuf {
    config_dir.join("logs").join("serve.stderr.log")
}

/// Spawn `exe` with `args` as a detached background process: stdin closed,
/// stdout discarded (nothing writes anything meaningful there), and stderr
/// appended to [`stderr_log_path`] rather than discarded, so a crash or an
/// error before tracing initializes still leaves a trace. Used to start the
/// gateway itself (by the daemon spawner and by the update-rollback
/// watchdog) and to hand a restart off to that watchdog (by
/// `commands::serve::foreground::relaunch`) — anywhere this codebase starts
/// a detached `residuum` process.
///
/// # Errors
///
/// Returns an I/O error if the log directory or file can't be created, or
/// the process can't be spawned.
pub fn spawn_gateway_process(
    exe: &Path,
    args: &[String],
    config_dir: &Path,
) -> std::io::Result<std::process::Child> {
    let log_dir = config_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log_path(config_dir))?;

    std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr_file)
        .spawn()
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
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!is_process_running(pid));
    }

    #[test]
    fn pid_overflow_returns_false() {
        // u32::MAX cannot be converted to i32, so this should return false
        // via the try_from guard rather than panicking.
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

        // A second fd should see the lock as held
        assert!(
            is_pid_locked(&pid_path).unwrap(),
            "lock should be detected as held"
        );
    }

    #[test]
    fn pid_lock_stale_file_detected() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("test.pid");

        // Write a PID file without acquiring a lock
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
        // Windows locks are mandatory, so this fails there if the lock is
        // taken on the PID file itself.
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
        // Should not panic or error even though nothing was ever written.
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
        let mut child = spawn_gateway_process(
            std::path::Path::new("sh"),
            &["-c".to_string(), "echo boom 1>&2".to_string()],
            dir.path(),
        )
        .unwrap();
        child.wait().unwrap();
        let logged = std::fs::read_to_string(stderr_log_path(dir.path())).unwrap();
        assert!(
            logged.contains("boom"),
            "stderr should land in the log file, got: {logged}"
        );
    }
}

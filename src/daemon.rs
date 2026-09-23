//! Daemon utilities for backgrounding the gateway process.
//!
//! Provides PID file management, process detection, signal sending,
//! and file locking.

use std::path::{Path, PathBuf};

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
}

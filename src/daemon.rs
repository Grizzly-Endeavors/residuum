//! Daemon utilities for backgrounding the gateway process.
//!
//! Provides PID file management, process detection, signal sending,
//! file locking, and file-only tracing initialization for daemonized operation.

use std::path::{Path, PathBuf};

use crate::util::FatalError;

/// Holds an exclusive advisory lock on the PID file for the daemon's lifetime.
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

/// Acquire an exclusive lock on the PID file and write the current PID.
///
/// The returned [`PidFileLock`] must be held for the entire daemon lifetime.
/// If another process already holds the lock, this returns an error.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the file cannot be opened, the lock
/// is already held, or the PID cannot be written.
pub fn acquire_pid_lock(path: &Path) -> Result<PidFileLock, FatalError> {
    use std::io::{Seek, SeekFrom, Write};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FatalError::Gateway(format!(
                "failed to create pid file directory {}: {e}",
                parent.display()
            ))
        })?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| {
            FatalError::Gateway(format!("failed to open pid file {}: {e}", path.display()))
        })?;

    // Leak the RwLock to get a 'static lifetime for the guard.
    // This is a one-time allocation for a process-lifetime singleton.
    let rw_lock = Box::leak(Box::new(fd_lock::RwLock::new(file)));

    let mut guard = rw_lock.try_write().map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            FatalError::Gateway(format!(
                "another instance is already running (lock held on {})",
                path.display()
            ))
        } else {
            FatalError::Gateway(format!("failed to lock pid file {}: {e}", path.display()))
        }
    })?;

    // Write PID to the locked file
    guard
        .seek(SeekFrom::Start(0))
        .map_err(|e| FatalError::Gateway(format!("failed to seek pid file: {e}")))?;
    guard
        .set_len(0)
        .map_err(|e| FatalError::Gateway(format!("failed to truncate pid file: {e}")))?;
    write!(guard, "{}", std::process::id())
        .map_err(|e| FatalError::Gateway(format!("failed to write pid to lock file: {e}")))?;

    let pid = std::process::id();
    tracing::debug!(path = %path.display(), pid, "acquired pid file lock");

    Ok(PidFileLock { _guard: guard })
}

/// Check whether the PID file is currently locked by another process.
///
/// Returns `true` if the lock is held (process is alive), `false` if
/// the lock can be acquired (process is dead or file doesn't exist).
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the file cannot be opened for a reason
/// other than it not existing.
pub fn is_pid_locked(path: &Path) -> Result<bool, FatalError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(FatalError::Gateway(format!(
                "failed to open pid file {}: {e}",
                path.display()
            )));
        }
    };

    let mut rw_lock = fd_lock::RwLock::new(file);
    match rw_lock.try_write() {
        Ok(_guard) => {
            // Lock acquired — process is dead, this is a stale file.
            // Guard drops immediately, releasing our test lock.
            Ok(false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            // Lock held — process is alive.
            Ok(true)
        }
        Err(e) => Err(FatalError::Gateway(format!(
            "failed to probe pid file lock {}: {e}",
            path.display()
        ))),
    }
}

#[expect(
    clippy::panic,
    reason = "deliberate termination when no log appender can be created"
)]
fn fatal_no_log_appender(msg: &str) -> ! {
    write_crash_note(msg);
    panic!("{msg}")
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

/// Write a PID to the given file path.
///
/// Creates parent directories if needed.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the file cannot be written.
pub fn write_pid_file(path: &Path, pid: u32) -> Result<(), FatalError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            FatalError::Gateway(format!(
                "failed to create pid file directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(path, pid.to_string()).map_err(|e| {
        FatalError::Gateway(format!("failed to write pid file {}: {e}", path.display()))
    })?;
    tracing::debug!(path = %path.display(), pid, "wrote pid file");
    Ok(())
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
///
/// Uses POSIX signal 0 via `kill(pid, None)`, which works on both Linux and macOS.
#[must_use]
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

/// Send `SIGTERM` to the process with the given PID.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the signal cannot be sent.
pub fn send_sigterm(pid: u32) -> Result<(), FatalError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(
        i32::try_from(pid)
            .map_err(|e| FatalError::Gateway(format!("pid {pid} out of range for signal: {e}")))?,
    );

    kill(nix_pid, Signal::SIGTERM)
        .map_err(|e| FatalError::Gateway(format!("failed to send SIGTERM to pid {pid}: {e}")))?;
    tracing::info!(pid, "sent SIGTERM");
    Ok(())
}

/// Debug logging modes for the `--debug` flag.
#[derive(Debug, Clone, Copy)]
pub enum DebugMode {
    /// `--debug` (no value): residuum crates at debug, deps at warn
    Default,
    /// `--debug=all`: everything at debug
    All,
    /// `--debug=trace`: residuum crates at trace, deps at warn
    Trace,
}

impl DebugMode {
    /// Parse a `--debug[=mode]` value into a `DebugMode`.
    ///
    /// Returns `None` for unrecognized modes (caller should report the error).
    #[must_use]
    pub fn from_flag_value(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("") => Some(Self::Default),
            Some("all") => Some(Self::All),
            Some("trace") => Some(Self::Trace),
            Some(_) => None,
        }
    }

    /// The `EnvFilter` directive string for this mode.
    #[must_use]
    pub fn filter_str(self) -> &'static str {
        match self {
            Self::Default => "residuum=debug,warn",
            Self::All => "debug",
            Self::Trace => "residuum=trace,warn",
        }
    }
}

/// Initialize tracing with file-only output for daemonized operation.
///
/// Logs are written to `<log_dir>/serve.YYYY-MM-DD.log` (or `serve-<name>`)
/// with daily rotation and 30-day retention. When `debug_mode` is `Some`,
/// the filter is overridden accordingly and stderr output is added so debug
/// output appears in the terminal.
///
/// When `agent_name` is `Some`, logs go to the agent-specific log directory
/// and the file prefix includes the agent name for identification.
pub fn init_daemon_tracing(debug_mode: Option<DebugMode>, agent_name: Option<&str>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let default_filter = debug_mode.map_or("info", DebugMode::filter_str);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    let log_dir = crate::agent_registry::paths::resolve_log_dir(agent_name).unwrap_or_else(|_| {
        write_crash_note(
            "warning: could not determine log directory; logs will be written to ./logs",
        );
        std::path::PathBuf::from("logs")
    });

    let log_prefix = match agent_name {
        Some(name) => format!("serve-{name}"),
        None => "serve".to_string(),
    };

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .filename_prefix(&log_prefix)
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(30)
        .build(&log_dir)
        .unwrap_or_else(|e| {
            write_crash_note(&format!(
                "warning: failed to create log file appender at {}: {e}; falling back to {}",
                log_dir.display(),
                std::env::temp_dir().display()
            ));
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix(&log_prefix)
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .max_log_files(30)
                .build(std::env::temp_dir())
                .unwrap_or_else(|e2| {
                    fatal_no_log_appender(&format!(
                        "fatal: could not create log appender in temp dir: {e2}"
                    ))
                })
        });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(file_appender);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(debug_mode.map(|_| stderr_layer))
        .init();
    tracing::info!(
        dir = %log_dir.display(),
        prefix = %log_prefix,
        "logging initialized (daily rotation, 30-day retention)"
    );
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_detected_as_running() {
        let pid = std::process::id();
        assert!(is_process_running(pid));
    }

    #[test]
    fn nonexistent_pid_is_not_running() {
        // Use a high PID within i32 range that almost certainly doesn't exist.
        // /proc/sys/kernel/pid_max defaults to 4194304 on 64-bit Linux, and
        // macOS uses similar ranges, so i32::MAX won't be a real process.
        let fake_pid = i32::MAX as u32;
        assert!(!is_process_running(fake_pid));
    }

    #[test]
    fn pid_overflow_returns_false() {
        // u32::MAX cannot be converted to i32, so this should return false
        // via the try_from guard rather than panicking.
        assert!(!is_process_running(u32::MAX));
    }

    #[test]
    fn debug_mode_from_flag_value_none_is_default() {
        assert!(matches!(
            DebugMode::from_flag_value(None),
            Some(DebugMode::Default)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_empty_is_default() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("")),
            Some(DebugMode::Default)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_all() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("all")),
            Some(DebugMode::All)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_trace() {
        assert!(matches!(
            DebugMode::from_flag_value(Some("trace")),
            Some(DebugMode::Trace)
        ));
    }

    #[test]
    fn debug_mode_from_flag_value_unknown_is_none() {
        assert!(DebugMode::from_flag_value(Some("bogus")).is_none());
    }

    #[test]
    fn debug_mode_filter_strings() {
        assert_eq!(DebugMode::Default.filter_str(), "residuum=debug,warn");
        assert_eq!(DebugMode::All.filter_str(), "debug");
        assert_eq!(DebugMode::Trace.filter_str(), "residuum=trace,warn");
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
        write_pid_file(&pid_path, 99999).unwrap();

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
        assert!(result.is_err(), "second lock acquisition should fail");
    }
}

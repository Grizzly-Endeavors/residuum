//! Daemon utilities for backgrounding the gateway process.
//!
//! Provides PID file management, process detection, signal sending,
//! and file-only tracing initialization for daemonized operation.

use std::path::{Path, PathBuf};

use crate::error::ResiduumError;

/// Return the path to the PID file: `~/.residuum/residuum.pid`.
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn pid_file_path() -> Result<PathBuf, ResiduumError> {
    dirs::home_dir()
        .map(|h| h.join(".residuum").join("residuum.pid"))
        .ok_or_else(|| ResiduumError::Config("could not determine home directory".to_string()))
}

/// Write a PID to the given file path.
///
/// Creates parent directories if needed.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the file cannot be written.
pub fn write_pid_file(path: &Path, pid: u32) -> Result<(), ResiduumError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ResiduumError::Gateway(format!(
                "failed to create pid file directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(path, pid.to_string()).map_err(|e| {
        ResiduumError::Gateway(format!("failed to write pid file {}: {e}", path.display()))
    })
}

/// Read a PID from the given file path.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the file cannot be read or parsed.
pub fn read_pid_file(path: &Path) -> Result<u32, ResiduumError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ResiduumError::Gateway(format!("failed to read pid file {}: {e}", path.display()))
    })?;
    content
        .trim()
        .parse::<u32>()
        .map_err(|e| ResiduumError::Gateway(format!("invalid pid in {}: {e}", path.display())))
}

/// Remove the PID file at the given path.
///
/// Silently succeeds if the file does not exist.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if removal fails for a reason other than
/// the file not existing.
pub fn remove_pid_file(path: &Path) -> Result<(), ResiduumError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ResiduumError::Gateway(format!(
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
        return false;
    };
    // Signal 0 checks process existence without sending a signal.
    // Returns Ok if the process exists and we have permission to signal it.
    // Returns ESRCH if no such process, EPERM if it exists but we lack permission.
    // EPERM means the process is running, but since we own the daemon this shouldn't occur.
    kill(nix_pid, None).is_ok()
}

/// Send `SIGTERM` to the process with the given PID.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the signal cannot be sent.
pub fn send_sigterm(pid: u32) -> Result<(), ResiduumError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let nix_pid =
        Pid::from_raw(i32::try_from(pid).map_err(|e| {
            ResiduumError::Gateway(format!("pid {pid} out of range for signal: {e}"))
        })?);

    kill(nix_pid, Signal::SIGTERM)
        .map_err(|e| ResiduumError::Gateway(format!("failed to send SIGTERM to pid {pid}: {e}")))
}

/// Initialize tracing with file-only output for daemonized operation.
///
/// Logs are written to `~/.residuum/logs/serve.YYYY-MM-DD.log` with daily
/// rotation and 30-day retention. No stderr output.
pub fn init_daemon_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_dir = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("logs"),
        |h| h.join(".residuum").join("logs"),
    );

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .filename_prefix("serve")
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(30)
        .build(&log_dir)
        .unwrap_or_else(|e| {
            eprintln!("warning: failed to create log file appender: {e}");
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix("serve")
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .build(std::env::temp_dir())
                .unwrap_or_else(|e2| {
                    eprintln!("warning: fallback log appender also failed: {e2}");
                    tracing_appender::rolling::daily(std::env::temp_dir(), "serve.log")
                })
        });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(file_appender);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .init();
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
}

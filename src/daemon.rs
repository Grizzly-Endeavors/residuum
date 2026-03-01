//! Daemon utilities for backgrounding the gateway process.
//!
//! Provides PID file management, process detection, signal sending,
//! and file-only tracing initialization for daemonized operation.

use std::path::{Path, PathBuf};

use crate::error::IronclawError;

/// Return the path to the PID file: `~/.ironclaw/ironclaw.pid`.
///
/// # Errors
///
/// Returns `IronclawError::Config` if the home directory cannot be determined.
pub fn pid_file_path() -> Result<PathBuf, IronclawError> {
    dirs::home_dir()
        .map(|h| h.join(".ironclaw").join("ironclaw.pid"))
        .ok_or_else(|| IronclawError::Config("could not determine home directory".to_string()))
}

/// Write a PID to the given file path.
///
/// Creates parent directories if needed.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` if the file cannot be written.
pub fn write_pid_file(path: &Path, pid: u32) -> Result<(), IronclawError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            IronclawError::Gateway(format!(
                "failed to create pid file directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(path, pid.to_string()).map_err(|e| {
        IronclawError::Gateway(format!(
            "failed to write pid file {}: {e}",
            path.display()
        ))
    })
}

/// Read a PID from the given file path.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` if the file cannot be read or parsed.
pub fn read_pid_file(path: &Path) -> Result<u32, IronclawError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        IronclawError::Gateway(format!(
            "failed to read pid file {}: {e}",
            path.display()
        ))
    })?;
    content.trim().parse::<u32>().map_err(|e| {
        IronclawError::Gateway(format!(
            "invalid pid in {}: {e}",
            path.display()
        ))
    })
}

/// Remove the PID file at the given path.
///
/// Silently succeeds if the file does not exist.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` if removal fails for a reason other than
/// the file not existing.
pub fn remove_pid_file(path: &Path) -> Result<(), IronclawError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(IronclawError::Gateway(format!(
            "failed to remove pid file {}: {e}",
            path.display()
        ))),
    }
}

/// Check whether a process with the given PID is currently running.
///
/// Uses `/proc/{pid}` existence on Linux.
#[must_use]
pub fn is_process_running(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Send `SIGTERM` to the process with the given PID.
///
/// # Errors
///
/// Returns `IronclawError::Gateway` if the signal cannot be sent.
pub fn send_sigterm(pid: u32) -> Result<(), IronclawError> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let nix_pid = Pid::from_raw(i32::try_from(pid).map_err(|e| {
        IronclawError::Gateway(format!("pid {pid} out of range for signal: {e}"))
    })?);

    kill(nix_pid, Signal::SIGTERM).map_err(|e| {
        IronclawError::Gateway(format!("failed to send SIGTERM to pid {pid}: {e}"))
    })
}

/// Initialize tracing with file-only output for daemonized operation.
///
/// Logs are written to `~/.ironclaw/logs/serve.YYYY-MM-DD.log` with daily
/// rotation and 30-day retention. No stderr output.
pub fn init_daemon_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_dir = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("logs"),
        |h| h.join(".ironclaw").join("logs"),
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

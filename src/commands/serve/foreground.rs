//! Foreground process lifecycle and setup wizard for the gateway.

use residuum::config::Config;
use residuum::util::FatalError;

use super::ServeArgs;
use super::startup_config::{ConfigProblem, classify_load_error};

/// How the foreground gateway finished.
enum ForegroundExit {
    /// Shut down; the process should exit.
    Done,
    /// A restart was requested (the binary was updated); relaunch it.
    Restart,
}

/// Run the gateway in foreground mode (called as `residuum serve --foreground`).
///
/// Used by the daemon spawner as the child process, or for debugging.
///
/// # Errors
///
/// Returns `FatalError` if initialization or the gateway loop fails.
#[tracing::instrument(skip_all)]
pub(crate) async fn run_serve_foreground(args: &ServeArgs) -> Result<(), FatalError> {
    let config_dir = residuum::config::Config::config_dir()?;
    let pid_path = config_dir.join("residuum.pid");

    // Acquire exclusive lock on the PID file. This both:
    // 1. Prevents two instances from running simultaneously
    // 2. Makes stale PID files detectable (lock released on process death)
    let pid_lock = residuum::daemon::acquire_pid_lock(&pid_path)?;

    // Clear markers a previous attempt left behind, so a waiter (the CLI or
    // the update-rollback watchdog) never reads a stale readiness signal or
    // error from before this attempt started.
    residuum::daemon::remove_ready_file(&config_dir);
    residuum::daemon::clear_startup_error(&config_dir);

    // Restart is handled here, above the gateway, so PID-file cleanup and the
    // relaunch happen in one place and in the right order. `relaunch` owns the
    // PID file from here: after a restart it may belong to the new process.
    let result = run_serve_foreground_inner(args).await;
    if let Ok(ForegroundExit::Restart) = result {
        return relaunch(pid_lock, &pid_path);
    }

    if let Err(ref e) = result {
        residuum::daemon::write_startup_error(&config_dir, &e.to_string());
    }

    // Clean up PID file and readiness marker on exit. On crash/SIGKILL the
    // lock is released by the OS, and the next startup detects the stale
    // file.
    remove_pid_file(&pid_path);
    residuum::daemon::remove_ready_file(&config_dir);
    result.map(|_| ())
}

fn remove_pid_file(pid_path: &std::path::Path) {
    if let Err(e) = residuum::daemon::remove_pid_file(pid_path) {
        tracing::warn!(error = %e, "failed to remove pid file on exit");
    }
}

/// Run the onboarding wizard in an isolated temp directory, then boot gateway.
#[tracing::instrument(skip_all)]
async fn run_setup_mode() -> Result<ForegroundExit, FatalError> {
    let tmp_dir = std::env::temp_dir().join("residuum-setup");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to clean setup directory {}: {e}",
                tmp_dir.display()
            ))
        })?;
    }
    residuum::config::Config::bootstrap_at_dir(&tmp_dir)?;
    println!(
        "setup mode: config will be written to {}",
        tmp_dir.display()
    );
    match residuum::gateway::setup::run_setup_server_at(tmp_dir.clone()).await? {
        residuum::gateway::setup::SetupExit::ConfigSaved => {
            tracing::debug!("setup complete, loading config from temp directory");
        }
        residuum::gateway::setup::SetupExit::Shutdown => return Ok(ForegroundExit::Done),
    }

    // Load the config written by the wizard and run the gateway
    let mut cfg = Config::load_at(&tmp_dir)?;
    cfg.workspace_dir = tmp_dir.join("workspace");
    if let Some(first) = cfg.skills.dirs.first_mut() {
        *first = residuum::workspace::layout::WorkspaceLayout::new(&cfg.workspace_dir).skills_dir();
    }
    tracing::info!(
        model = cfg.main.first().map_or("(none)", |s| s.model.model.as_str()),
        provider_url = cfg.main.first().map_or("(none)", |s| s.provider_url.as_str()),
        workspace = %cfg.workspace_dir.display(),
        "setup-mode: configuration loaded, starting gateway"
    );
    match Box::pin(residuum::gateway::run_gateway_with_config(cfg)).await? {
        residuum::gateway::GatewayExit::Shutdown => {}
        residuum::gateway::GatewayExit::Restart => {
            // Relaunching would repeat `--setup`, which wipes the temp config.
            tracing::warn!(
                "restart requested in setup mode; exiting instead. Start residuum normally to run the updated binary"
            );
        }
    }
    Ok(ForegroundExit::Done)
}

/// Inner implementation of foreground serve, wrapped by PID file lifecycle.
#[tracing::instrument(skip_all)]
async fn run_serve_foreground_inner(args: &ServeArgs) -> Result<ForegroundExit, FatalError> {
    if args.setup {
        // Box::pin reduces stack frame size — this future is large
        return Box::pin(run_setup_mode()).await;
    }

    let config_dir = residuum::config::Config::config_dir()?;
    loop {
        Config::bootstrap_at_dir(&config_dir)?;

        // A live config that fails to load is only classified (fresh
        // install vs. a file to fix) when there's no last-known-good copy
        // to fall back on — `run_gateway` itself retries on one and
        // publishes a notice once it's up if it had to. This mirrors the
        // check `run_serve_command` makes before spawning the daemon.
        if let Err(err) = Config::load_at(&config_dir)
            && !residuum::gateway::has_last_known_good(&config_dir)
        {
            match classify_load_error(&config_dir, &err) {
                ConfigProblem::NotSetUp => {
                    tracing::info!(error = %err, "config not set up yet, starting setup wizard");
                    // Box::pin reduces stack frame size — this future is large
                    match Box::pin(residuum::gateway::setup::run_setup_server()).await? {
                        residuum::gateway::setup::SetupExit::ConfigSaved => {
                            tracing::debug!("setup complete, loading configuration");
                        }
                        residuum::gateway::setup::SetupExit::Shutdown => break,
                    }
                    continue;
                }
                ConfigProblem::Invalid(invalid) => return Err(invalid),
            }
        }

        // Gateway handles reloads in-place and only returns on shutdown or
        // a fatal error neither the live config nor its last-known-good
        // copy could recover from.
        // Box::pin reduces stack frame size — this future is large
        match Box::pin(residuum::gateway::run_gateway(&config_dir)).await? {
            residuum::gateway::GatewayExit::Restart => return Ok(ForegroundExit::Restart),
            residuum::gateway::GatewayExit::Shutdown => {}
        }
        break;
    }
    Ok(ForegroundExit::Done)
}

/// The path of the running binary, as it now exists on disk.
///
/// On Linux, atomically replacing the binary (via `mv`) unlinks the old inode
/// while the process is still running. The kernel then appends " (deleted)" to
/// `/proc/self/exe`. Strip the suffix to get the live path on disk.
#[cfg(target_os = "linux")]
fn resolve_exe_path(raw: &std::path::Path) -> std::path::PathBuf {
    let s = raw.to_string_lossy();
    if let Some(stripped) = s.strip_suffix(" (deleted)") {
        std::path::PathBuf::from(stripped)
    } else {
        raw.to_path_buf()
    }
}

#[cfg(not(target_os = "linux"))]
fn resolve_exe_path(raw: &std::path::Path) -> std::path::PathBuf {
    raw.to_path_buf()
}

/// The command that relaunches this process with the binary now on disk and
/// the same arguments, so `serve --foreground` is preserved.
fn relaunch_command() -> Result<std::process::Command, FatalError> {
    let raw_exe = std::env::current_exe().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for restart: {e}"
        ))
    })?;
    let exe = resolve_exe_path(&raw_exe);
    tracing::info!(exe = %exe.display(), "restarting with updated binary");

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args().skip(1));
    Ok(cmd)
}

/// Relaunch with the (potentially updated) binary.
///
/// Takes the rollback-capable path — handing off to the update watchdog —
/// when a self-update just swapped the binary (a [`residuum::update::PendingRollback`]
/// marker is present); otherwise takes the plain in-place path used for a
/// restart that didn't change the binary.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the executable path can't be determined,
/// or if starting the replacement process fails.
fn relaunch(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
) -> Result<(), FatalError> {
    let config_dir = pid_path.parent().map_or_else(
        || std::path::PathBuf::from("."),
        std::path::Path::to_path_buf,
    );

    // Clear whatever readiness marker the run we're leaving left behind —
    // otherwise a waiter polling for the process we're about to hand off to
    // could read stale success before that process has done anything.
    residuum::daemon::remove_ready_file(&config_dir);

    if let Some(pending) = residuum::update::take_pending_rollback(&config_dir) {
        return relaunch_with_watchdog(pid_lock, pid_path, &config_dir, &pending);
    }
    relaunch_plain(pid_lock, pid_path)
}

/// Hand a restart off to the update-rollback watchdog, run from the
/// just-preserved previous binary — proven to execute, unlike the new one —
/// so it can supervise the new version's startup and roll back if it never
/// becomes healthy. See `commands::update_watchdog`.
fn relaunch_with_watchdog(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
    config_dir: &std::path::Path,
    pending: &residuum::update::PendingRollback,
) -> Result<(), FatalError> {
    remove_pid_file(pid_path);
    drop(pid_lock);

    let raw_exe = std::env::current_exe().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for restart: {e}"
        ))
    })?;
    let gateway_exe = resolve_exe_path(&raw_exe);
    let serve_args: Vec<String> = std::env::args().skip(1).collect();

    let mut watchdog_args = vec![
        "update-watchdog".to_string(),
        "--gateway-exe".to_string(),
        gateway_exe.display().to_string(),
        "--prev-exe".to_string(),
        pending.previous_binary.display().to_string(),
        "--previous-version".to_string(),
        pending.previous_version.clone(),
        "--target-version".to_string(),
        pending.target_version.clone(),
        "--".to_string(),
    ];
    watchdog_args.extend(serve_args);

    let child = residuum::daemon::spawn_gateway_process(
        &pending.previous_binary,
        &watchdog_args,
        config_dir,
    )
    .map_err(|e| FatalError::Gateway(format!("failed to start update-rollback watchdog: {e}")))?;

    tracing::info!(
        pid = child.id(),
        target_version = %pending.target_version,
        "handed restart off to the update-rollback watchdog"
    );
    Ok(())
}

/// Relaunch with the current binary in place, for a restart that isn't
/// rolling out a new version (nothing to roll back to if it fails).
///
/// On Unix, `exec()` replaces the process image in place. The PID stays the
/// same, so the PID file stays correct and the lock is held right up to the
/// exec; the new image takes it again on startup.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the executable path can't be determined
/// or `exec()` fails. It does not return on success.
#[cfg(unix)]
fn relaunch_plain(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
) -> Result<(), FatalError> {
    use std::os::unix::process::CommandExt;

    let err = match relaunch_command() {
        Ok(mut cmd) => FatalError::Gateway(format!("restart failed: {}", cmd.exec())),
        Err(e) => e,
    };
    // Still this process: exec never happened, so clean up as on any exit.
    remove_pid_file(pid_path);
    drop(pid_lock);
    Err(err)
}

/// Relaunch with the current binary in place, for a restart that isn't
/// rolling out a new version (nothing to roll back to if it fails).
///
/// Windows can't replace a running process image, so this starts a new
/// process and returns, letting this one exit normally. The PID file is
/// removed and the lock released first: the new process has a different PID
/// and needs the lock free when it starts.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the executable path can't be determined
/// or the new process can't be started.
#[cfg(windows)]
fn relaunch_plain(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
) -> Result<(), FatalError> {
    remove_pid_file(pid_path);
    drop(pid_lock);
    let mut cmd = relaunch_command()?;
    cmd.spawn()
        .map_err(|e| FatalError::Gateway(format!("failed to start updated binary: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn resolve_exe_path_strips_deleted_suffix() {
        let raw = std::path::Path::new("/usr/bin/residuum (deleted)");
        let result = resolve_exe_path(raw);
        assert_eq!(
            result,
            std::path::PathBuf::from("/usr/bin/residuum"),
            "should strip ' (deleted)' suffix from path"
        );
    }

    #[test]
    fn resolve_exe_path_normal_path_unchanged() {
        let raw = std::path::Path::new("/usr/bin/residuum");
        let result = resolve_exe_path(raw);
        assert_eq!(
            result,
            std::path::PathBuf::from("/usr/bin/residuum"),
            "should return path unchanged when no ' (deleted)' suffix"
        );
    }
}

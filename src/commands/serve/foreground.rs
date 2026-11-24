use residuum::config::HubConfig;
use residuum::util::FatalError;

use super::ServeArgs;
use super::startup_config::ensure_hub_config_loads_or_has_fallback;

enum ForegroundExit {
    Done,
    Restart,
}

#[tracing::instrument(skip_all)]
pub(crate) async fn run_serve_foreground(args: &ServeArgs) -> Result<(), FatalError> {
    let residuum_root = residuum::config::residuum_root()?;
    let hub_dir = residuum::config::paths::hub_dir(&residuum_root);
    let pid_path = residuum::config::HubPaths::new(&hub_dir).pid_file();

    let pid_lock = residuum::daemon::acquire_pid_lock(&pid_path)?;

    residuum::daemon::remove_ready_file(&hub_dir);
    residuum::daemon::clear_startup_error(&hub_dir);

    let result = run_serve_foreground_inner(args).await;
    if let Ok(ForegroundExit::Restart) = result {
        return relaunch(pid_lock, &pid_path);
    }

    if let Err(ref e) = result {
        residuum::daemon::write_startup_error(&hub_dir, &e.to_string());
    }

    // Clean up PID file and readiness marker on exit. On crash/SIGKILL the
    // lock is released by the OS, and the next startup detects the stale
    // file.
    remove_pid_file(&pid_path);
    residuum::daemon::remove_ready_file(&hub_dir);
    result.map(|_| ())
}

fn remove_pid_file(pid_path: &std::path::Path) {
    if let Err(e) = residuum::daemon::remove_pid_file(pid_path) {
        tracing::warn!(error = %e, "failed to remove pid file on exit");
    }
}

#[tracing::instrument(skip_all)]
async fn run_setup_mode() -> Result<ForegroundExit, FatalError> {
    let tmp_root = std::env::temp_dir().join("residuum-setup");
    if tmp_root.exists() {
        std::fs::remove_dir_all(&tmp_root).map_err(|e| {
            FatalError::Config(format!(
                "failed to clean setup directory {}: {e}",
                tmp_root.display()
            ))
        })?;
    }
    let hub_dir = residuum::config::paths::hub_dir(&tmp_root);
    HubConfig::bootstrap_at(&hub_dir)?;
    println!(
        "setup mode: config will be written under {}",
        tmp_root.display()
    );
    match residuum::wiring::run_hub(&tmp_root).await? {
        residuum::hub::HubExit::Shutdown => {}
        residuum::hub::HubExit::Restart => {
            tracing::warn!(
                "restart requested in setup mode; exiting instead. Start residuum normally to run the updated binary"
            );
        }
    }
    Ok(ForegroundExit::Done)
}

#[tracing::instrument(skip_all)]
async fn run_serve_foreground_inner(args: &ServeArgs) -> Result<ForegroundExit, FatalError> {
    if args.setup {
        return Box::pin(run_setup_mode()).await;
    }

    let residuum_root = residuum::config::residuum_root()?;
    let hub_dir = residuum::config::paths::hub_dir(&residuum_root);
    HubConfig::bootstrap_at(&hub_dir)?;

    if residuum::config::discover_agents(&residuum_root)?.is_empty() {
        tracing::info!("no agent found, serving the setup wizard");
    }

    ensure_hub_config_loads_or_has_fallback(&residuum_root, &hub_dir)?;

    match residuum::wiring::run_hub(&residuum_root).await? {
        residuum::hub::HubExit::Restart => Ok(ForegroundExit::Restart),
        residuum::hub::HubExit::Shutdown => Ok(ForegroundExit::Done),
    }
}

/// The command that relaunches this process from its install path with the
/// same arguments, so `serve --foreground` is preserved.
///
/// The install path, not `current_exe`: after an update renamed the running
/// binary aside, Linux reports the renamed old file as `current_exe`.
fn relaunch_command() -> Result<std::process::Command, FatalError> {
    let exe = residuum::hub::update::install_path().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for restart: {e}"
        ))
    })?;
    tracing::info!(exe = %exe.display(), "restarting with updated binary");

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args().skip(1));
    Ok(cmd)
}

fn relaunch(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
) -> Result<(), FatalError> {
    let config_dir = pid_path.parent().map_or_else(
        || std::path::PathBuf::from("."),
        std::path::Path::to_path_buf,
    );

    residuum::daemon::remove_ready_file(&config_dir);

    if let Some(pending) = residuum::hub::update::take_pending_rollback(&config_dir) {
        return relaunch_with_watchdog(pid_lock, pid_path, &config_dir, &pending);
    }
    relaunch_plain(pid_lock, pid_path)
}

/// Hand a restart off to the update-rollback watchdog, run from the
/// just-preserved previous binary — proven to execute, unlike the new one —
/// so it can supervise the new version's startup and roll back if it never
/// becomes healthy. See `commands::update_watchdog`.
///
/// On Unix, replaces the current process image via `exec()` so the main PID
/// stays alive across the handoff and service managers (`systemd`, Docker,
/// `supervisord`) do not treat the handoff as a service exit and kill the
/// cgroup.
#[cfg(unix)]
fn relaunch_with_watchdog(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
    config_dir: &std::path::Path,
    pending: &residuum::hub::update::PendingRollback,
) -> Result<(), FatalError> {
    let watchdog_args = watchdog_args(pending, std::env::args().skip(1));
    tracing::info!(
        pid = std::process::id(),
        gateway_exe = %pending.gateway_binary.display(),
        target_version = %pending.target_version,
        "handing restart off to the update-rollback watchdog in-place"
    );

    remove_pid_file(pid_path);
    drop(pid_lock);

    let err = residuum::daemon::exec_gateway_process(
        &pending.previous_binary(),
        &watchdog_args,
        config_dir,
    );
    Err(FatalError::Gateway(format!(
        "failed to exec update-rollback watchdog: {err}"
    )))
}

#[cfg(windows)]
fn relaunch_with_watchdog(
    pid_lock: residuum::daemon::PidFileLock,
    pid_path: &std::path::Path,
    config_dir: &std::path::Path,
    pending: &residuum::hub::update::PendingRollback,
) -> Result<(), FatalError> {
    remove_pid_file(pid_path);
    drop(pid_lock);

    let watchdog_args = watchdog_args(pending, std::env::args().skip(1));
    let child = residuum::daemon::spawn_gateway_process(
        &pending.previous_binary(),
        &watchdog_args,
        config_dir,
    )
    .map_err(|e| FatalError::Gateway(format!("failed to start update-rollback watchdog: {e}")))?;

    tracing::info!(
        pid = child.id(),
        gateway_exe = %pending.gateway_binary.display(),
        target_version = %pending.target_version,
        "handed restart off to the update-rollback watchdog"
    );
    Ok(())
}

fn watchdog_args(
    pending: &residuum::hub::update::PendingRollback,
    serve_args: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut args = vec![
        "update-watchdog".to_string(),
        "--gateway-exe".to_string(),
        pending.gateway_binary.display().to_string(),
        "--prev-exe".to_string(),
        pending.previous_binary().display().to_string(),
        "--previous-version".to_string(),
        pending.previous_version.clone(),
        "--target-version".to_string(),
        pending.target_version.clone(),
        "--".to_string(),
    ];
    args.extend(serve_args);
    args
}

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

    fn flag_value<'a>(args: &'a [String], flag: &str) -> &'a str {
        args.iter().skip_while(|arg| *arg != flag).nth(1).unwrap()
    }

    #[test]
    fn the_watchdog_starts_the_install_path_and_keeps_the_preserved_binary_apart() {
        let install = std::env::temp_dir().join("bin").join("residuum");
        let pending = residuum::hub::update::PendingRollback {
            gateway_binary: install.clone(),
            previous_version: "v2026.10.10".to_string(),
            target_version: "v2026.10.12".to_string(),
        };
        let args = watchdog_args(&pending, ["serve".to_string(), "--foreground".to_string()]);

        assert_eq!(args.first().map(String::as_str), Some("update-watchdog"));
        assert_eq!(
            flag_value(&args, "--gateway-exe"),
            install.display().to_string()
        );
        assert_eq!(
            flag_value(&args, "--prev-exe"),
            residuum::hub::update::previous_binary_path(&install)
                .display()
                .to_string()
        );
        assert_ne!(
            flag_value(&args, "--gateway-exe"),
            flag_value(&args, "--prev-exe"),
            "the watchdog deletes --prev-exe once --gateway-exe is healthy, so they must differ"
        );
        assert_eq!(flag_value(&args, "--target-version"), "v2026.10.12");
        assert!(
            args.ends_with(&["--", "serve", "--foreground"].map(String::from)),
            "the gateway's own arguments follow the separator: {args:?}"
        );
    }
}

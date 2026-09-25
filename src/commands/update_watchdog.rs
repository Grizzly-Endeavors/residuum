//! Update-rollback watchdog: supervises a self-update restart and rolls
//! back to the previous binary if the new one never becomes healthy.
//!
//! `commands::serve::foreground::relaunch` always spawns this from the
//! *previous*, known-good binary rather than the newly-installed one — so a
//! new binary that can't even execute (wrong format, missing, corrupt)
//! still gets rolled back instead of losing the supervisor along with the
//! failed launch.

use std::path::PathBuf;
use std::time::Duration;

use residuum::util::FatalError;

/// Arguments `commands::serve::foreground::relaunch` passes when handing a
/// restart off to this watchdog.
#[derive(clap::Args)]
pub(super) struct UpdateWatchdogArgs {
    /// The canonical path the gateway runs from. Holds the newly-installed
    /// binary when the watchdog starts; holds the restored previous binary
    /// after a rollback.
    #[arg(long)]
    gateway_exe: PathBuf,
    /// Where the previous, known-good binary was preserved. Restored over
    /// `gateway_exe` on failure; deleted once the new version is confirmed
    /// healthy.
    #[arg(long)]
    prev_exe: PathBuf,
    #[arg(long)]
    previous_version: String,
    #[arg(long)]
    target_version: String,
    /// Arguments to launch the gateway with — the same ones the restart
    /// this watchdog took over from was using, e.g. `serve --foreground`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    serve_args: Vec<String>,
}

/// Run the watchdog: start the new version, wait up to
/// [`residuum::daemon::READINESS_TIMEOUT`] for it to report healthy, and
/// roll back to the previous version if it doesn't.
#[tracing::instrument(skip_all, fields(target_version = %args.target_version))]
pub(super) fn run_update_watchdog(args: &UpdateWatchdogArgs) -> Result<(), FatalError> {
    residuum::util::tracing_init::init_default_tracing();

    let config_dir = residuum::config::Config::config_dir()?;
    let ready_path = residuum::daemon::ready_file_path(&config_dir);

    tracing::info!(
        gateway_exe = %args.gateway_exe.display(),
        previous_version = %args.previous_version,
        timeout_secs = residuum::daemon::READINESS_TIMEOUT.as_secs(),
        "starting new version under supervision"
    );

    let outcome = start_and_wait(args, &config_dir, &ready_path);

    match outcome {
        Ok(residuum::daemon::ReadinessOutcome::Ready) => {
            tracing::info!("new version is healthy; removing the preserved previous binary");
            if let Err(e) = std::fs::remove_file(&args.prev_exe)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(
                    path = %args.prev_exe.display(),
                    error = %e,
                    "failed to remove the preserved previous binary after a healthy update"
                );
            }
            Ok(())
        }
        Ok(residuum::daemon::ReadinessOutcome::ProcessExited) => {
            let reason = residuum::daemon::read_startup_error(&config_dir)
                .unwrap_or_else(|| "the new version exited before starting up".to_string());
            roll_back(args, &config_dir, &reason)
        }
        Ok(residuum::daemon::ReadinessOutcome::TimedOut) => roll_back(
            args,
            &config_dir,
            &format!(
                "did not become healthy within {}s",
                residuum::daemon::READINESS_TIMEOUT.as_secs()
            ),
        ),
        Err(e) => roll_back(
            args,
            &config_dir,
            &format!("couldn't start the new version: {e}"),
        ),
    }
}

/// Spawn the gateway at `args.gateway_exe` and wait for it to become ready,
/// killing it first if it's still running but never became healthy.
fn start_and_wait(
    args: &UpdateWatchdogArgs,
    config_dir: &std::path::Path,
    ready_path: &std::path::Path,
) -> std::io::Result<residuum::daemon::ReadinessOutcome> {
    let mut child =
        residuum::daemon::spawn_gateway_process(&args.gateway_exe, &args.serve_args, config_dir)?;

    let outcome = residuum::daemon::wait_for_ready(
        ready_path,
        residuum::daemon::READINESS_TIMEOUT,
        Duration::from_millis(200),
        || matches!(child.try_wait(), Ok(None)),
    );

    if !matches!(outcome, residuum::daemon::ReadinessOutcome::Ready) {
        // A hung-but-not-yet-exited new process must not keep the pid lock
        // or the port held while we roll back and start the old one.
        if let Err(e) = child.kill() {
            tracing::debug!(error = %e, "failed to kill the unhealthy new gateway process (it may have already exited)");
        }
        if let Err(e) = child.wait() {
            tracing::debug!(error = %e, "failed to wait for the unhealthy new gateway process to exit");
        }
    }

    Ok(outcome)
}

/// Restore the previous binary over `gateway_exe`, record why, and start it
/// so the gateway comes back up on the version that was working before.
fn roll_back(
    args: &UpdateWatchdogArgs,
    config_dir: &std::path::Path,
    reason: &str,
) -> Result<(), FatalError> {
    tracing::warn!(
        reason,
        target_version = %args.target_version,
        previous_version = %args.previous_version,
        "rolling back to the previous version"
    );

    residuum::daemon::remove_ready_file(config_dir);
    residuum::update::write_rollback_notice(
        config_dir,
        &residuum::update::RollbackNotice {
            attempted_version: args.target_version.clone(),
            reason: reason.to_string(),
            at: chrono::Utc::now(),
        },
    );

    std::fs::rename(&args.prev_exe, &args.gateway_exe).map_err(|e| {
        FatalError::Gateway(format!(
            "rollback failed: couldn't restore the previous binary at {}: {e}",
            args.gateway_exe.display()
        ))
    })?;

    residuum::daemon::spawn_gateway_process(&args.gateway_exe, &args.serve_args, config_dir)
        .map_err(|e| {
            FatalError::Gateway(format!(
                "rollback restored the previous binary but couldn't start it: {e}"
            ))
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(gateway_exe: PathBuf, prev_exe: PathBuf) -> UpdateWatchdogArgs {
        UpdateWatchdogArgs {
            gateway_exe,
            prev_exe,
            previous_version: "v2026.09.01".to_string(),
            target_version: "v2026.09.24".to_string(),
            serve_args: vec!["serve".to_string(), "--foreground".to_string()],
        }
    }

    #[test]
    fn roll_back_restores_previous_binary_and_writes_notice() {
        let dir = tempfile::tempdir().unwrap();
        let gateway_exe = dir.path().join("residuum");
        let prev_exe = dir.path().join("residuum.prev");
        // A stub "previous binary": something that exists and is
        // executable so the restart-after-rollback spawn succeeds.
        std::fs::write(&prev_exe, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&prev_exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let a = args(gateway_exe.clone(), prev_exe.clone());
        roll_back(&a, dir.path(), "did not become healthy within 60s").unwrap();

        assert!(
            gateway_exe.exists(),
            "previous binary should be restored to the gateway path"
        );
        assert!(
            !prev_exe.exists(),
            "the preserved backup should be consumed by the restore"
        );

        let notice = residuum::update::read_rollback_notice(dir.path()).unwrap();
        assert_eq!(notice.attempted_version, "v2026.09.24");
        assert_eq!(notice.reason, "did not become healthy within 60s");
    }

    #[test]
    fn roll_back_fails_clearly_when_previous_binary_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let gateway_exe = dir.path().join("residuum");
        let prev_exe = dir.path().join("residuum.prev");
        let a = args(gateway_exe, prev_exe);

        let result = roll_back(&a, dir.path(), "couldn't start the new version");
        assert!(
            result.is_err(),
            "rollback with no preserved binary must fail visibly, not silently"
        );
    }
}

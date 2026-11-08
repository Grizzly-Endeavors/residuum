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

/// Outcome of [`start_and_wait`]: either the new process reached readiness and
/// is returned alive for supervision, or it failed before becoming healthy.
enum WatchdogStart {
    Ready(std::process::Child),
    ProcessExited,
    TimedOut,
}

/// Run the watchdog: start the new version, wait up to
/// [`residuum::daemon::READINESS_TIMEOUT`] for it to report healthy, and
/// roll back to the previous version if it doesn't.
///
/// On Unix, once the new (or rolled-back) gateway process is running, this
/// watchdog stays alive as its supervisor until the child exits — forwarding
/// `SIGTERM`/`SIGINT` to it — because `commands::serve::foreground::relaunch`
/// `exec()`s into this watchdog in-place. Staying alive keeps the original
/// main PID running for the lifetime of the service so `systemd` (`Type=simple`,
/// `KillMode=control-group`) and other process supervisors never treat the
/// update handoff as a service exit.
#[tracing::instrument(skip_all, fields(target_version = %args.target_version))]
pub(super) async fn run_update_watchdog(args: &UpdateWatchdogArgs) -> Result<(), FatalError> {
    residuum::logging::init_default_tracing();

    let config_dir = residuum::config::default_hub_dir()?;
    let ready_path = residuum::daemon::ready_file_path(&config_dir);

    tracing::info!(
        gateway_exe = %args.gateway_exe.display(),
        previous_version = %args.previous_version,
        timeout_secs = residuum::daemon::READINESS_TIMEOUT.as_secs(),
        "starting new version under supervision"
    );

    let outcome = start_and_wait(args, &config_dir, &ready_path);

    let active_child = match outcome {
        Ok(WatchdogStart::Ready(child)) => {
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
            child
        }
        Ok(WatchdogStart::ProcessExited) => {
            let reason = residuum::daemon::read_startup_error(&config_dir)
                .unwrap_or_else(|| "the new version exited before starting up".to_string());
            roll_back(args, &config_dir, &reason)?
        }
        Ok(WatchdogStart::TimedOut) => roll_back(
            args,
            &config_dir,
            &format!(
                "did not become healthy within {}s",
                residuum::daemon::READINESS_TIMEOUT.as_secs()
            ),
        )?,
        Err(e) => roll_back(
            args,
            &config_dir,
            &format!("couldn't start the new version: {e}"),
        )?,
    };

    supervise_until_exit(active_child).await
}

/// Spawn the gateway at `args.gateway_exe` and wait for it to become ready,
/// killing it first if it's still running but never became healthy.
fn start_and_wait(
    args: &UpdateWatchdogArgs,
    config_dir: &std::path::Path,
    ready_path: &std::path::Path,
) -> std::io::Result<WatchdogStart> {
    let mut child =
        residuum::daemon::spawn_gateway_process(&args.gateway_exe, &args.serve_args, config_dir)?;

    let outcome = residuum::daemon::wait_for_ready(
        ready_path,
        residuum::daemon::READINESS_TIMEOUT,
        Duration::from_millis(200),
        || matches!(child.try_wait(), Ok(None)),
    );

    match outcome {
        residuum::daemon::ReadinessOutcome::Ready => Ok(WatchdogStart::Ready(child)),
        residuum::daemon::ReadinessOutcome::ProcessExited => {
            reap_unhealthy_child(&mut child);
            Ok(WatchdogStart::ProcessExited)
        }
        residuum::daemon::ReadinessOutcome::TimedOut => {
            reap_unhealthy_child(&mut child);
            Ok(WatchdogStart::TimedOut)
        }
    }
}

/// Kill and reap an unhealthy gateway child so it cannot keep the PID lock or
/// HTTP port held while `roll_back` starts the previous binary.
fn reap_unhealthy_child(child: &mut std::process::Child) {
    if let Err(e) = child.kill() {
        tracing::debug!(error = %e, "failed to kill the unhealthy new gateway process (it may have already exited)");
    }
    if let Err(e) = child.wait() {
        tracing::debug!(error = %e, "failed to wait for the unhealthy new gateway process to exit");
    }
}

/// Wait for `child` to exit on Unix, forwarding `SIGTERM` and `SIGINT` to it
/// so stopping the service unit gracefully stops the child gateway before this
/// supervisor process exits.
#[cfg(unix)]
async fn supervise_until_exit(mut child: std::process::Child) -> Result<(), FatalError> {
    use tokio::signal::unix::{SignalKind, signal};

    let child_pid = child.id();
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| FatalError::Gateway(format!("failed to register SIGTERM handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| FatalError::Gateway(format!("failed to register SIGINT handler: {e}")))?;

    let mut wait_task = tokio::task::spawn_blocking(move || child.wait());

    let status = loop {
        tokio::select! {
            res = &mut wait_task => {
                let status = res
                    .map_err(|e| FatalError::Gateway(format!("watchdog wait task panicked: {e}")))?
                    .map_err(|e| FatalError::Gateway(format!("failed waiting for gateway process (pid {child_pid}): {e}")))?;
                break status;
            }
            _ = sigterm.recv() => {
                tracing::info!(child_pid, "forwarding SIGTERM to supervised gateway process");
                if let Err(e) = residuum::daemon::send_sigterm(child_pid) {
                    tracing::debug!(child_pid, error = %e, "failed to forward SIGTERM to gateway child");
                }
            }
            _ = sigint.recv() => {
                tracing::info!(child_pid, "forwarding SIGINT as SIGTERM to supervised gateway process");
                if let Err(e) = residuum::daemon::send_sigterm(child_pid) {
                    tracing::debug!(child_pid, error = %e, "failed to forward SIGTERM to gateway child");
                }
            }
        }
    };

    if status.success() {
        Ok(())
    } else {
        Err(FatalError::Gateway(format!(
            "supervised gateway process (pid {child_pid}) exited with {status}"
        )))
    }
}

/// On Windows, the watchdog was spawned as a separate process rather than
/// `exec()`d in place, so once the new or restored gateway is up there is
/// no parent PID contract to hold open.
#[cfg(windows)]
#[expect(
    clippy::unused_async,
    reason = "matches the async Unix signature called from run_update_watchdog"
)]
async fn supervise_until_exit(_child: std::process::Child) -> Result<(), FatalError> {
    Ok(())
}

/// Restore the previous binary over `gateway_exe`, record why, and start it
/// so the gateway comes back up on the version that was working before.
fn roll_back(
    args: &UpdateWatchdogArgs,
    config_dir: &std::path::Path,
    reason: &str,
) -> Result<std::process::Child, FatalError> {
    tracing::warn!(
        reason,
        target_version = %args.target_version,
        previous_version = %args.previous_version,
        "rolling back to the previous version"
    );

    residuum::daemon::remove_ready_file(config_dir);
    residuum::hub::update::write_rollback_notice(
        config_dir,
        &residuum::hub::update::RollbackNotice {
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
        })
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
        // A stub "previous binary": something that exists and is
        // executable so the restart-after-rollback spawn succeeds. On Unix
        // it is a link to the shell, run as `sh -c "exit 0"`, not a script
        // written here: executing a file this process just wrote fails with
        // "Text file busy" when a test on another thread forks while the file
        // is still open for writing, because the forked child holds a copy of
        // the descriptor until it execs. Windows won't run a script as a
        // binary, so it gets a copy of a real one.
        #[cfg(unix)]
        let (gateway_exe, prev_exe, serve_args) = {
            let prev_exe = dir.path().join("residuum.prev");
            std::os::unix::fs::symlink("/bin/sh", &prev_exe).unwrap();
            (
                dir.path().join("residuum"),
                prev_exe,
                vec!["-c".to_string(), "exit 0".to_string()],
            )
        };
        #[cfg(windows)]
        let (gateway_exe, prev_exe, serve_args) = {
            let prev_exe = dir.path().join("residuum.exe.prev");
            let system_root = std::env::var("SystemRoot").unwrap();
            std::fs::copy(
                std::path::Path::new(&system_root)
                    .join("System32")
                    .join("whoami.exe"),
                &prev_exe,
            )
            .unwrap();
            (
                dir.path().join("residuum.exe"),
                prev_exe,
                vec!["serve".to_string(), "--foreground".to_string()],
            )
        };

        let a = UpdateWatchdogArgs {
            serve_args,
            ..args(gateway_exe.clone(), prev_exe.clone())
        };
        let mut child = roll_back(&a, dir.path(), "did not become healthy within 60s").unwrap();
        child.wait().unwrap();

        assert!(
            gateway_exe.exists(),
            "previous binary should be restored to the gateway path"
        );
        assert!(
            !prev_exe.exists(),
            "the preserved backup should be consumed by the restore"
        );

        let notice = residuum::hub::update::read_rollback_notice(dir.path()).unwrap();
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

    #[cfg(unix)]
    #[tokio::test]
    async fn supervise_until_exit_propagates_child_exit_status() {
        let ok_child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        assert!(supervise_until_exit(ok_child).await.is_ok());

        let failing_child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 42"])
            .spawn()
            .unwrap();
        let err = supervise_until_exit(failing_child).await.unwrap_err();
        assert!(
            err.to_string().contains("42"),
            "error should report the child's exit status: {err}"
        );
    }
}

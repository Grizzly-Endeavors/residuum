//! Foreground process lifecycle and setup wizard for the gateway.

use residuum::config::Config;
use residuum::util::FatalError;

use super::ServeArgs;

/// Run the gateway in foreground mode (called as `residuum serve --foreground`).
///
/// This is the current behavior — runs the gateway event loop directly.
/// Used by the daemon spawner as the child process, or for debugging.
///
/// # Errors
///
/// Returns `FatalError` if initialization or the gateway loop fails.
#[tracing::instrument(skip_all, fields(agent = ?args.agent))]
pub(crate) async fn run_serve_foreground(args: &ServeArgs) -> Result<(), FatalError> {
    let agent_name = args.agent.as_deref();
    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;

    // Acquire exclusive lock on the PID file. This both:
    // 1. Prevents two instances from running simultaneously
    // 2. Makes stale PID files detectable (lock released on process death)
    let _pid_lock = residuum::daemon::acquire_pid_lock(&pid_path)?;

    let result = run_serve_foreground_inner(args).await;

    // Clean up PID file on normal exit. On crash/SIGKILL the lock is
    // released by the OS, and the next startup detects the stale file.
    if let Err(e) = residuum::daemon::remove_pid_file(&pid_path) {
        tracing::warn!(error = %e, "failed to remove pid file on exit");
    }

    result
}

/// Run the onboarding wizard in an isolated temp directory, then boot gateway.
#[tracing::instrument(skip_all)]
async fn run_setup_mode() -> Result<(), FatalError> {
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
        residuum::gateway::setup::SetupExit::Shutdown => return Ok(()),
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
    let _ = Box::pin(residuum::gateway::run_gateway(cfg)).await?;
    Ok(())
}

/// Inner implementation of foreground serve, wrapped by PID file lifecycle.
#[tracing::instrument(skip_all, fields(agent = ?args.agent))]
async fn run_serve_foreground_inner(args: &ServeArgs) -> Result<(), FatalError> {
    // Clean up leftover .exe.old from a previous Windows self-update (no-op on Unix)
    residuum::update::cleanup_old_binary();

    let agent_name = args.agent.as_deref();

    if args.setup {
        // Box::pin reduces stack frame size — this future is large
        return Box::pin(run_setup_mode()).await;
    }

    let config_dir = residuum::agent_registry::paths::resolve_config_dir(agent_name)?;
    // Determine first-boot from disk state: if a backup exists, the gateway
    // has previously loaded a valid config, so this is a restart.
    let is_first_boot = !config_dir.join("config.toml.bak").exists();

    loop {
        Config::bootstrap_at_dir(&config_dir)?;
        match Config::load_at(&config_dir) {
            Ok(mut cfg) => {
                cfg.config_dir.clone_from(&config_dir);
                tracing::info!(
                    agent = agent_name.unwrap_or("(default)"),
                    model = cfg.main.first().map_or("(none)", |s| s.model.model.as_str()),
                    provider_url = cfg.main.first().map_or("(none)", |s| s.provider_url.as_str()),
                    workspace = %cfg.workspace_dir.display(),
                    "configuration loaded"
                );
                // Gateway handles reloads in-place and only returns on shutdown
                // or fatal error. Backup is created inside run_gateway().
                // Box::pin reduces stack frame size — this future is large
                match Box::pin(residuum::gateway::run_gateway(cfg)).await? {
                    residuum::gateway::GatewayExit::Restart => return re_exec_serve_foreground(),
                    residuum::gateway::GatewayExit::Shutdown => {}
                }
                break;
            }
            Err(err) if !is_first_boot => {
                // Config broken on restart — try restoring from backup
                tracing::warn!(error = %err, "config invalid, attempting rollback from backup");
                if residuum::gateway::rollback_config(&config_dir) {
                    if let Err(retry_err) = Config::load_at(&config_dir) {
                        return Err(FatalError::Config(format!(
                            "config invalid after rollback: {retry_err}\n\n\
                             fix {}/config.toml and providers.toml manually, then restart",
                            config_dir.display()
                        )));
                    }
                    tracing::info!("config restored from backup, starting gateway");
                    continue;
                }
                tracing::warn!("config rollback failed: no backup available");
                return Err(FatalError::Config(format!(
                    "config invalid and rollback failed: {err}\n\n\
                     fix {}/config.toml and providers.toml manually, then restart",
                    config_dir.display()
                )));
            }
            Err(err) => {
                if let Some(name) = agent_name {
                    // Named agents don't get setup wizard — config must be ready
                    return Err(FatalError::Config(format!(
                        "config invalid for agent '{name}': {err}\n\n\
                         edit {}/config.toml manually or recreate the agent",
                        config_dir.display()
                    )));
                }
                // First boot — setup wizard (default agent only)
                tracing::warn!(error = %err, "config invalid, starting setup wizard");
                // Box::pin reduces stack frame size — this future is large
                match Box::pin(residuum::gateway::setup::run_setup_server()).await? {
                    residuum::gateway::setup::SetupExit::ConfigSaved => {
                        tracing::debug!("setup complete, loading configuration");
                    }
                    residuum::gateway::setup::SetupExit::Shutdown => break,
                }
            }
        }
    }
    Ok(())
}

/// Re-exec the current binary with `serve --foreground` args.
///
/// Uses `exec()` to replace the process image with the (potentially updated)
/// binary on disk. The PID stays the same, so the daemon parent doesn't notice.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the current executable path cannot be
/// determined. On Unix, `exec()` does not return on success.
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

/// Re-exec the serve foreground process with the (potentially updated) binary.
///
/// On Unix, uses `exec()` to replace the process image (PID stays the same).
/// On Windows, spawns a new process and exits the current one.
#[cfg(unix)]
fn re_exec_serve_foreground() -> Result<(), FatalError> {
    use std::os::unix::process::CommandExt;

    let raw_exe = std::env::current_exe().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for re-exec: {e}"
        ))
    })?;

    let exe = resolve_exe_path(&raw_exe);

    tracing::info!(exe = %exe.display(), "re-execing with updated binary");

    // Forward the original args so --foreground and --agent are preserved across re-exec
    let original_args: Vec<String> = std::env::args().skip(1).collect();
    let err = std::process::Command::new(&exe).args(&original_args).exec();

    // exec() only returns on error
    Err(FatalError::Gateway(format!("re-exec failed: {err}")))
}

/// Re-exec the serve foreground process with the (potentially updated) binary.
///
/// On Unix, uses `exec()` to replace the process image (PID stays the same).
/// On Windows, spawns a new process and exits the current one.
#[cfg(windows)]
fn re_exec_serve_foreground() -> Result<(), FatalError> {
    let raw_exe = std::env::current_exe().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for re-exec: {e}"
        ))
    })?;

    let exe = resolve_exe_path(&raw_exe);

    tracing::info!(exe = %exe.display(), "spawning updated binary and exiting");

    let original_args: Vec<String> = std::env::args().skip(1).collect();
    std::process::Command::new(&exe)
        .args(&original_args)
        .spawn()
        .map_err(|e| FatalError::Gateway(format!("failed to spawn updated binary: {e}")))?;

    // New process will acquire its own PID lock; exit this one.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

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

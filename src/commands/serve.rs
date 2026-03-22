//! Serve and daemonize subcommands.

use residuum::config::Config;
use residuum::util::FatalError;

/// Run the gateway in foreground mode (called as `residuum serve --foreground`).
///
/// This is the current behavior — runs the gateway event loop directly.
/// Used by the daemon spawner as the child process, or for debugging.
///
/// # Errors
///
/// Returns `FatalError` if initialization or the gateway loop fails.
pub(super) async fn run_serve_foreground(
    args: &[String],
    agent_name: Option<&str>,
) -> Result<(), FatalError> {
    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;

    // Acquire exclusive lock on the PID file. This both:
    // 1. Prevents two instances from running simultaneously
    // 2. Makes stale PID files detectable (lock released on process death)
    let _pid_lock = residuum::daemon::acquire_pid_lock(&pid_path)?;

    let result = run_serve_foreground_inner(args, agent_name).await;

    // Clean up PID file on normal exit. On crash/SIGKILL the lock is
    // released by the OS, and the next startup detects the stale file.
    if let Err(e) = residuum::daemon::remove_pid_file(&pid_path) {
        tracing::warn!(error = %e, "failed to remove pid file on exit");
    }

    result
}

/// Run the onboarding wizard in an isolated temp directory, then boot gateway.
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
            tracing::info!("setup complete, loading config from temp directory");
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
    let _ = residuum::gateway::run_gateway(cfg).await?;
    Ok(())
}

/// Inner implementation of foreground serve, wrapped by PID file lifecycle.
async fn run_serve_foreground_inner(
    args: &[String],
    agent_name: Option<&str>,
) -> Result<(), FatalError> {
    if args.iter().any(|a| a == "--setup") {
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
                        tracing::info!("setup complete, loading configuration");
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
fn re_exec_serve_foreground() -> Result<(), FatalError> {
    use std::os::unix::process::CommandExt;

    let raw_exe = std::env::current_exe().map_err(|e| {
        FatalError::Gateway(format!(
            "failed to determine current executable for re-exec: {e}"
        ))
    })?;

    // On Linux, atomically replacing the binary (via `mv`) unlinks the old
    // inode while the process is still running. The kernel then appends
    // " (deleted)" to /proc/self/exe, so current_exe() returns a path that
    // no longer exists. Strip the suffix to get the live path on disk, which
    // now points to the freshly-installed binary.
    let exe = {
        let s = raw_exe.to_string_lossy();
        if let Some(stripped) = s.strip_suffix(" (deleted)") {
            std::path::PathBuf::from(stripped)
        } else {
            raw_exe
        }
    };

    tracing::info!(exe = %exe.display(), "re-execing with updated binary");

    // Forward the original args so --debug is preserved across re-exec
    let original_args: Vec<String> = std::env::args().skip(1).collect();
    let err = std::process::Command::new(&exe).args(&original_args).exec();

    // exec() only returns on error
    Err(FatalError::Gateway(format!("re-exec failed: {err}")))
}

/// Spawn the gateway as a background daemon process.
///
/// Launches `residuum serve --foreground` as a detached child, polls for the
/// PID file to confirm startup, then exits. Prints a first-launch welcome
/// message if no config exists yet.
///
/// # Errors
///
/// Returns `FatalError` if the child process cannot be spawned or
/// startup times out.
pub(super) fn run_daemonize(args: &[String], agent_name: Option<&str>) -> Result<(), FatalError> {
    use residuum::config::GatewayConfig;
    use residuum::daemon::{is_process_running, read_pid_file};

    residuum::util::tracing_init::init_default_tracing();

    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    let label = agent_name.map_or("gateway".to_string(), |n| format!("agent '{n}'"));

    // Check for an already-running instance via file lock (primary detection)
    if residuum::daemon::is_pid_locked(&pid_path)? {
        let pid_msg = residuum::daemon::read_pid_file(&pid_path)
            .map_or_else(|_| String::new(), |pid| format!(" (pid {pid})"));
        println!("residuum: {label} is already running{pid_msg}");
        return Ok(());
    }

    // Clean up stale PID file if lock was not held
    if pid_path.exists()
        && let Err(e) = residuum::daemon::remove_pid_file(&pid_path)
    {
        tracing::warn!(error = %e, "failed to clean stale pid file");
    }

    // Resolve gateway address from config or defaults
    let config_dir = residuum::agent_registry::paths::resolve_config_dir(agent_name)?;
    let gateway_addr = Config::load_at(&config_dir).map_or_else(
        |_| GatewayConfig::default().addr(),
        |cfg| cfg.gateway.addr(),
    );

    // Detect whether the child will enter setup mode (no PID file until setup completes)
    // Named agents never enter setup mode.
    let needs_setup = agent_name.is_none()
        && (args.iter().any(|a| a == "--setup") || !config_dir.join("config.toml").exists());

    // First-launch welcome (or --setup which mimics it)
    if needs_setup {
        println!("welcome to residuum!");
        println!();
        println!("  it looks like this is your first time running residuum.");
        println!("  configure your agent at: http://{gateway_addr}");
        println!("  or run: residuum setup");
        println!();
    }

    // Build child args: forward any extra flags (like --setup) plus --foreground
    let exe = std::env::current_exe()
        .map_err(|e| FatalError::Gateway(format!("failed to determine current executable: {e}")))?;

    let mut child_args = vec!["serve".to_string(), "--foreground".to_string()];
    // Forward flags from the original invocation (skip argv[0] and "serve")
    let skip = if args.get(1).is_some_and(|a| a == "serve") {
        2
    } else {
        1
    };
    for arg in args.iter().skip(skip) {
        if arg != "--foreground" {
            child_args.push(arg.clone());
        }
    }

    let mut child = std::process::Command::new(&exe)
        .args(&child_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| FatalError::Gateway(format!("failed to spawn daemon process: {e}")))?;

    // When setup is needed, the setup wizard runs before the gateway and
    // no PID file is written until setup completes. Just verify the child
    // is alive and direct the user to the web UI.
    if needs_setup {
        // Brief pause to catch immediate crashes
        std::thread::sleep(std::time::Duration::from_millis(500));
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(FatalError::Gateway(format!(
                    "daemon exited immediately with {status}"
                )));
            }
            Ok(None) => {
                println!("residuum: setup server starting at http://{gateway_addr}");
                return Ok(());
            }
            Err(e) => {
                return Err(FatalError::Gateway(format!(
                    "failed to check daemon status: {e}"
                )));
            }
        }
    }

    // Poll for PID file to confirm startup (100ms intervals, 10s timeout)
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    let poll_interval = std::time::Duration::from_millis(100);

    loop {
        if start.elapsed() > timeout {
            if let Ok(Some(status)) = child.try_wait() {
                println!("residuum: {label} crashed during startup (exit status: {status})");
                println!("  check logs: residuum logs");
                println!("  note: if no log files exist, the daemon crashed before writing any");
            } else {
                println!("residuum: {label} did not start within 10 seconds");
                println!("  check logs: residuum logs");
            }
            return Err(FatalError::Gateway("daemon startup timed out".to_string()));
        }

        if let Ok(pid) = read_pid_file(&pid_path)
            && is_process_running(pid)
        {
            println!("residuum: {label} started at http://{gateway_addr} (pid {pid})");
            return Ok(());
        }

        std::thread::sleep(poll_interval);
    }
}

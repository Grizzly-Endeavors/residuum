//! Daemon spawning for the gateway.

use residuum::util::FatalError;

use super::ServeArgs;

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
pub(crate) fn run_serve_command(args: &ServeArgs) -> Result<(), FatalError> {
    use residuum::daemon::{is_process_running, read_pid_file};

    residuum::util::tracing_init::init_default_tracing();

    let agent_name = args.agent.as_deref();
    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    let label = super::super::agent_label(agent_name);

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
    let gateway_addr = super::super::resolve_gateway_addr(&config_dir);

    // Detect whether the child will enter setup mode (no PID file until setup completes)
    // Named agents never enter setup mode.
    let needs_setup =
        agent_name.is_none() && (args.setup || !config_dir.join("config.toml").exists());

    // First-launch welcome (or --setup which mimics it)
    if needs_setup {
        println!("welcome to residuum!");
        println!();
        println!("  it looks like this is your first time running residuum.");
        println!("  configure your agent at: http://{gateway_addr}");
        println!("  or run: residuum setup");
        println!();
    }

    // Build child args: forward original args plus --foreground.
    // We use std::env::args() rather than reconstructing from the parsed
    // struct to preserve flag formatting across the process boundary.
    let exe = std::env::current_exe()
        .map_err(|e| FatalError::Gateway(format!("failed to determine current executable: {e}")))?;

    let mut child_args = vec!["serve".to_string(), "--foreground".to_string()];
    let raw_args: Vec<String> = std::env::args().collect();
    // Skip argv[0] and "serve" (if present)
    let skip = if raw_args.get(1).is_some_and(|a| a == "serve") {
        2
    } else {
        1
    };
    for arg in raw_args.iter().skip(skip) {
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

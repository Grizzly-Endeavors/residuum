//! Stop subcommand: gracefully shut down a running gateway daemon.

use residuum::config::Config;
use residuum::util::FatalError;

/// Stop a running gateway daemon.
///
/// Uses a layered approach:
/// 1. Check file lock on PID file — if unlocked, process is dead (clean up stale file)
/// 2. Send HTTP shutdown request to the gateway API
/// 3. Fall back to SIGTERM if HTTP fails
///
/// # Errors
///
/// Returns `FatalError` if the process cannot be stopped.
pub(super) async fn run_stop_command(agent_name: Option<&str>) -> Result<(), FatalError> {
    use residuum::daemon::{is_pid_locked, read_pid_file, remove_pid_file, send_sigterm};

    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    let label = agent_name.map_or("gateway".to_string(), |n| format!("agent '{n}'"));

    // Layer 1: File lock check
    if !pid_path.exists() {
        println!("residuum: no {label} running (no pid file)");
        return Ok(());
    }

    if !is_pid_locked(&pid_path)? {
        let pid_msg = read_pid_file(&pid_path)
            .map_or_else(|_| String::new(), |pid| format!(" for pid {pid}"));
        println!("residuum: no {label} running (stale pid file{pid_msg})");
        remove_pid_file(&pid_path)?;
        return Ok(());
    }

    let pid = read_pid_file(&pid_path)?;

    // Layer 2: HTTP graceful shutdown
    let config_dir = residuum::agent_registry::paths::resolve_config_dir(agent_name)?;
    let gateway_addr = Config::load_at(&config_dir).map_or_else(
        |_| residuum::config::GatewayConfig::default().addr(),
        |cfg| cfg.gateway.addr(),
    );

    let http_ok = try_http_shutdown(&gateway_addr).await;

    if http_ok && poll_for_exit(&pid_path, pid, &label).await? {
        return Ok(());
    }

    if http_ok {
        tracing::warn!("HTTP shutdown accepted but process did not exit, falling back to SIGTERM");
    }

    // Layer 3: SIGTERM fallback (Unix-only)
    // TODO(windows): use TerminateProcess on Windows
    send_sigterm(pid)?;

    if poll_for_exit(&pid_path, pid, &label).await? {
        return Ok(());
    }

    println!("residuum: {label} (pid {pid}) did not stop within 5 seconds");
    Err(FatalError::Gateway(format!(
        "{label} pid {pid} did not exit after SIGTERM"
    )))
}

/// Attempt to shut down the gateway via its HTTP API.
///
/// Returns `true` if the server accepted the shutdown request.
async fn try_http_shutdown(gateway_addr: &str) -> bool {
    let url = format!("http://{gateway_addr}/api/shutdown");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build();

    let Ok(client) = client else {
        tracing::debug!("failed to build HTTP client for shutdown request");
        return false;
    };

    match client.post(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!("HTTP shutdown request accepted");
            true
        }
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "HTTP shutdown request rejected");
            false
        }
        Err(e) => {
            tracing::debug!(error = %e, "HTTP shutdown request failed");
            false
        }
    }
}

/// Poll for the process to exit after a shutdown signal.
///
/// Checks both the file lock and process status. Returns `true` if the
/// process exited within 5 seconds.
///
/// # Errors
///
/// Returns `FatalError` if file operations fail.
async fn poll_for_exit(
    pid_path: &std::path::Path,
    pid: u32,
    label: &str,
) -> Result<bool, FatalError> {
    use residuum::daemon::{is_pid_locked, is_process_running, remove_pid_file};

    let timeout = std::time::Duration::from_secs(5);
    let poll_interval = std::time::Duration::from_millis(200);
    let start = std::time::Instant::now();

    loop {
        let lock_held = is_pid_locked(pid_path)?;
        let process_alive = is_process_running(pid);

        if !lock_held || !process_alive {
            remove_pid_file(pid_path)?;
            println!("residuum: {label} stopped (pid {pid})");
            return Ok(true);
        }

        if start.elapsed() > timeout {
            return Ok(false);
        }

        tokio::time::sleep(poll_interval).await;
    }
}

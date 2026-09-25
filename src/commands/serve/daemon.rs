//! Daemon spawning for the gateway.

use residuum::util::FatalError;

use super::ServeArgs;

/// Build the `residuum serve --foreground` arguments for the daemon child,
/// forwarding everything from the original invocation except a redundant
/// `serve`/`--foreground`.
fn build_child_args(raw: &[String]) -> Vec<String> {
    let mut child_args = vec!["serve".to_string(), "--foreground".to_string()];
    let skip = if raw.get(1).is_some_and(|a| a == "serve") {
        2
    } else {
        1
    };
    for arg in raw.iter().skip(skip) {
        if arg != "--foreground" {
            child_args.push(arg.clone());
        }
    }
    child_args
}

/// Spawn the gateway as a background daemon process.
///
/// Launches `residuum serve --foreground` as a detached child and waits for
/// it to report itself healthy (providers, workspace, and its HTTP listener
/// all ready — see `gateway::event_loop::run_loop::run_gateway`) before
/// reporting success, so a later init failure is caught here instead of
/// being reported as "started" and then exiting silently. Prints a
/// first-launch welcome message if no config exists yet.
///
/// # Errors
///
/// Returns `FatalError` if the child process cannot be spawned, exits
/// before becoming healthy, or does not become healthy within the timeout.
#[tracing::instrument(skip_all)]
pub(crate) fn run_serve_command(args: &ServeArgs) -> Result<(), FatalError> {
    use residuum::daemon::read_pid_file;

    residuum::util::tracing_init::init_default_tracing();

    let pid_path = residuum::config::Config::config_dir()?.join("residuum.pid");
    let label = "gateway";

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
    let config_dir = residuum::config::Config::config_dir()?;
    let gateway_addr = super::super::resolve_gateway_addr(&config_dir);

    // Detect whether the child will enter setup mode (no PID file until setup completes)
    let needs_setup = args.setup || !config_dir.join("config.toml").exists();

    // Catch an invalid config here, where the user can see the error. The
    // child takes its PID lock before loading config, so the startup poll
    // below would report success before the child exits. A live config
    // that fails to load is only blocked here when there's no
    // last-known-good copy either — the child falls back to one and keeps
    // running, same as `run_serve_foreground_inner`'s check.
    if !needs_setup
        && let Err(err) = residuum::config::Config::load_at(&config_dir)
        && !residuum::gateway::has_last_known_good(&config_dir)
        && let super::startup_config::ConfigProblem::Invalid(invalid) =
            super::startup_config::classify_load_error(&config_dir, &err)
    {
        return Err(invalid);
    }

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

    let raw_args: Vec<String> = std::env::args().collect();
    let child_args = build_child_args(&raw_args);

    // A previous attempt's readiness/error markers must not leak into this
    // one's poll below.
    residuum::daemon::remove_ready_file(&config_dir);
    residuum::daemon::clear_startup_error(&config_dir);

    let mut child = residuum::daemon::spawn_gateway_process(&exe, &child_args, &config_dir)
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

    // Wait for the daemon to report itself healthy: providers and workspace
    // initialized, and its HTTP listener bound (see
    // `gateway::event_loop::run_loop::run_gateway`). Reaching the PID lock
    // alone isn't enough — a later init failure would otherwise be reported
    // as "started" and then exit silently.
    let ready_path = residuum::daemon::ready_file_path(&config_dir);
    let outcome = residuum::daemon::wait_for_ready(
        &ready_path,
        residuum::daemon::READINESS_TIMEOUT,
        std::time::Duration::from_millis(100),
        || matches!(child.try_wait(), Ok(None)),
    );

    match outcome {
        residuum::daemon::ReadinessOutcome::Ready => {
            let pid_msg = read_pid_file(&pid_path)
                .map_or_else(|_| String::new(), |pid| format!(" (pid {pid})"));
            println!("residuum: {label} started at http://{gateway_addr}{pid_msg}");
            Ok(())
        }
        residuum::daemon::ReadinessOutcome::ProcessExited => {
            report_startup_failure(&config_dir, label, "crashed during startup");
            Err(FatalError::Gateway(format!(
                "{label} crashed during startup"
            )))
        }
        residuum::daemon::ReadinessOutcome::TimedOut => {
            report_startup_failure(
                &config_dir,
                label,
                &format!(
                    "did not become healthy within {}s",
                    residuum::daemon::READINESS_TIMEOUT.as_secs()
                ),
            );
            Err(FatalError::Gateway("daemon startup timed out".to_string()))
        }
    }
}

/// Print why startup failed, preferring the plain-language error the
/// process itself recorded over a generic message.
fn report_startup_failure(config_dir: &std::path::Path, label: &str, generic_reason: &str) {
    match residuum::daemon::read_startup_error(config_dir) {
        Some(reason) => println!("residuum: {label} failed to start: {reason}"),
        None => println!("residuum: {label} {generic_reason}"),
    }
    println!(
        "  check logs: residuum logs (stderr also at {})",
        residuum::daemon::stderr_log_path(config_dir).display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn build_child_args_with_serve_subcommand() {
        let result = build_child_args(&args(&["residuum", "serve", "--extra", "foo"]));
        assert_eq!(
            result,
            args(&["serve", "--foreground", "--extra", "foo"]),
            "should prepend serve --foreground and forward remaining args"
        );
    }

    #[test]
    fn build_child_args_without_serve_subcommand() {
        let result = build_child_args(&args(&["residuum", "--extra", "foo"]));
        assert_eq!(
            result,
            args(&["serve", "--foreground", "--extra", "foo"]),
            "should handle missing serve subcommand by skipping only argv[0]"
        );
    }

    #[test]
    fn build_child_args_deduplicates_foreground_flag() {
        let result = build_child_args(&args(&[
            "residuum",
            "serve",
            "--foreground",
            "--extra",
            "foo",
        ]));
        assert_eq!(
            result,
            args(&["serve", "--foreground", "--extra", "foo"]),
            "should not duplicate --foreground when already present"
        );
    }

    #[test]
    fn build_child_args_only_exe() {
        let result = build_child_args(&args(&["residuum"]));
        assert_eq!(
            result,
            args(&["serve", "--foreground"]),
            "should produce minimal args when only exe is present"
        );
    }
}

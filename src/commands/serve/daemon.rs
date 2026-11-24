use residuum::util::FatalError;

use super::ServeArgs;

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

#[tracing::instrument(skip_all)]
pub(crate) fn run_serve_command(args: &ServeArgs) -> Result<(), FatalError> {
    use residuum::daemon::read_pid_file;

    residuum::logging::init_default_tracing();

    let residuum_root = residuum::config::residuum_root()?;
    let hub_dir = residuum::config::paths::hub_dir(&residuum_root);
    let pid_path = residuum::config::HubPaths::new(&hub_dir).pid_file();
    let label = "gateway";

    if residuum::daemon::is_pid_locked(&pid_path)? {
        let pid_msg = residuum::daemon::read_pid_file(&pid_path)
            .map_or_else(|_| String::new(), |pid| format!(" (pid {pid})"));
        println!("residuum: {label} is already running{pid_msg}");
        return Ok(());
    }

    if pid_path.exists()
        && let Err(e) = residuum::daemon::remove_pid_file(&pid_path)
    {
        tracing::warn!(error = %e, "failed to clean stale pid file");
    }

    let gateway_addr = super::super::resolve_gateway_addr(&residuum_root);

    let has_agents = !residuum::config::discover_agents(&residuum_root)?.is_empty();
    let needs_setup = args.setup || !has_agents;

    if !args.setup {
        super::startup_config::ensure_hub_config_loads_or_has_fallback(&residuum_root, &hub_dir)?;
    }

    if needs_setup {
        println!("welcome to residuum!");
        println!();
        println!("  it looks like this is your first time running residuum.");
        println!("  configure your agent at: http://{gateway_addr}");
        println!("  or run: residuum setup");
        println!();
    }

    let exe = std::env::current_exe()
        .map_err(|e| FatalError::Gateway(format!("failed to determine current executable: {e}")))?;

    let raw_args: Vec<String> = std::env::args().collect();
    let child_args = build_child_args(&raw_args);

    residuum::daemon::remove_ready_file(&hub_dir);
    residuum::daemon::clear_startup_error(&hub_dir);

    let mut child = residuum::daemon::spawn_gateway_process(&exe, &child_args, &hub_dir)
        .map_err(|e| FatalError::Gateway(format!("failed to spawn daemon process: {e}")))?;

    if needs_setup {
        std::thread::sleep(std::time::Duration::from_millis(500));
        match child.try_wait() {
            Ok(Some(status)) => {
                report_startup_failure(
                    &hub_dir,
                    label,
                    &format!("exited immediately with {status}"),
                );
                return Err(FatalError::Gateway(format!(
                    "{label} exited immediately with {status}"
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

    let ready_path = residuum::daemon::ready_file_path(&hub_dir);
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
            report_startup_failure(&hub_dir, label, "crashed during startup");
            Err(FatalError::Gateway(format!(
                "{label} crashed during startup"
            )))
        }
        residuum::daemon::ReadinessOutcome::TimedOut => {
            report_startup_failure(
                &hub_dir,
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

//! CLI subcommand dispatch and shared argument helpers.

mod connect;
mod logs;
mod secret;
mod serve;
mod setup;
mod stop;
mod update;

use residuum::util::FatalError;

pub async fn run() -> Result<(), FatalError> {
    // Install rustls CryptoProvider before any TLS usage. Required since
    // rustls 0.23 when both `ring` and `aws-lc-rs` appear in the dep tree.
    // Err means a provider was already installed by a dependency — that's
    // expected and fine; we just continue with whatever was registered first.
    drop(rustls::crypto::ring::default_provider().install_default());

    // Install a panic hook that logs to tracing and stderr.
    // tracing::error! is a no-op until a subscriber is initialized; write_crash_note is the real fallback.
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(%info, "panic in spawned task");
        residuum::daemon::write_crash_note(&format!("PANIC: {info}"));
    }));

    // Load .env early (ignore if missing, warn on parse errors)
    if let Err(e) = dotenvy::dotenv()
        && !e.not_found()
    {
        residuum::daemon::write_crash_note(&format!("warning: failed to parse .env file: {e}"));
    }

    // Parse subcommand from argv
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str);

    // Parse --agent <name> flag from args (applies to serve, stop, connect, logs)
    let agent_name = extract_flag_value(&args, "--agent");

    match subcommand {
        Some("secret") => secret::run_secret_command(&args),
        Some("agent") => {
            residuum::agent_registry::commands::run_agent_command(args.get(2..).unwrap_or(&[]))
        }
        Some("connect") => {
            residuum::util::tracing_init::init_cli_tracing();
            let url = if let Some(ref name) = agent_name {
                // Look up port from registry
                let registry_dir = residuum::agent_registry::paths::registry_base_dir()?;
                let registry =
                    residuum::agent_registry::registry::AgentRegistry::load(&registry_dir)?;
                let entry = registry.get(name).ok_or_else(|| {
                    FatalError::Config(format!("agent '{name}' not found in registry"))
                })?;
                format!("ws://127.0.0.1:{}/ws", entry.port)
            } else {
                // Use explicit URL arg or default
                args.iter()
                    .skip(2)
                    .find(|a| !a.starts_with('-'))
                    .cloned()
                    .unwrap_or_else(|| "ws://127.0.0.1:7700/ws".to_string())
            };
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            connect::run_connect(&url, verbose).await
        }
        Some("logs") => {
            residuum::util::tracing_init::init_default_tracing();
            let watch = args.iter().any(|a| a == "--watch" || a == "-w");
            logs::run_logs_command(watch, agent_name.as_deref()).await
        }
        Some("setup") => {
            residuum::util::tracing_init::init_default_tracing();
            setup::run_setup_command(&args)
        }
        Some("stop") => {
            residuum::util::tracing_init::init_default_tracing();
            stop::run_stop_command(agent_name.as_deref()).await
        }
        Some("update") => {
            residuum::util::tracing_init::init_default_tracing();
            update::run_update_command(&args).await
        }
        // "serve" or no subcommand → start gateway
        Some("serve") | None => {
            let foreground = args.iter().any(|a| a == "--foreground");
            let debug_mode = parse_debug_flag(&args)?;

            if foreground {
                // Foreground mode: file + stderr logging, run gateway directly
                residuum::daemon::init_daemon_tracing(debug_mode, agent_name.as_deref());
                serve::run_serve_foreground(&args, agent_name.as_deref()).await
            } else {
                // Daemon mode: spawn foreground child, poll for PID file, exit
                serve::run_daemonize(&args, agent_name.as_deref())
            }
        }
        Some(other) => Err(FatalError::Config(format!(
            "unknown subcommand '{other}', expected one of: serve, connect, logs, setup, secret, stop, update, agent"
        ))),
    }
}

/// Extract a `--flag value` pair from args.
pub(super) fn extract_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Parse `--debug` or `--debug=<mode>` from args.
///
/// Returns `Ok(None)` if no `--debug` flag is present, `Ok(Some(mode))` for
/// valid modes, or an error for unrecognized mode values.
fn parse_debug_flag(args: &[String]) -> Result<Option<residuum::daemon::DebugMode>, FatalError> {
    use residuum::daemon::DebugMode;

    for arg in args {
        if arg == "--debug" {
            return Ok(Some(DebugMode::Default));
        }
        if let Some(value) = arg.strip_prefix("--debug=") {
            return DebugMode::from_flag_value(Some(value))
                .map(Some)
                .ok_or_else(|| {
                    FatalError::Config(format!(
                        "unknown debug mode '{value}', expected one of: all, trace"
                    ))
                });
        }
    }
    Ok(None)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    #[test]
    fn parse_debug_flag_absent() {
        let args: Vec<String> = vec!["residuum", "serve", "--foreground"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(super::parse_debug_flag(&args).unwrap().is_none());
    }

    #[test]
    fn parse_debug_flag_bare() {
        let args: Vec<String> = vec!["residuum", "serve", "--debug"]
            .into_iter()
            .map(String::from)
            .collect();
        let mode = super::parse_debug_flag(&args).unwrap().unwrap();
        assert_eq!(mode.filter_str(), "residuum=debug,warn");
    }

    #[test]
    fn parse_debug_flag_all() {
        let args: Vec<String> = vec!["residuum", "serve", "--debug=all"]
            .into_iter()
            .map(String::from)
            .collect();
        let mode = super::parse_debug_flag(&args).unwrap().unwrap();
        assert_eq!(mode.filter_str(), "debug");
    }

    #[test]
    fn parse_debug_flag_trace() {
        let args: Vec<String> = vec!["residuum", "serve", "--debug=trace"]
            .into_iter()
            .map(String::from)
            .collect();
        let mode = super::parse_debug_flag(&args).unwrap().unwrap();
        assert_eq!(mode.filter_str(), "residuum=trace,warn");
    }

    #[test]
    fn parse_debug_flag_unknown_mode_errors() {
        let args: Vec<String> = vec!["residuum", "serve", "--debug=bogus"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(super::parse_debug_flag(&args).is_err());
    }
}

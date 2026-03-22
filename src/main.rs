//! `Residuum`: personal AI agent gateway.
//!
//! Entrypoint with subcommands:
//! - `serve` (default): starts the gateway as a background daemon
//! - `serve --foreground`: runs the gateway in the foreground
//! - `serve --debug[=mode]`: run with debug logging (modes: all, trace)
//! - `serve --agent <name>`: start a named agent instance
//! - `stop [--agent <name>]`: stops a running gateway daemon
//! - `connect [--agent <name>] [url]`: connects a CLI client to a running gateway
//! - `logs [--agent <name>] [--watch]`: display CLI log files
//! - `setup`: interactive configuration wizard
//! - `agent <create|list|delete|info>`: manage named agent instances

use residuum::config::Config;
use residuum::error::FatalError;
use residuum::gateway::protocol::{ClientMessage, ServerMessage};
use residuum::interfaces::cli::CliClient;
use residuum::interfaces::cli::commands::CommandEffect;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        // tracing::error goes to the log file; println is for the terminal user
        tracing::error!(error = %e, "fatal error");
        println!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), FatalError> {
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
        Some("secret") => run_secret_command(&args),
        Some("agent") => {
            residuum::agent_registry::commands::run_agent_command(args.get(2..).unwrap_or(&[]))
        }
        Some("connect") => {
            init_cli_tracing();
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
            run_connect(&url, verbose).await
        }
        Some("logs") => {
            init_default_tracing();
            let watch = args.iter().any(|a| a == "--watch" || a == "-w");
            run_logs_command(watch, agent_name.as_deref()).await
        }
        Some("setup") => {
            init_default_tracing();
            run_setup_command(&args)
        }
        Some("stop") => {
            init_default_tracing();
            run_stop_command(agent_name.as_deref())
        }
        Some("update") => {
            init_default_tracing();
            run_update_command(&args).await
        }
        // "serve" or no subcommand → start gateway
        Some("serve") | None => {
            let foreground = args.iter().any(|a| a == "--foreground");
            let debug_mode = parse_debug_flag(&args)?;

            if foreground {
                // Foreground mode: file-only logging (+ stderr if --debug), run gateway directly
                residuum::daemon::init_daemon_tracing(debug_mode, agent_name.as_deref());
                run_serve_foreground(&args, agent_name.as_deref()).await
            } else {
                // Daemon mode: spawn foreground child, poll for PID file, exit
                run_daemonize(&args, agent_name.as_deref())
            }
        }
        Some(other) => Err(FatalError::Config(format!(
            "unknown subcommand '{other}', expected one of: serve, connect, logs, setup, secret, stop, update, agent"
        ))),
    }
}

/// Run the gateway in foreground mode (called as `residuum serve --foreground`).
///
/// This is the current behavior — runs the gateway event loop directly.
/// Used by the daemon spawner as the child process, or for debugging.
///
/// # Errors
///
/// Returns `FatalError` if initialization or the gateway loop fails.
async fn run_serve_foreground(args: &[String], agent_name: Option<&str>) -> Result<(), FatalError> {
    // Write PID file early so the daemon parent (and `residuum stop`) can find us,
    // even during setup wizard before the gateway starts.
    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    residuum::daemon::write_pid_file(&pid_path, std::process::id())?;

    let result = run_serve_foreground_inner(args, agent_name).await;

    // Always clean up PID file on exit
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
        let mut cfg_result = Config::load_at(&config_dir);
        // Set config_dir on success so the gateway knows where config lives
        if let Ok(ref mut cfg) = cfg_result {
            cfg.config_dir.clone_from(&config_dir);
        }
        match cfg_result {
            Ok(cfg) => {
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
                    // Backup restored — retry loading
                    match Config::load_at(&config_dir) {
                        Ok(mut cfg) => {
                            cfg.config_dir.clone_from(&config_dir);
                            tracing::info!("config restored from backup, starting gateway");
                            // Box::pin reduces stack frame size — this future is large
                            match Box::pin(residuum::gateway::run_gateway(cfg)).await? {
                                residuum::gateway::GatewayExit::Restart => {
                                    return re_exec_serve_foreground();
                                }
                                residuum::gateway::GatewayExit::Shutdown => {}
                            }
                            break;
                        }
                        Err(retry_err) => {
                            return Err(FatalError::Config(format!(
                                "config invalid after rollback: {retry_err}\n\n\
                                 fix {}/config.toml and providers.toml manually, then restart",
                                config_dir.display()
                            )));
                        }
                    }
                }
                tracing::warn!("config rollback failed: no backup available");
                return Err(FatalError::Config(format!(
                    "config invalid and rollback failed: {err}\n\n\
                     fix {}/config.toml and providers.toml manually, then restart",
                    config_dir.display()
                )));
            }
            Err(err) if agent_name.is_some() => {
                // Named agents don't get setup wizard — config must be ready
                return Err(FatalError::Config(format!(
                    "config invalid for agent '{}': {err}\n\n\
                     edit {}/config.toml manually or recreate the agent",
                    agent_name.unwrap_or(""),
                    config_dir.display()
                )));
            }
            Err(err) => {
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
fn run_daemonize(args: &[String], agent_name: Option<&str>) -> Result<(), FatalError> {
    use residuum::config::GatewayConfig;
    use residuum::daemon::{is_process_running, read_pid_file};

    init_default_tracing();

    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    let label = agent_name.map_or("gateway".to_string(), |n| format!("agent '{n}'"));

    // Check for an already-running instance
    if let Ok(existing_pid) = read_pid_file(&pid_path)
        && is_process_running(existing_pid)
    {
        println!("residuum: {label} is already running (pid {existing_pid})");
        return Ok(());
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

/// Check for and install updates from GitHub Releases.
///
/// Fetches the latest release tag, compares it against the build-time
/// version, and optionally downloads the install script to replace the
/// current binary. With `-y`/`--yes`, automatically triggers a daemon
/// restart after a successful update.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the GitHub API request fails or
/// the install script cannot be executed.
async fn run_update_command(args: &[String]) -> Result<(), FatalError> {
    use residuum::update::{self, CURRENT_VERSION};

    let check_only = args.iter().any(|a| a == "--check");
    let auto_yes = args.iter().any(|a| a == "-y" || a == "--yes");

    println!("residuum: checking for updates...");

    let latest = update::fetch_latest_version().await?;

    if update::is_up_to_date(CURRENT_VERSION, &latest) {
        println!("residuum: already up to date ({CURRENT_VERSION})");
        return Ok(());
    }

    println!("residuum: current version: {CURRENT_VERSION}");
    println!("residuum: latest version:  {latest}");

    if check_only {
        return Ok(());
    }

    println!("residuum: downloading and installing {latest}...");

    update::download_and_install(&latest).await?;
    println!("residuum: updated to {latest}");

    // Check if gateway is running and try to restart it
    if let Ok(pid_path) = residuum::daemon::pid_file_path()
        && let Ok(pid) = residuum::daemon::read_pid_file(&pid_path)
        && residuum::daemon::is_process_running(pid)
    {
        if auto_yes {
            // Try to trigger seamless restart via the API
            let gateway_addr = Config::load().map_or_else(
                |_| residuum::config::GatewayConfig::default().addr(),
                |cfg| cfg.gateway.addr(),
            );
            let url = format!("http://{gateway_addr}/api/update/restart");
            match reqwest::Client::new().post(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    println!("residuum: restart signal sent to gateway (pid {pid})");
                }
                Ok(resp) => {
                    println!(
                        "residuum: failed to signal gateway restart (status {}) — restart it manually",
                        resp.status()
                    );
                }
                Err(e) => {
                    println!(
                        "residuum: failed to signal gateway restart ({e}) — restart it manually"
                    );
                }
            }
        } else {
            println!(
                "residuum: gateway is still running (pid {pid}) — restart it to use the new version"
            );
        }
    }

    Ok(())
}

/// Stop a running gateway daemon.
///
/// Reads the PID file, verifies the process is running, sends SIGTERM,
/// and polls for the process to exit.
///
/// # Errors
///
/// Returns `FatalError` if the PID file cannot be read or the signal
/// cannot be sent.
fn run_stop_command(agent_name: Option<&str>) -> Result<(), FatalError> {
    use residuum::daemon::{is_process_running, read_pid_file, remove_pid_file, send_sigterm};

    let pid_path = residuum::agent_registry::paths::resolve_pid_path(agent_name)?;
    let label = agent_name.map_or("gateway".to_string(), |n| format!("agent '{n}'"));

    let Ok(pid) = read_pid_file(&pid_path) else {
        println!("residuum: no {label} running (no pid file)");
        return Ok(());
    };

    if !is_process_running(pid) {
        println!("residuum: no {label} running (stale pid file for pid {pid})");
        remove_pid_file(&pid_path)?;
        return Ok(());
    }

    send_sigterm(pid)?;

    // Poll for process exit (200ms intervals, 5s timeout)
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    let poll_interval = std::time::Duration::from_millis(200);

    loop {
        if !is_process_running(pid) {
            // Process exited; clean up PID file if still present
            remove_pid_file(&pid_path)?;
            println!("residuum: {label} stopped (pid {pid})");
            return Ok(());
        }

        if start.elapsed() > timeout {
            println!("residuum: {label} (pid {pid}) did not stop within 5 seconds");
            return Err(FatalError::Gateway(format!(
                "{label} pid {pid} did not exit after SIGTERM"
            )));
        }

        std::thread::sleep(poll_interval);
    }
}

/// Run the CLI connect client.
///
/// Connects to a running gateway over WebSocket and bridges stdin/stdout
/// to the agent.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the WebSocket connection fails.
async fn run_connect(url: &str, verbose: bool) -> Result<(), FatalError> {
    use futures_util::StreamExt;
    use residuum::interfaces::cli::CliReader;

    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to connect to {url}: {e}")))?;

    let mut client = CliClient::new(url, verbose);
    client.print_banner();

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send verbose preference if requested
    if verbose {
        send_client_message(&mut ws_tx, &ClientMessage::SetVerbose { enabled: true }).await?;
    }

    // Prompt gate: readline blocks after sending input until we signal turn completion
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let prompt = client.user_prompt();

    // Spawn readline thread
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(1);
    tokio::task::spawn_blocking(move || match CliReader::new() {
        Ok(reader) => reader.run(input_tx, prompt, gate_rx),
        Err(e) => println!("error initializing readline: {e}"),
    });

    let mut msg_counter: u64 = 0;
    let mut indicator_tick = tokio::time::interval(std::time::Duration::from_millis(300));
    // Track whether we need to unblock the readline gate after the current turn
    let mut turn_active = false;

    loop {
        tokio::select! {
            // User input → check for commands, then send to gateway
            input = input_rx.recv() => {
                let Some(line) = input else {
                    println!("\nGoodbye!");
                    break;
                };

                match handle_cli_input(
                    &line, &mut client, &mut ws_tx, &gate_tx,
                    &mut msg_counter, &mut turn_active,
                ).await? {
                    std::ops::ControlFlow::Break(()) => break,
                    std::ops::ControlFlow::Continue(()) => {}
                }
            }

            // Gateway → display to user
            frame = ws_rx.next() => {
                let Some(frame_result) = frame else {
                    println!("connection closed by server");
                    break;
                };

                match handle_ws_frame(frame_result, &mut client, &mut turn_active, &gate_tx) {
                    std::ops::ControlFlow::Break(()) => break,
                    std::ops::ControlFlow::Continue(()) => {}
                }
            }

            // Indicator animation tick
            _ = indicator_tick.tick(), if client.indicator.is_active() => {
                client.indicator.tick();
            }
        }
    }

    Ok(())
}

/// Initialize tracing with stderr-only output (default for serve/logs/setup).
fn init_default_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
}

/// Initialize tracing with both stderr and a daily rolling file appender.
///
/// Log files are written to `~/.residuum/logs/cli.YYYY-MM-DD.log`.
fn init_cli_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    let log_dir = dirs::home_dir().map_or_else(
        || std::path::PathBuf::from("logs"),
        |h| h.join(".residuum").join("logs"),
    );

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .filename_prefix("cli")
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(30)
        .build(&log_dir)
        .or_else(|e| {
            residuum::daemon::write_crash_note(&format!(
                "warning: failed to create log file appender: {e}"
            ));
            residuum::daemon::write_crash_note(&format!(
                "warning: logs will be written to {} instead — 'residuum logs' will not find them",
                std::env::temp_dir().display()
            ));
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix("cli")
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .build(std::env::temp_dir())
                .map_err(|e2| {
                    residuum::daemon::write_crash_note(&format!(
                        "warning: fallback log appender also failed: {e2}"
                    ));
                    residuum::daemon::write_crash_note(
                        "warning: log file output disabled — 'residuum logs' will not find them",
                    );
                })
        })
        .ok();

    let file_layer = file_appender.map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(appender)
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();
}

/// Display CLI log files.
///
/// Finds the most recent log file in `~/.residuum/logs/` and prints its
/// contents. With `--watch`, polls for new lines every 500ms.
async fn run_logs_command(watch: bool, agent_name: Option<&str>) -> Result<(), FatalError> {
    let log_dir = residuum::agent_registry::paths::resolve_log_dir(agent_name)?;

    if !log_dir.exists() {
        println!(
            "no log files found (directory does not exist: {})",
            log_dir.display()
        );
        return Ok(());
    }

    // Find the most recent log file
    let mut entries: Vec<_> = std::fs::read_dir(&log_dir)
        .map_err(|e| FatalError::Config(format!("failed to read log directory: {e}")))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect();

    if entries.is_empty() {
        println!("no log files found in {}", log_dir.display());
        return Ok(());
    }

    // Sort by modification time, most recent last
    entries.sort_by_key(|e| {
        match e.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(path = %e.path().display(), error = %err, "failed to read log file metadata for sorting");
                std::time::SystemTime::UNIX_EPOCH
            }
        }
    });

    let Some(latest_entry) = entries.last() else {
        return Ok(());
    };
    let latest = latest_entry.path();

    println!("showing: {}", latest.display());
    println!();

    let content = std::fs::read_to_string(&latest)
        .map_err(|e| FatalError::Config(format!("failed to read log file: {e}")))?;
    print!("{content}");

    if watch {
        use tokio::io::{AsyncBufReadExt, AsyncSeekExt};

        let file = tokio::fs::File::open(&latest)
            .await
            .map_err(|e| FatalError::Config(format!("failed to open log file for watch: {e}")))?;
        let mut reader = tokio::io::BufReader::new(file);

        // Seek to current end
        let metadata = std::fs::metadata(&latest)
            .map_err(|e| FatalError::Config(format!("failed to stat log file: {e}")))?;
        let file_len = metadata.len();
        reader
            .seek(std::io::SeekFrom::Start(file_len))
            .await
            .map_err(|e| FatalError::Config(format!("failed to seek log file: {e}")))?;

        let mut line_buf = String::new();
        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => {
                    // No new data yet
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(_) => {
                    print!("{line_buf}");
                }
                Err(e) => {
                    println!("error reading log file: {e}");
                    println!(
                        "  hint: the log file may have been rotated — re-run 'residuum logs --watch' to follow the new file"
                    );
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Run the `setup` subcommand — interactive or flag-driven config wizard.
fn run_setup_command(args: &[String]) -> Result<(), FatalError> {
    use residuum::config::wizard;

    let config_dir = Config::config_dir()?;
    let config_path = config_dir.join("config.toml");

    if config_path.exists() {
        println!("config.toml already exists at {}", config_path.display());
        println!("edit it directly or delete it to re-run setup");
        return Ok(());
    }

    // Check if any flags are present → non-interactive mode
    let tz_flag = extract_flag_value(args, "--timezone");
    let provider_flag = extract_flag_value(args, "--provider");
    let key_flag = extract_flag_value(args, "--api-key");
    let model_flag = extract_flag_value(args, "--model");
    let ws_backend_flag = extract_flag_value(args, "--web-search-backend");
    let ws_key_flag = extract_flag_value(args, "--web-search-api-key");
    let ws_url_flag = extract_flag_value(args, "--web-search-base-url");

    let has_flags = tz_flag.is_some()
        || provider_flag.is_some()
        || key_flag.is_some()
        || model_flag.is_some()
        || ws_backend_flag.is_some()
        || ws_key_flag.is_some()
        || ws_url_flag.is_some();

    let answers = if has_flags {
        wizard::from_flags(
            tz_flag.as_deref(),
            provider_flag.as_deref(),
            key_flag.as_deref(),
            model_flag.as_deref(),
            ws_backend_flag.as_deref(),
            ws_key_flag.as_deref(),
            ws_url_flag.as_deref(),
        )?
    } else {
        wizard::run_interactive()?
    };

    // Bootstrap creates the directory + example config
    Config::bootstrap_at_dir(&config_dir)?;
    // Write the wizard-generated config (overwrites the minimal template)
    wizard::write_config(&config_dir, &answers)?;

    // Validate the result
    match Config::load_at(&config_dir) {
        Ok(cfg) => {
            println!("configuration saved to {}", config_path.display());
            println!("  timezone: {}", answers.timezone);
            println!("  model: {}/{}", answers.provider, answers.model);
            if cfg.main.first().and_then(|s| s.api_key.as_ref()).is_some() {
                println!("  api key: configured");
            }
            if let Some(ref backend) = answers.web_search_backend {
                println!("  web search: {backend}");
            }
        }
        Err(err) => {
            println!("warning: config was written but validation failed: {err}");
            println!("you may need to edit {} manually", config_path.display());
        }
    }

    Ok(())
}

/// Run the `secret` subcommand — manage encrypted secret storage.
///
/// Subcommands:
/// - `residuum secret set <name> [value]` — store a secret (prompts for value if omitted)
/// - `residuum secret list` — list stored secret names
/// - `residuum secret delete <name>` — remove a secret
fn run_secret_command(args: &[String]) -> Result<(), FatalError> {
    use residuum::config::SecretStore;

    let config_dir = Config::config_dir()?;
    let sub = args.get(2).map(String::as_str);

    match sub {
        Some("set") => {
            let Some(name) = args.get(3) else {
                println!("usage: residuum secret set <name> [value]");
                return Ok(());
            };

            let value = if let Some(v) = args.get(4) {
                v.clone()
            } else {
                // Prompt for value with masked input
                rpassword::prompt_password(format!("value for '{name}': "))
                    .map_err(|e| FatalError::Config(format!("failed to read secret value: {e}")))?
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.set(name, &value, &config_dir)?;
            println!("secret '{name}' saved");
        }
        Some("list") => {
            let store = SecretStore::load(&config_dir)?;
            let names = store.names();
            if names.is_empty() {
                println!("no secrets stored");
            } else {
                for name in &names {
                    println!("{name}");
                }
            }
        }
        Some("delete") => {
            let Some(name) = args.get(3) else {
                println!("usage: residuum secret delete <name>");
                return Ok(());
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.delete(name, &config_dir)?;
            println!("secret '{name}' deleted");
        }
        _ => {
            println!("usage: residuum secret <set|list|delete>");
            println!();
            println!("  set <name> [value]  store a secret (prompts if value omitted)");
            println!("  list                list stored secret names");
            println!("  delete <name>       remove a secret");
        }
    }

    Ok(())
}

/// Extract a `--flag value` pair from args.
fn extract_flag_value(args: &[String], flag: &str) -> Option<String> {
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

/// Process a single WebSocket frame from the gateway.
///
/// Returns `Break(())` to exit the event loop, `Continue(())` otherwise.
fn handle_ws_frame(
    frame_result: Result<
        tokio_tungstenite::tungstenite::Message,
        tokio_tungstenite::tungstenite::Error,
    >,
    client: &mut CliClient,
    turn_active: &mut bool,
    gate_tx: &std::sync::mpsc::Sender<()>,
) -> std::ops::ControlFlow<()> {
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let raw = match frame_result {
        Ok(TungsteniteMessage::Text(txt)) => txt,
        Ok(TungsteniteMessage::Close(_)) => {
            println!("server closed connection");
            return std::ops::ControlFlow::Break(());
        }
        Ok(_) => return std::ops::ControlFlow::Continue(()),
        Err(e) => {
            println!("websocket error: {e}");
            return std::ops::ControlFlow::Break(());
        }
    };

    match serde_json::from_str::<ServerMessage>(&raw) {
        Ok(ServerMessage::Reloading) => {
            println!("server is reloading configuration...");
        }
        Ok(ref server_msg @ ServerMessage::Response { ref reply_to, .. })
            if *turn_active && !reply_to.is_empty() =>
        {
            client.display(server_msg);
            *turn_active = false;
            if gate_tx.send(()).is_err() {
                tracing::debug!("prompt gate send failed: readline thread has exited");
            }
        }
        Ok(ref server_msg @ ServerMessage::Error { .. }) if *turn_active => {
            client.display(server_msg);
            *turn_active = false;
            if gate_tx.send(()).is_err() {
                tracing::debug!("prompt gate send failed: readline thread has exited");
            }
        }
        Ok(server_msg) => client.display(&server_msg),
        Err(e) => tracing::warn!(error = %e, "failed to parse server message"),
    }

    std::ops::ControlFlow::Continue(())
}

/// Handle a line of CLI input: dispatch slash commands or send as a message.
///
/// Returns `Break(())` to exit the event loop, `Continue(())` otherwise.
///
/// # Errors
///
/// Returns `FatalError::Gateway` on serialization or send failure.
async fn handle_cli_input<S>(
    line: &str,
    client: &mut CliClient,
    ws_tx: &mut S,
    gate_tx: &std::sync::mpsc::Sender<()>,
    msg_counter: &mut u64,
    turn_active: &mut bool,
) -> Result<std::ops::ControlFlow<()>, FatalError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    if let Some(effect) = client.handle_command(line) {
        match effect {
            CommandEffect::ToggleVerbose => {
                let new_verbose = !client.verbose();
                client.set_verbose(new_verbose);
                let label = if new_verbose { "on" } else { "off" };
                println!("verbose mode: {label}");
                send_client_message(
                    ws_tx,
                    &ClientMessage::SetVerbose {
                        enabled: new_verbose,
                    },
                )
                .await?;
            }
            CommandEffect::Reload => {
                send_client_message(ws_tx, &ClientMessage::Reload).await?;
            }
            CommandEffect::ServerCommand { name, args } => {
                send_client_message(
                    ws_tx,
                    &ClientMessage::ServerCommand {
                        name: name.to_string(),
                        args,
                    },
                )
                .await?;
            }
            CommandEffect::InboxAdd(body) => {
                send_client_message(ws_tx, &ClientMessage::InboxAdd { body }).await?;
            }
            CommandEffect::Quit => return Ok(std::ops::ControlFlow::Break(())),
            CommandEffect::PrintLocal(text) => println!("{text}"),
        }
        // Slash commands don't trigger agent turns; unblock prompt immediately
        if gate_tx.send(()).is_err() {
            tracing::debug!("prompt gate send failed: readline thread has exited");
        }
        return Ok(std::ops::ControlFlow::Continue(()));
    }

    *msg_counter += 1;
    let client_msg = ClientMessage::SendMessage {
        id: format!("cli-{}", *msg_counter),
        content: line.to_string(),
        images: vec![],
    };
    let json = serde_json::to_string(&client_msg)
        .map_err(|e| FatalError::Gateway(format!("failed to serialize message: {e}")))?;
    if let Err(e) = ws_tx.send(TungsteniteMessage::text(json)).await {
        tracing::warn!(error = %e, "connection closed");
        return Ok(std::ops::ControlFlow::Break(()));
    }
    *turn_active = true;

    Ok(std::ops::ControlFlow::Continue(()))
}

/// Serialize and send a `ClientMessage` over the WebSocket.
///
/// # Errors
///
/// Returns `FatalError::Gateway` on serialization or send failure.
async fn send_client_message<S>(ws_tx: &mut S, msg: &ClientMessage) -> Result<(), FatalError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let json = serde_json::to_string(msg)
        .map_err(|e| FatalError::Gateway(format!("failed to serialize message: {e}")))?;
    ws_tx
        .send(TungsteniteMessage::text(json))
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to send message: {e}")))?;
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    #[test]
    fn backup_config_creates_bak_file() {
        use residuum::gateway::backup_config;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "timezone = \"UTC\"\n").unwrap();

        backup_config(dir.path());

        let bak = dir.path().join("config.toml.bak");
        assert!(bak.exists(), "backup should create config.toml.bak");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "timezone = \"UTC\"\n",
            "backup content should match original"
        );
    }

    #[test]
    fn rollback_restores_backup() {
        use residuum::gateway::rollback_config;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let bak = dir.path().join("config.toml.bak");

        std::fs::write(&bak, "timezone = \"UTC\"\n").unwrap();
        std::fs::write(&config, "BROKEN").unwrap();

        assert!(rollback_config(dir.path()), "rollback should succeed");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "timezone = \"UTC\"\n",
            "config should be restored from backup"
        );
    }

    #[test]
    fn rollback_fails_without_backup() {
        use residuum::gateway::rollback_config;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "BROKEN").unwrap();

        assert!(
            !rollback_config(dir.path()),
            "rollback should fail when no backup exists"
        );
    }

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

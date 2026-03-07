//! `Residuum`: personal AI agent gateway.
//!
//! Entrypoint with subcommands:
//! - `serve` (default): starts the gateway as a background daemon
//! - `serve --foreground`: runs the gateway in the foreground
//! - `stop`: stops a running gateway daemon
//! - `connect [url]`: connects a CLI client to a running gateway
//! - `logs [--watch]`: display CLI log files
//! - `setup`: interactive configuration wizard

use residuum::config::Config;
use residuum::error::ResiduumError;
use residuum::gateway::protocol::{ClientMessage, ServerMessage};
use residuum::interfaces::cli::CliClient;
use residuum::interfaces::cli::commands::CommandEffect;

#[tokio::main]
async fn main() {
    if let Err(e) = Box::pin(run()).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ResiduumError> {
    // Load .env early (ignore if missing, warn on parse errors)
    if let Err(e) = dotenvy::dotenv()
        && !e.not_found()
    {
        eprintln!("warning: failed to parse .env file: {e}");
    }

    // Parse subcommand from argv
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str);

    match subcommand {
        Some("secret") => run_secret_command(&args),
        Some("connect") => {
            init_cli_tracing();
            let url = args.get(2).map_or("ws://127.0.0.1:7700/ws", String::as_str);
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            run_connect(url, verbose).await
        }
        Some("logs") => {
            init_default_tracing();
            let watch = args.iter().any(|a| a == "--watch" || a == "-w");
            run_logs_command(watch).await
        }
        Some("setup") => {
            init_default_tracing();
            run_setup_command(&args)
        }
        Some("stop") => run_stop_command(),
        Some("update") => run_update_command(&args).await,
        // "serve" or no subcommand → start gateway
        Some("serve") | None => {
            let foreground = args.iter().any(|a| a == "--foreground");

            if foreground {
                // Foreground mode: file-only logging, run gateway directly
                residuum::daemon::init_daemon_tracing();
                run_serve_foreground(&args).await
            } else {
                // Daemon mode: spawn foreground child, poll for PID file, exit
                run_daemonize(&args)
            }
        }
        Some(other) => Err(ResiduumError::Config(format!(
            "unknown subcommand '{other}', expected one of: serve, connect, logs, setup, secret, stop, update"
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
/// Returns `ResiduumError` if initialization or the gateway loop fails.
async fn run_serve_foreground(args: &[String]) -> Result<(), ResiduumError> {
    // Write PID file early so the daemon parent (and `residuum stop`) can find us,
    // even during setup wizard before the gateway starts.
    let pid_path = residuum::daemon::pid_file_path()?;
    residuum::daemon::write_pid_file(&pid_path, std::process::id())?;

    let result = run_serve_foreground_inner(args).await;

    // Always clean up PID file on exit
    if let Err(e) = residuum::daemon::remove_pid_file(&pid_path) {
        tracing::warn!(error = %e, "failed to remove pid file on exit");
    }

    result
}

/// Inner implementation of foreground serve, wrapped by PID file lifecycle.
async fn run_serve_foreground_inner(args: &[String]) -> Result<(), ResiduumError> {
    // --setup: run the onboarding wizard in an isolated temp directory,
    // then boot the gateway with the resulting config for end-to-end testing
    if args.iter().any(|a| a == "--setup") {
        let tmp_dir = std::env::temp_dir().join("residuum-setup");
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir).map_err(|e| {
                ResiduumError::Config(format!(
                    "failed to clean setup directory {}: {e}",
                    tmp_dir.display()
                ))
            })?;
        }
        residuum::config::Config::bootstrap_at_dir(&tmp_dir)?;
        eprintln!(
            "setup mode: config will be written to {}",
            tmp_dir.display()
        );
        match Box::pin(residuum::gateway::setup::run_setup_server_at(
            tmp_dir.clone(),
        ))
        .await?
        {
            residuum::gateway::setup::SetupExit::ConfigSaved => {
                tracing::info!("setup complete, loading config from temp directory");
            }
            residuum::gateway::setup::SetupExit::Shutdown => return Ok(()),
        }

        // Load the config written by the wizard and run the gateway
        let mut cfg = Config::load_at(&tmp_dir)?;
        cfg.workspace_dir = tmp_dir.join("workspace");
        if let Some(first) = cfg.skills.dirs.first_mut() {
            *first =
                residuum::workspace::layout::WorkspaceLayout::new(&cfg.workspace_dir).skills_dir();
        }
        tracing::info!(
            model = cfg.main.first().map_or("(none)", |s| s.model.model.as_str()),
            provider_url = cfg.main.first().map_or("(none)", |s| s.provider_url.as_str()),
            workspace = %cfg.workspace_dir.display(),
            "setup-mode: configuration loaded, starting gateway"
        );
        Box::pin(residuum::gateway::run_gateway(cfg)).await?;
        return Ok(());
    }

    let config_dir = Config::config_dir()?;
    // Determine first-boot from disk state: if a backup exists, the gateway
    // has previously loaded a valid config, so this is a restart.
    let is_first_boot = !config_dir.join("config.toml.bak").exists();

    loop {
        Config::bootstrap_config_dir()?;
        match Config::load() {
            Ok(cfg) => {
                tracing::info!(
                    model = cfg.main.first().map_or("(none)", |s| s.model.model.as_str()),
                    provider_url = cfg.main.first().map_or("(none)", |s| s.provider_url.as_str()),
                    workspace = %cfg.workspace_dir.display(),
                    "configuration loaded"
                );
                // Gateway handles reloads in-place and only returns on shutdown
                // or fatal error. Backup is created inside run_gateway().
                Box::pin(residuum::gateway::run_gateway(cfg)).await?;
                break;
            }
            Err(err) if !is_first_boot => {
                // Config broken on restart — try restoring from backup
                tracing::warn!(error = %err, "config invalid, attempting rollback from backup");
                if residuum::gateway::rollback_config(&config_dir) {
                    // Backup restored — retry loading
                    match Config::load() {
                        Ok(cfg) => {
                            tracing::info!("config restored from backup, starting gateway");
                            Box::pin(residuum::gateway::run_gateway(cfg)).await?;
                            break;
                        }
                        Err(retry_err) => {
                            return Err(ResiduumError::Config(format!(
                                "config invalid after rollback: {retry_err}\n\n\
                                 fix ~/.residuum/config.toml and ~/.residuum/providers.toml manually, then restart"
                            )));
                        }
                    }
                }
                return Err(ResiduumError::Config(format!(
                    "config invalid and no backup available: {err}\n\n\
                     fix ~/.residuum/config.toml and ~/.residuum/providers.toml manually, then restart"
                )));
            }
            Err(err) => {
                // First boot — setup wizard
                tracing::warn!(error = %err, "config invalid, starting setup wizard");
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

/// Spawn the gateway as a background daemon process.
///
/// Launches `residuum serve --foreground` as a detached child, polls for the
/// PID file to confirm startup, then exits. Prints a first-launch welcome
/// message if no config exists yet.
///
/// # Errors
///
/// Returns `ResiduumError` if the child process cannot be spawned or
/// startup times out.
fn run_daemonize(args: &[String]) -> Result<(), ResiduumError> {
    use residuum::config::GatewayConfig;
    use residuum::daemon::{is_process_running, pid_file_path, read_pid_file};

    let pid_path = pid_file_path()?;

    // Check for an already-running instance
    if let Ok(existing_pid) = read_pid_file(&pid_path)
        && is_process_running(existing_pid)
    {
        eprintln!("residuum: gateway is already running (pid {existing_pid})");
        return Ok(());
    }

    // Resolve gateway address from config or defaults
    let gateway_addr = Config::load().map_or_else(
        |_| GatewayConfig::default().addr(),
        |cfg| cfg.gateway.addr(),
    );

    // Detect whether the child will enter setup mode (no PID file until setup completes)
    let config_dir = Config::config_dir()?;
    let needs_setup =
        args.iter().any(|a| a == "--setup") || !config_dir.join("config.toml").exists();

    // First-launch welcome (or --setup which mimics it)
    if needs_setup {
        eprintln!("welcome to residuum!");
        eprintln!();
        eprintln!("  it looks like this is your first time running residuum.");
        eprintln!("  configure your agent at: http://{gateway_addr}");
        eprintln!("  or run: residuum setup");
        eprintln!();
    }

    // Build child args: forward any extra flags (like --setup) plus --foreground
    let exe = std::env::current_exe().map_err(|e| {
        ResiduumError::Gateway(format!("failed to determine current executable: {e}"))
    })?;

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
        .map_err(|e| ResiduumError::Gateway(format!("failed to spawn daemon process: {e}")))?;

    // When setup is needed, the setup wizard runs before the gateway and
    // no PID file is written until setup completes. Just verify the child
    // is alive and direct the user to the web UI.
    if needs_setup {
        // Brief pause to catch immediate crashes
        std::thread::sleep(std::time::Duration::from_millis(500));
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(ResiduumError::Gateway(format!(
                    "daemon exited immediately with {status}"
                )));
            }
            Ok(None) => {
                eprintln!("residuum: setup server starting at http://{gateway_addr}");
                return Ok(());
            }
            Err(e) => {
                return Err(ResiduumError::Gateway(format!(
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
            eprintln!("residuum: gateway did not start within 10 seconds");
            eprintln!("  check logs: residuum logs");
            return Err(ResiduumError::Gateway(
                "daemon startup timed out".to_string(),
            ));
        }

        if let Ok(pid) = read_pid_file(&pid_path)
            && is_process_running(pid)
        {
            eprintln!("residuum: gateway started at http://{gateway_addr} (pid {pid})");
            return Ok(());
        }

        std::thread::sleep(poll_interval);
    }
}

/// Check for and install updates from GitHub Releases.
///
/// Fetches the latest release tag, compares it against the build-time
/// version, and optionally downloads the install script to replace the
/// current binary.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` if the GitHub API request fails or
/// the install script cannot be executed.
async fn run_update_command(args: &[String]) -> Result<(), ResiduumError> {
    let check_only = args.iter().any(|a| a == "--check");
    let current = env!("RESIDUUM_VERSION");

    eprintln!("residuum: checking for updates...");

    let latest = fetch_latest_version().await?;

    if is_up_to_date(current, &latest) {
        eprintln!("residuum: already up to date ({current})");
        return Ok(());
    }

    eprintln!("residuum: current version: {current}");
    eprintln!("residuum: latest version:  {latest}");

    if check_only {
        return Ok(());
    }

    eprintln!("residuum: downloading and installing {latest}...");

    // Download install script to a temp file and execute it
    let client = reqwest::Client::new();
    let script = client
        .get("https://residuum.bearflinn.com/install")
        .send()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to download install script: {e}")))?
        .text()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to read install script body: {e}")))?;

    let tmp_dir = std::env::temp_dir();
    let script_path = tmp_dir.join("residuum-install.sh");
    std::fs::write(&script_path, &script)
        .map_err(|e| ResiduumError::Gateway(format!("failed to write install script: {e}")))?;

    let status = std::process::Command::new("sh")
        .arg(&script_path)
        .status()
        .map_err(|e| ResiduumError::Gateway(format!("failed to execute install script: {e}")))?;

    // Clean up temp script (best-effort, ignore failure)
    drop(std::fs::remove_file(&script_path));

    if !status.success() {
        return Err(ResiduumError::Gateway(format!(
            "install script exited with {status}"
        )));
    }

    eprintln!("residuum: updated to {latest}");

    // Warn if gateway is running
    if let Ok(pid_path) = residuum::daemon::pid_file_path()
        && let Ok(pid) = residuum::daemon::read_pid_file(&pid_path)
        && residuum::daemon::is_process_running(pid)
    {
        eprintln!(
            "residuum: gateway is still running (pid {pid}) — restart it to use the new version"
        );
    }

    Ok(())
}

/// Fetch the latest release tag name from GitHub.
async fn fetch_latest_version() -> Result<String, ResiduumError> {
    let client = reqwest::Client::builder()
        .user_agent("residuum-updater")
        .build()
        .map_err(|e| ResiduumError::Gateway(format!("failed to build http client: {e}")))?;

    let resp = client
        .get("https://api.github.com/repos/grizzly-endeavors/residuum/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to fetch latest release: {e}")))?;

    if !resp.status().is_success() {
        return Err(ResiduumError::Gateway(format!(
            "github api returned {} — are you online?",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to parse release response: {e}")))?;

    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            ResiduumError::Gateway("release response missing tag_name field".to_string())
        })
}

/// Compare the current build version against the latest release tag.
///
/// Returns `true` if the current version starts with the latest tag,
/// accounting for `git describe` suffixes like `-5-gabcdef1`.
fn is_up_to_date(current: &str, latest: &str) -> bool {
    // Exact match (tagged commit)
    if current == latest {
        return true;
    }
    // current is "v2026.03.02-5-gabcdef1" and latest is "v2026.03.02" —
    // the current build is *ahead* of the latest release
    if current.starts_with(latest) && current.as_bytes().get(latest.len()) == Some(&b'-') {
        return true;
    }
    false
}

/// Stop a running gateway daemon.
///
/// Reads the PID file, verifies the process is running, sends SIGTERM,
/// and polls for the process to exit.
///
/// # Errors
///
/// Returns `ResiduumError` if the PID file cannot be read or the signal
/// cannot be sent.
fn run_stop_command() -> Result<(), ResiduumError> {
    use residuum::daemon::{
        is_process_running, pid_file_path, read_pid_file, remove_pid_file, send_sigterm,
    };

    let pid_path = pid_file_path()?;

    let Ok(pid) = read_pid_file(&pid_path) else {
        eprintln!("residuum: no gateway running (no pid file)");
        return Ok(());
    };

    if !is_process_running(pid) {
        eprintln!("residuum: no gateway running (stale pid file for pid {pid})");
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
            eprintln!("residuum: gateway stopped (pid {pid})");
            return Ok(());
        }

        if start.elapsed() > timeout {
            eprintln!("residuum: gateway (pid {pid}) did not stop within 5 seconds");
            return Err(ResiduumError::Gateway(format!(
                "gateway pid {pid} did not exit after SIGTERM"
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
/// Returns `ResiduumError::Gateway` if the WebSocket connection fails.
async fn run_connect(url: &str, verbose: bool) -> Result<(), ResiduumError> {
    use futures_util::StreamExt;
    use residuum::interfaces::cli::CliReader;

    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to connect to {url}: {e}")))?;

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
        Err(e) => eprintln!("error initializing readline: {e}"),
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
                    eprintln!("\nGoodbye!");
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
                    eprintln!("connection closed by server");
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
        .unwrap_or_else(|e| {
            eprintln!("warning: failed to create log file appender: {e}");
            // Fall back to writing to /dev/null-equivalent temp dir
            tracing_appender::rolling::RollingFileAppender::builder()
                .filename_prefix("cli")
                .filename_suffix("log")
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .build(std::env::temp_dir())
                .unwrap_or_else(|e2| {
                    eprintln!("warning: fallback log appender also failed: {e2}");
                    // Last resort: same as daily to temp dir with never rotation
                    tracing_appender::rolling::daily(std::env::temp_dir(), "cli.log")
                })
        });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(file_appender);

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
async fn run_logs_command(watch: bool) -> Result<(), ResiduumError> {
    let log_dir = dirs::home_dir()
        .map(|h| h.join(".residuum").join("logs"))
        .ok_or_else(|| ResiduumError::Config("could not determine home directory".to_string()))?;

    if !log_dir.exists() {
        eprintln!(
            "no log files found (directory does not exist: {})",
            log_dir.display()
        );
        return Ok(());
    }

    // Find the most recent log file
    let mut entries: Vec<_> = std::fs::read_dir(&log_dir)
        .map_err(|e| ResiduumError::Config(format!("failed to read log directory: {e}")))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .collect();

    if entries.is_empty() {
        eprintln!("no log files found in {}", log_dir.display());
        return Ok(());
    }

    // Sort by modification time, most recent last
    entries.sort_by_key(|e| {
        e.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let latest = entries
        .last()
        .map(std::fs::DirEntry::path)
        .ok_or_else(|| ResiduumError::Config("no log files found".to_string()))?;

    eprintln!("showing: {}", latest.display());
    eprintln!();

    let content = std::fs::read_to_string(&latest)
        .map_err(|e| ResiduumError::Config(format!("failed to read log file: {e}")))?;
    print!("{content}");

    if watch {
        use tokio::io::{AsyncBufReadExt, AsyncSeekExt};

        let file = tokio::fs::File::open(&latest).await.map_err(|e| {
            ResiduumError::Config(format!("failed to open log file for watch: {e}"))
        })?;
        let mut reader = tokio::io::BufReader::new(file);

        // Seek to current end
        let metadata = std::fs::metadata(&latest)
            .map_err(|e| ResiduumError::Config(format!("failed to stat log file: {e}")))?;
        let file_len = metadata.len();
        reader
            .seek(std::io::SeekFrom::Start(file_len))
            .await
            .map_err(|e| ResiduumError::Config(format!("failed to seek log file: {e}")))?;

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
                    eprintln!("error reading log file: {e}");
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Run the `setup` subcommand — interactive or flag-driven config wizard.
fn run_setup_command(args: &[String]) -> Result<(), ResiduumError> {
    use residuum::config::wizard;

    let config_dir = Config::config_dir()?;
    let config_path = config_dir.join("config.toml");

    if config_path.exists() {
        eprintln!("config.toml already exists at {}", config_path.display());
        eprintln!("edit it directly or delete it to re-run setup");
        return Ok(());
    }

    // Check if any flags are present → non-interactive mode
    let tz_flag = extract_flag_value(args, "--timezone");
    let provider_flag = extract_flag_value(args, "--provider");
    let key_flag = extract_flag_value(args, "--api-key");
    let model_flag = extract_flag_value(args, "--model");

    let has_flags =
        tz_flag.is_some() || provider_flag.is_some() || key_flag.is_some() || model_flag.is_some();

    let answers = if has_flags {
        wizard::from_flags(
            tz_flag.as_deref(),
            provider_flag.as_deref(),
            key_flag.as_deref(),
            model_flag.as_deref(),
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
            eprintln!("configuration saved to {}", config_path.display());
            eprintln!("  timezone: {}", answers.timezone);
            eprintln!("  model: {}/{}", answers.provider, answers.model);
            if cfg.main.first().and_then(|s| s.api_key.as_ref()).is_some() {
                eprintln!("  api key: configured");
            }
        }
        Err(err) => {
            eprintln!("warning: config was written but validation failed: {err}");
            eprintln!("you may need to edit {} manually", config_path.display());
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
fn run_secret_command(args: &[String]) -> Result<(), ResiduumError> {
    use residuum::config::SecretStore;

    let config_dir = Config::config_dir()?;
    let sub = args.get(2).map(String::as_str);

    match sub {
        Some("set") => {
            let Some(name) = args.get(3) else {
                eprintln!("usage: residuum secret set <name> [value]");
                return Ok(());
            };

            let value = if let Some(v) = args.get(4) {
                v.clone()
            } else {
                // Prompt for value with masked input
                rpassword::prompt_password(format!("value for '{name}': ")).map_err(|e| {
                    ResiduumError::Config(format!("failed to read secret value: {e}"))
                })?
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.set(name, &value, &config_dir)?;
            eprintln!("secret '{name}' saved");
        }
        Some("list") => {
            let store = SecretStore::load(&config_dir)?;
            let names = store.names();
            if names.is_empty() {
                eprintln!("no secrets stored");
            } else {
                for name in &names {
                    println!("{name}");
                }
            }
        }
        Some("delete") => {
            let Some(name) = args.get(3) else {
                eprintln!("usage: residuum secret delete <name>");
                return Ok(());
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.delete(name, &config_dir)?;
            eprintln!("secret '{name}' deleted");
        }
        _ => {
            eprintln!("usage: residuum secret <set|list|delete>");
            eprintln!();
            eprintln!("  set <name> [value]  store a secret (prompts if value omitted)");
            eprintln!("  list                list stored secret names");
            eprintln!("  delete <name>       remove a secret");
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
            eprintln!("server closed connection");
            return std::ops::ControlFlow::Break(());
        }
        Ok(_) => return std::ops::ControlFlow::Continue(()),
        Err(e) => {
            eprintln!("websocket error: {e}");
            return std::ops::ControlFlow::Break(());
        }
    };

    match serde_json::from_str::<ServerMessage>(&raw) {
        Ok(ServerMessage::Reloading) => {
            eprintln!("server is reloading configuration...");
        }
        Ok(ref server_msg @ ServerMessage::Response { ref reply_to, .. })
            if *turn_active && !reply_to.is_empty() =>
        {
            client.display(server_msg);
            *turn_active = false;
            gate_tx.send(()).ok();
        }
        Ok(ref server_msg @ ServerMessage::Error { .. }) if *turn_active => {
            client.display(server_msg);
            *turn_active = false;
            gate_tx.send(()).ok();
        }
        Ok(server_msg) => client.display(&server_msg),
        Err(e) => eprintln!("warning: failed to parse server message: {e}"),
    }

    std::ops::ControlFlow::Continue(())
}

/// Handle a line of CLI input: dispatch slash commands or send as a message.
///
/// Returns `Break(())` to exit the event loop, `Continue(())` otherwise.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` on serialization or send failure.
async fn handle_cli_input<S>(
    line: &str,
    client: &mut CliClient,
    ws_tx: &mut S,
    gate_tx: &std::sync::mpsc::Sender<()>,
    msg_counter: &mut u64,
    turn_active: &mut bool,
) -> Result<std::ops::ControlFlow<()>, ResiduumError>
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
                eprintln!("verbose mode: {label}");
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
            CommandEffect::PrintLocal(text) => eprintln!("{text}"),
        }
        // Slash commands don't trigger agent turns; unblock prompt immediately
        gate_tx.send(()).ok();
        return Ok(std::ops::ControlFlow::Continue(()));
    }

    *msg_counter = msg_counter.wrapping_add(1);
    let client_msg = ClientMessage::SendMessage {
        id: format!("cli-{}", *msg_counter),
        content: line.to_string(),
    };
    let json = serde_json::to_string(&client_msg)
        .map_err(|e| ResiduumError::Gateway(format!("failed to serialize message: {e}")))?;
    if ws_tx.send(TungsteniteMessage::text(json)).await.is_err() {
        eprintln!("connection closed");
        return Ok(std::ops::ControlFlow::Break(()));
    }
    *turn_active = true;

    Ok(std::ops::ControlFlow::Continue(()))
}

/// Serialize and send a `ClientMessage` over the WebSocket.
///
/// # Errors
///
/// Returns `ResiduumError::Gateway` on serialization or send failure.
async fn send_client_message<S>(ws_tx: &mut S, msg: &ClientMessage) -> Result<(), ResiduumError>
where
    S: futures_util::Sink<
            tokio_tungstenite::tungstenite::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let json = serde_json::to_string(msg)
        .map_err(|e| ResiduumError::Gateway(format!("failed to serialize message: {e}")))?;
    ws_tx
        .send(TungsteniteMessage::text(json))
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to send message: {e}")))?;
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    #[test]
    fn first_boot_detected_when_no_backup_exists() {
        let dir = tempfile::tempdir().unwrap();
        let is_first = !dir.path().join("config.toml.bak").exists();
        assert!(is_first, "no backup file should indicate first boot");
    }

    #[test]
    fn not_first_boot_when_backup_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml.bak"), "# previous config").unwrap();
        let is_first = !dir.path().join("config.toml.bak").exists();
        assert!(
            !is_first,
            "existing backup should indicate prior successful boot"
        );
    }

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
    fn is_up_to_date_exact_match() {
        assert!(
            super::is_up_to_date("v2026.03.02", "v2026.03.02"),
            "exact match should be up to date"
        );
    }

    #[test]
    fn is_up_to_date_ahead_of_release() {
        assert!(
            super::is_up_to_date("v2026.03.02-5-gabcdef1", "v2026.03.02"),
            "build ahead of latest release should be up to date"
        );
    }

    #[test]
    fn is_up_to_date_different_version() {
        assert!(
            !super::is_up_to_date("v2026.03.01", "v2026.03.02"),
            "older version should not be up to date"
        );
    }

    #[test]
    fn is_up_to_date_dev_build() {
        assert!(
            !super::is_up_to_date("dev", "v2026.03.02"),
            "dev build should not be up to date"
        );
    }

    #[test]
    fn is_up_to_date_no_false_prefix_match() {
        assert!(
            !super::is_up_to_date("v2026.03.021", "v2026.03.02"),
            "version with shared prefix but no dash separator should not match"
        );
    }

    #[test]
    fn extract_check_flag() {
        let args: Vec<String> = vec!["residuum", "update", "--check"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(
            args.iter().any(|a| a == "--check"),
            "--check flag should be detected"
        );
    }
}

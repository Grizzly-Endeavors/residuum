//! CLI subcommand dispatch using clap.

mod a2a;
mod agent;
mod agent_keys;
mod bug_report;
mod feedback;
mod hub_client;
mod logs;
mod secret;
mod serve;
mod setup;
mod stop;
mod teams;
mod tracing_cmd;
mod update;
mod update_watchdog;

use clap::Parser;

use residuum::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger};
use residuum::util::FatalError;

/// Checkpoint the config repository before a CLI write, if a checkpoint
/// engine could be opened. Never fails or blocks the command — see
/// [`CheckpointEngine::open_for_cli`].
async fn checkpoint_config_before_write(checkpoints: Option<&CheckpointEngine>, summary: String) {
    let Some(engine) = checkpoints else { return };
    engine
        .checkpoint_config_before_write(CheckpointContext::system(
            CheckpointTrigger::PreConfigWrite,
            summary,
        ))
        .await;
}

fn resolve_gateway_addr(residuum_root: &std::path::Path) -> String {
    use residuum::config::{GatewayConfig, HubConfig};
    let hub_dir = residuum::config::paths::hub_dir(residuum_root);
    // First launch has no timezone yet. Loading the way startup does still
    // resolves the gateway address, including RESIDUUM_GATEWAY_PORT, so the
    // welcome message names the address the hub will actually bind.
    HubConfig::load_at_for_start(&hub_dir, residuum_root).map_or_else(
        |_| GatewayConfig::default().addr(),
        |hub| hub.gateway.addr(),
    )
}

#[derive(Parser)]
#[command(name = "residuum", about = "Personal AI agent gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Start the gateway (default when no subcommand is given)
    Serve(serve::ServeArgs),
    /// Display and tail log files
    Logs(logs::LogsArgs),
    /// Interactive or flag-driven configuration wizard
    Setup(setup::SetupArgs),
    /// Manage keys and tokens the agent can use in commands
    AgentKeys {
        #[command(subcommand)]
        command: agent_keys::AgentKeysCommand,
    },
    /// List, create, delete and control the hub's agents
    Agent {
        #[command(subcommand)]
        command: agent::AgentCommand,
    },
    /// Manage the A2A protocol listener
    A2a {
        #[command(subcommand)]
        command: a2a::A2aCommand,
    },
    /// Manage encrypted secret storage
    Secret {
        #[command(subcommand)]
        command: secret::SecretCommand,
    },
    /// Manage Microsoft Teams bot integration and ATK setup
    Teams {
        #[command(subcommand)]
        command: teams::TeamsCommand,
    },
    /// Stop the running hub and all its agents
    Stop(stop::StopArgs),
    /// Manage tracing and observability
    Tracing {
        #[command(subcommand)]
        command: tracing_cmd::TracingCommand,
    },
    /// Send a bug report with trace dump to the developer
    BugReport(bug_report::BugReportArgs),
    /// Send a short feedback message to the developer (no trace dump)
    Feedback(feedback::FeedbackArgs),
    /// Check for and install updates
    Update(update::UpdateArgs),
    /// Internal: supervise a self-update restart and roll back on failure.
    /// `serve::foreground::relaunch` spawns this itself; not meant to be
    /// run directly.
    #[command(hide = true)]
    UpdateWatchdog(update_watchdog::UpdateWatchdogArgs),
}

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

    let cli = Cli::parse();
    let command = cli
        .command
        .unwrap_or(Command::Serve(serve::ServeArgs::default()));

    match command {
        Command::Secret { command } => secret::run_secret_command(&command).await,
        Command::Teams { ref command } => teams::run_teams_command(command).await,
        Command::AgentKeys { ref command } => agent_keys::run_agent_keys_command(command).await,
        Command::A2a { ref command } => a2a::run_a2a_command(command).await,
        Command::Agent { ref command } => {
            residuum::util::tracing_init::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            agent::run_agent_command(command, &gateway_addr).await
        }
        Command::Logs(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            logs::run_logs_command(args).await
        }
        Command::Setup(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            setup::run_setup_command(args).await
        }
        Command::Stop(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            stop::run_stop_command(args).await
        }
        Command::Update(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            update::run_update_command(args).await
        }
        Command::UpdateWatchdog(ref args) => update_watchdog::run_update_watchdog(args),
        Command::Tracing { ref command } => {
            residuum::util::tracing_init::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            tracing_cmd::run_tracing_command(command, &gateway_addr).await
        }
        Command::BugReport(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            bug_report::run_bug_report_command(args, &gateway_addr).await
        }
        Command::Feedback(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            feedback::run_feedback_command(args, &gateway_addr).await
        }
        Command::Serve(ref args) => {
            if args.foreground {
                // Load the hub config to get the configured log level
                // ([tracing] is hub-owned).
                let log_level = {
                    residuum::config::default_hub_dir()
                        .and_then(|hub_dir| residuum::config::HubConfig::load_at(&hub_dir))
                        .map_or(residuum::config::LogLevel::default(), |hub| {
                            hub.tracing.log_level
                        })
                };
                residuum::util::tracing_init::init_daemon_tracing(args.foreground, log_level);
                serve::run_serve_foreground(args).await
            } else {
                serve::run_serve_command(args)
            }
        }
    }
}

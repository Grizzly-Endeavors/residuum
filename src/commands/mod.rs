mod a2a;
mod agent;
mod agent_keys;
mod bug_report;
mod feedback;
mod hub_client;
mod logs;
mod remote;
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
    Serve(serve::ServeArgs),
    Logs(logs::LogsArgs),
    Setup(setup::SetupArgs),
    AgentKeys {
        #[command(subcommand)]
        command: agent_keys::AgentKeysCommand,
    },
    Agent {
        #[command(subcommand)]
        command: agent::AgentCommand,
    },
    A2a {
        #[command(subcommand)]
        command: a2a::A2aCommand,
    },
    Remote {
        #[command(subcommand)]
        command: remote::RemoteCommand,
    },
    Secret {
        #[command(subcommand)]
        command: secret::SecretCommand,
    },
    Teams {
        #[command(subcommand)]
        command: teams::TeamsCommand,
    },
    Stop(stop::StopArgs),
    Tracing {
        #[command(subcommand)]
        command: tracing_cmd::TracingCommand,
    },
    BugReport(bug_report::BugReportArgs),
    Feedback(feedback::FeedbackArgs),
    Update(update::UpdateArgs),
    #[command(hide = true)]
    UpdateWatchdog(update_watchdog::UpdateWatchdogArgs),
}

pub async fn run() -> Result<(), FatalError> {
    drop(rustls::crypto::ring::default_provider().install_default());

    std::panic::set_hook(Box::new(|info| {
        tracing::error!(%info, "panic in spawned task");
        residuum::daemon::write_crash_note(&format!("PANIC: {info}"));
    }));

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
            residuum::logging::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            agent::run_agent_command(command, &gateway_addr).await
        }
        Command::Remote { ref command } => {
            residuum::logging::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            remote::run_remote_command(command, &gateway_addr).await
        }
        Command::Logs(ref args) => {
            residuum::logging::init_default_tracing();
            logs::run_logs_command(args).await
        }
        Command::Setup(ref args) => {
            residuum::logging::init_default_tracing();
            setup::run_setup_command(args).await
        }
        Command::Stop(ref args) => {
            residuum::logging::init_default_tracing();
            stop::run_stop_command(args).await
        }
        Command::Update(ref args) => {
            residuum::logging::init_default_tracing();
            update::run_update_command(args).await
        }
        Command::UpdateWatchdog(ref args) => update_watchdog::run_update_watchdog(args).await,
        Command::Tracing { ref command } => {
            residuum::logging::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            tracing_cmd::run_tracing_command(command, &gateway_addr).await
        }
        Command::BugReport(ref args) => {
            residuum::logging::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            bug_report::run_bug_report_command(args, &gateway_addr).await
        }
        Command::Feedback(ref args) => {
            residuum::logging::init_default_tracing();
            let gateway_addr = resolve_gateway_addr(&residuum::config::residuum_root()?);
            feedback::run_feedback_command(args, &gateway_addr).await
        }
        Command::Serve(ref args) => {
            if args.foreground {
                let log_level = {
                    residuum::config::default_hub_dir()
                        .and_then(|hub_dir| residuum::config::HubConfig::load_at(&hub_dir))
                        .map_or(residuum::config::LogLevel::default(), |hub| {
                            hub.tracing.log_level
                        })
                };
                residuum::logging::init_daemon_tracing(args.foreground, log_level);
                serve::run_serve_foreground(args).await
            } else {
                serve::run_serve_command(args)
            }
        }
    }
}

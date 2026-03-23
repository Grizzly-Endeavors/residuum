//! CLI subcommand dispatch using clap.

mod connect;
mod logs;
mod secret;
mod serve;
mod setup;
mod stop;
mod update;

use clap::Parser;

use residuum::util::FatalError;

fn resolve_gateway_addr(config_dir: &std::path::Path) -> String {
    use residuum::config::{Config, GatewayConfig};
    Config::load_at(config_dir).map_or_else(
        |_| GatewayConfig::default().addr(),
        |cfg| cfg.gateway.addr(),
    )
}

fn agent_label(agent_name: Option<&str>) -> String {
    agent_name.map_or("gateway".to_string(), |n| format!("agent '{n}'"))
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
    /// Connect a CLI client to a running gateway
    Connect(connect::ConnectArgs),
    /// Display and tail log files
    Logs(logs::LogsArgs),
    /// Interactive or flag-driven configuration wizard
    Setup(setup::SetupArgs),
    /// Manage encrypted secret storage
    Secret {
        #[command(subcommand)]
        command: secret::SecretCommand,
    },
    /// Stop a running gateway daemon
    Stop(stop::StopArgs),
    /// Check for and install updates
    Update(update::UpdateArgs),
    /// Manage named agent instances
    Agent {
        #[command(subcommand)]
        command: residuum::agent_registry::commands::AgentCommand,
    },
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
        Command::Secret { command } => secret::run_secret_command(&command),
        Command::Agent { command } => {
            residuum::agent_registry::commands::run_agent_command(&command)
        }
        Command::Connect(ref args) => {
            residuum::util::tracing_init::init_cli_tracing();
            let url = connect::resolve_url(args)?;
            connect::run_connect_command(&url, args.verbose).await
        }
        Command::Logs(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            logs::run_logs_command(args).await
        }
        Command::Setup(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            setup::run_setup_command(args)
        }
        Command::Stop(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            stop::run_stop_command(args).await
        }
        Command::Update(ref args) => {
            residuum::util::tracing_init::init_default_tracing();
            update::run_update_command(args).await
        }
        Command::Serve(ref args) => {
            if args.foreground {
                residuum::util::tracing_init::init_daemon_tracing(
                    args.debug,
                    args.agent.as_deref(),
                );
                serve::run_serve_foreground(args).await
            } else {
                serve::run_serve_command(args)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_label_none_returns_gateway() {
        assert_eq!(
            agent_label(None),
            "gateway",
            "None should produce 'gateway'"
        );
    }

    #[test]
    fn agent_label_some_returns_formatted_name() {
        assert_eq!(
            agent_label(Some("myagent")),
            "agent 'myagent'",
            "Some should produce \"agent '<name>'\""
        );
    }
}

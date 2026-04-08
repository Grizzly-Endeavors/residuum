//! Serve subcommand: start the gateway.

mod daemon;
mod foreground;

pub(super) use daemon::run_serve_command;
pub(super) use foreground::run_serve_foreground;

#[derive(clap::Args, Default)]
pub(super) struct ServeArgs {
    /// Run in foreground instead of daemonizing
    #[arg(long)]
    pub foreground: bool,
    /// Start the setup wizard before booting the gateway
    #[arg(long)]
    pub setup: bool,
    /// Target a named agent instance
    #[arg(long)]
    pub agent: Option<String>,
}

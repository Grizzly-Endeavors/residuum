mod daemon;
mod foreground;
mod startup_config;

pub(super) use daemon::run_serve_command;
pub(super) use foreground::run_serve_foreground;

#[derive(clap::Args, Default)]
pub(super) struct ServeArgs {
    #[arg(long)]
    pub foreground: bool,
    #[arg(long)]
    pub setup: bool,
}

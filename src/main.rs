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

mod commands;

#[tokio::main]
async fn main() {
    if let Err(e) = commands::run().await {
        // tracing::error goes to the log file; println is for the terminal user
        tracing::error!(error = %e, "fatal error");
        println!("error: {e}");
        std::process::exit(1);
    }
}

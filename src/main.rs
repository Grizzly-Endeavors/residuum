//! `Residuum`: personal AI agent gateway.
//!
//! Uses clap for CLI argument parsing. Run `residuum --help` for usage.

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

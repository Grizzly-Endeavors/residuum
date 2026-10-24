//! `Residuum`: personal AI agent gateway.
//!
//! Uses clap for CLI argument parsing. Run `residuum --help` for usage.

#[cfg(test)]
#[path = "../crates/core/src/testing/crypto.rs"]
mod crypto;

mod commands;

#[tokio::main]
async fn main() {
    // Before anything can rename the binary: a self-update moves it aside,
    // and a restart has to run the install path, not the moved file.
    residuum::hub::update::capture_install_path();
    if let Err(e) = commands::run().await {
        // tracing::error goes to the log file; println is for the terminal user
        tracing::error!(error = %e, "fatal error");
        println!("error: {e}");
        std::process::exit(1);
    }
}

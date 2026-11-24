#[cfg(test)]
#[path = "../crates/core/src/testing/crypto.rs"]
mod crypto;

mod commands;

#[tokio::main]
async fn main() {
    residuum::hub::update::capture_install_path();
    if let Err(e) = commands::run().await {
        tracing::error!(error = %e, "fatal error");
        println!("error: {e}");
        std::process::exit(1);
    }
}

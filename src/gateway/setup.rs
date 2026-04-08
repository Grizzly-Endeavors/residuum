//! Setup mode server for first-run configuration.
//!
//! When `Config::load()` fails (no valid config), this server starts a
//! minimal HTTP server with the config API and static web UI. Once the
//! user completes setup, it signals the main loop to retry loading config.

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::get;

use crate::config::Config;
use crate::util::FatalError;

use super::web::{self, ConfigApiState};

/// Outcome of the setup server.
pub enum SetupExit {
    /// User completed setup; config has been written.
    ConfigSaved,
    /// Shutdown requested.
    Shutdown,
}

/// Run the setup-mode HTTP server (config API + static files only).
///
/// Blocks until the user completes setup or the server is shut down.
/// Uses the default config directory (`~/.residuum/`).
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the server cannot bind or the config
/// directory cannot be determined.
pub async fn run_setup_server() -> Result<SetupExit, FatalError> {
    let config_dir = Config::config_dir()?;
    run_setup_server_at(config_dir).await
}

/// Run the setup-mode HTTP server writing config to `config_dir`.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the server cannot bind.
#[tracing::instrument(skip_all)]
pub async fn run_setup_server_at(config_dir: PathBuf) -> Result<SetupExit, FatalError> {
    let (setup_done_tx, mut setup_done_rx) = tokio::sync::watch::channel(false);
    let setup_done_tx = Arc::new(setup_done_tx);

    // During setup, workspace_dir defaults to config_dir/workspace since the user
    // hasn't configured a custom workspace yet.
    let workspace_dir = config_dir.join("workspace");
    let api_state = ConfigApiState {
        config_dir,
        workspace_dir,
        memory_dir: None,
        reload_tx: None,
        setup_done: Some(Arc::clone(&setup_done_tx)),
        secret_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

    let app = web::config_api_router(api_state).fallback(get(web::static_handler));

    // Resolve gateway bind/port from env vars and defaults (no config file during setup)
    let gateway_cfg = crate::config::resolve::resolve_default_gateway_config();
    if gateway_cfg.bind != "127.0.0.1" && gateway_cfg.bind != "localhost" {
        tracing::warn!(
            bind = %gateway_cfg.bind,
            "setup wizard is exposed on a non-loopback address with no authentication"
        );
    }
    let addr = gateway_cfg.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to bind setup server to {addr}: {e}")))?;

    println!("Setup wizard available at http://{addr}");
    tracing::info!(addr = %addr, "setup wizard listening");

    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        setup_done_rx.wait_for(|v| *v).await.ok();
    });

    if let Err(e) = server.await {
        tracing::error!(error = %e, "setup server error");
        return Ok(SetupExit::Shutdown);
    }

    Ok(SetupExit::ConfigSaved)
}

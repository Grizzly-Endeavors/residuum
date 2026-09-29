//! Setup mode server for first-run configuration.
//!
//! When `Config::load()` fails (no valid config), this server starts a
//! minimal HTTP server with the config API and static web UI. Once the
//! user completes setup, it signals the main loop to retry loading config.

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::get;

use crate::config::HubPaths;
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
/// Uses the default residuum root (`~/.residuum/`).
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the server cannot bind or the residuum
/// root cannot be determined.
pub async fn run_setup_server() -> Result<SetupExit, FatalError> {
    let residuum_root = crate::config::residuum_root()?;
    run_setup_server_at(residuum_root).await
}

/// Run the setup-mode HTTP server. Onboarding writes `hub/config.toml` and
/// the first agent's directory under `residuum_root`.
///
/// No agent exists yet, so `ConfigApiState`'s agent-scoped fields
/// (`config_dir`, `workspace_dir`, `agent_name`) are placeholders —
/// `api_complete_setup` creates the real agent directory itself, from the
/// name in its request body.
///
/// # Errors
///
/// Returns `FatalError::Gateway` if the server cannot bind.
#[tracing::instrument(skip_all)]
pub async fn run_setup_server_at(residuum_root: PathBuf) -> Result<SetupExit, FatalError> {
    let (setup_done_tx, mut setup_done_rx) = tokio::sync::watch::channel(false);
    let setup_done_tx = Arc::new(setup_done_tx);

    let hub = HubPaths::new(&residuum_root);
    let hub_dir = hub.root().to_path_buf();
    let placeholder_agent_dir = residuum_root.join("_pending-agent");
    let workspace_dir = placeholder_agent_dir.clone();
    let config_dir = placeholder_agent_dir.join("config");
    let checkpoints = Arc::new(
        crate::checkpoints::CheckpointEngine::new(
            workspace_dir.clone(),
            config_dir.clone(),
            hub_dir.clone(),
            &hub.checkpoints_dir(),
            None,
        )
        .map_err(|e| FatalError::Gateway(format!("failed to open checkpoint repositories: {e}")))?,
    );
    let api_state = ConfigApiState {
        hub_dir,
        config_dir,
        agent_name: String::new(),
        workspace_dir,
        memory_dir: None,
        reload_tx: None,
        setup_done: Some(Arc::clone(&setup_done_tx)),
        secret_lock: Arc::new(tokio::sync::Mutex::new(())),
        checkpoints,
    };

    let app = web::config_api_router(api_state)
        .fallback(get(web::static_handler))
        .layer(axum::middleware::from_fn(
            super::cross_site::reject_cross_site_requests,
        ));

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

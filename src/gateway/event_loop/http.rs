//! HTTP server setup and adapter spawning in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::Config;
use crate::error::ResiduumError;
use crate::gateway::types::{GatewayState, ReloadSignal, ServerCommand};

use crate::gateway::web;
use crate::gateway::ws::ws_handler;

/// Bundled senders for spawning a chat adapter (Discord or Telegram).
pub struct AdapterSenders {
    pub publisher: crate::bus::Publisher,
    pub reload: tokio::sync::watch::Sender<ReloadSignal>,
    pub command: mpsc::Sender<ServerCommand>,
}

/// Lifecycle handles returned from spawning chat adapters.
pub struct AdapterHandles {
    pub discord_handle: Option<tokio::task::JoinHandle<()>>,
    pub discord_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub telegram_handle: Option<tokio::task::JoinHandle<()>>,
    pub telegram_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

/// Build the gateway app with WebSocket, webhook, cloud, update, and config API routes.
pub fn build_gateway_app(
    state: GatewayState,
    cfg: &Config,
    config_api_state: web::ConfigApiState,
    update_api_state: web::update::UpdateApiState,
) -> axum::Router {
    use axum::routing::{get, post};

    let webhook_router = if cfg.webhooks.is_empty() {
        None
    } else {
        let mut endpoints = std::collections::HashMap::new();
        for (name, entry) in &cfg.webhooks {
            endpoints.insert(
                name.clone(),
                crate::interfaces::webhook::WebhookEndpointState {
                    secret: entry.secret.clone(),
                    format: entry.format.clone(),
                    content_fields: entry.content_fields.clone(),
                },
            );
        }
        let webhook_state = crate::interfaces::webhook::WebhookState {
            inbound_tx: state.inbound_tx.clone(),
            webhooks: endpoints,
        };
        Some(
            axum::Router::new()
                .route(
                    "/webhook/{name}",
                    axum::routing::post(crate::interfaces::webhook::webhook_handler),
                )
                .with_state(webhook_state),
        )
    };

    let cloud_router = {
        let cloud_state = web::cloud::CloudApiState {
            config_dir: config_api_state.config_dir.clone(),
            reload_tx: state.reload_tx.clone(),
            tunnel_status_rx: state.tunnel_status_rx.clone(),
            secret_lock: Arc::clone(&config_api_state.secret_lock),
        };
        axum::Router::new()
            .route("/api/cloud/status", get(web::cloud::api_cloud_status))
            .route("/cloud/callback", get(web::cloud::cloud_callback))
            .route(
                "/api/cloud/disconnect",
                post(web::cloud::api_cloud_disconnect),
            )
            .with_state(cloud_state)
    };

    let update_router = axum::Router::new()
        .route("/api/update/status", get(web::update::api_update_status))
        .route("/api/update/check", post(web::update::api_update_check))
        .route("/api/update/apply", post(web::update::api_update_apply))
        .route("/api/update/restart", post(web::update::api_update_restart))
        .with_state(update_api_state);

    let mut app = axum::Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);
    if let Some(wh) = webhook_router {
        app = app.merge(wh);
    }
    app.merge(cloud_router)
        .merge(update_router)
        .merge(web::config_api_router(config_api_state))
        .fallback(web::static_handler)
}

/// Bind the HTTP server and spawn it as a background task.
///
/// # Errors
/// Returns `ResiduumError` if the listener cannot bind to the configured address.
pub async fn spawn_http_server(
    cfg: &Config,
    app: axum::Router,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
) -> Result<tokio::task::JoinHandle<()>, ResiduumError> {
    let addr = cfg.gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| ResiduumError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");
    if cfg.gateway.bind != "127.0.0.1" && cfg.gateway.bind != "localhost" {
        tracing::warn!(
            bind = %cfg.gateway.bind,
            "web UI is exposed on a non-loopback address with no authentication"
        );
    }

    let mut shutdown_rx = shutdown_tx.subscribe();
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|v| *v).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    });
    Ok(handle)
}

/// Spawn Discord and Telegram adapters if configured.
pub fn spawn_adapters(
    cfg: &Config,
    discord: AdapterSenders,
    telegram: AdapterSenders,
    tz: chrono_tz::Tz,
) -> AdapterHandles {
    let (mut discord_handle, mut discord_shutdown_tx) = (None, None);
    if let Some(ref discord_cfg) = cfg.discord {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::discord::DiscordInterface::new(
            discord_cfg.clone(),
            discord.publisher,
            cfg.workspace_dir.clone(),
            discord.reload,
            discord.command,
            tz,
            rx,
        );
        discord_handle = Some(crate::spawn::spawn_monitored("discord", async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "discord interface failed");
            }
        }));
        discord_shutdown_tx = Some(tx);
        tracing::info!("discord interface started (DM-only mode)");
    }

    let (mut telegram_handle, mut telegram_shutdown_tx) = (None, None);
    if let Some(ref telegram_cfg) = cfg.telegram {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let iface = crate::interfaces::telegram::TelegramInterface::new(
            telegram_cfg.clone(),
            telegram.publisher,
            cfg.workspace_dir.clone(),
            telegram.reload,
            telegram.command,
            tz,
            rx,
        );
        telegram_handle = Some(crate::spawn::spawn_monitored("telegram", async move {
            if let Err(e) = iface.start().await {
                tracing::error!(error = %e, "telegram interface failed");
            }
        }));
        telegram_shutdown_tx = Some(tx);
        tracing::info!("telegram interface started (DM-only mode)");
    }

    AdapterHandles {
        discord_handle,
        discord_shutdown_tx,
        telegram_handle,
        telegram_shutdown_tx,
    }
}

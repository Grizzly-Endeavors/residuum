//! Degraded gateway mode: serves the web UI and error messages when full init fails.
//!
//! Entered as a last resort when config rollback fails and the gateway cannot
//! fully initialize. Serves the config editor so users can fix settings
//! in-browser, and responds to every WebSocket message with the error.

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, watch};

use crate::gateway::protocol::{ClientMessage, ServerMessage};

use super::GatewayExit;
use super::ReloadSignal;
use super::web::{self, ConfigApiState};

/// Shared state for the degraded gateway.
#[derive(Clone)]
struct DegradedState {
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_tx: watch::Sender<ReloadSignal>,
    error_message: String,
}

/// Run a minimal gateway that serves the web UI and error messages.
///
/// Binds on the given address (or falls back to `127.0.0.1:7700`), serves the
/// config editor and static assets, and responds to every WebSocket message
/// with the error. Handles `/reload` to retry full initialization.
///
/// # Errors
///
/// Returns `ResiduumError` if the server cannot bind.
pub async fn run_degraded_gateway(
    error_message: String,
    config_dir: std::path::PathBuf,
    bind_addr: Option<String>,
) -> super::GatewayExit {
    let addr = bind_addr.as_deref().unwrap_or("127.0.0.1:7700");

    tracing::warn!(addr = %addr, "entering degraded gateway mode");
    eprintln!("warning: gateway running in degraded mode — {error_message}");

    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<ServerMessage>(64);
    let (reload_tx, mut reload_rx) = watch::channel(ReloadSignal::None);

    let state = DegradedState {
        broadcast_tx: broadcast_tx.clone(),
        reload_tx: reload_tx.clone(),
        error_message: error_message.clone(),
    };

    let app = axum::Router::new()
        .route("/ws", get(degraded_ws_handler))
        .with_state(state)
        .merge(web::config_api_router(ConfigApiState {
            config_dir,
            memory_dir: None,
            reload_tx: Some(reload_tx),
            setup_done: None,
        }))
        .fallback(web::static_handler);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) => {
            // If we can't even bind, try the default fallback
            tracing::error!(addr = %addr, error = %err, "degraded mode: failed to bind");
            if addr == "127.0.0.1:7700" {
                tracing::error!("degraded mode: cannot bind, shutting down");
                return GatewayExit::Shutdown;
            }
            match tokio::net::TcpListener::bind("127.0.0.1:7700").await {
                Ok(l) => {
                    tracing::info!("degraded mode: bound to fallback 127.0.0.1:7700");
                    l
                }
                Err(fallback_err) => {
                    tracing::error!(error = %fallback_err, "degraded mode: fallback bind also failed, cannot recover");
                    return GatewayExit::Shutdown;
                }
            }
        }
    };

    tracing::info!(addr = %addr, "degraded gateway listening");

    // Broadcast the degraded mode message to any clients that connect
    broadcast_tx
        .send(ServerMessage::DegradedMode {
            message: error_message,
        })
        .ok();

    let mut shutdown_rx = reload_rx.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx
                    .wait_for(|v| *v != ReloadSignal::None)
                    .await
                    .ok();
            })
            .await
        {
            tracing::error!(error = %e, "degraded gateway server error");
        }
    });

    // Wait for reload signal or SIGTERM
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

    tokio::select! {
        _ = reload_rx.wait_for(|v| *v != ReloadSignal::None) => {
            server_handle.abort();
            tracing::info!("degraded mode: reload requested, retrying full initialization");
            GatewayExit::Reload
        }
        () = async {
            match sigterm.as_mut() {
                Some(s) => { s.recv().await; }
                None => std::future::pending().await,
            }
        } => {
            server_handle.abort();
            tracing::info!("degraded mode: received SIGTERM, shutting down");
            GatewayExit::Shutdown
        }
    }
}

/// Axum handler for WebSocket upgrades in degraded mode.
async fn degraded_ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<DegradedState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| degraded_handle_connection(socket, state))
}

/// Handle a WebSocket connection in degraded mode.
///
/// Sends the degraded mode error immediately, then processes client messages.
/// Responds to all messages with the error. Handles Reload and Ping specially.
async fn degraded_handle_connection(socket: WebSocket, state: DegradedState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send the degraded mode message immediately on connect
    let greeting = ServerMessage::DegradedMode {
        message: state.error_message.clone(),
    };
    if let Ok(json) = serde_json::to_string(&greeting) {
        ws_tx.send(WsMessage::text(json)).await.ok();
    }

    // Forward broadcast messages to this client
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    let fwd_handle = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            let Ok(json) = serde_json::to_string(&msg) else {
                continue;
            };
            if ws_tx.send(WsMessage::text(json)).await.is_err() {
                break;
            }
        }
    });

    // Read loop
    while let Some(frame) = ws_rx.next().await {
        let raw = match frame {
            Ok(WsMessage::Text(txt)) => txt,
            Ok(WsMessage::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };

        let client_msg: ClientMessage = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match client_msg {
            ClientMessage::Reload => {
                tracing::info!("degraded mode: reload requested by client");
                state.broadcast_tx.send(ServerMessage::Reloading).ok();
                state.reload_tx.send(ReloadSignal::Root).ok();
            }
            ClientMessage::Ping => {
                state.broadcast_tx.send(ServerMessage::Pong).ok();
            }
            ClientMessage::SendMessage { .. }
            | ClientMessage::SetVerbose { .. }
            | ClientMessage::ServerCommand { .. }
            | ClientMessage::InboxAdd { .. } => {
                // For any other message, respond with the error
                state
                    .broadcast_tx
                    .send(ServerMessage::Error {
                        reply_to: None,
                        message: format!(
                            "gateway is in degraded mode: {}\nUse /reload after fixing the config.",
                            state.error_message
                        ),
                    })
                    .ok();
            }
        }
    }

    fwd_handle.abort();
    tracing::debug!("degraded mode: client disconnected");
}

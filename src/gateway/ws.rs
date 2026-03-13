//! WebSocket connection handler.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};

use crate::gateway::protocol::{ClientMessage, ServerMessage};
use crate::interfaces::types::{InboundMessage, MessageOrigin, RoutedMessage};
use crate::interfaces::websocket::WsReplyHandle;

use crate::gateway::types::GatewayState;

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
pub(super) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

/// Handle a single WebSocket connection.
///
/// Splits the socket into read/write halves. A forwarding task reads from the
/// broadcast channel and sends all events to the client. Verbose filtering
/// is handled client-side. The read loop processes incoming client messages.
async fn handle_connection(socket: WebSocket, state: GatewayState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut broadcast_rx = state.broadcast_tx.subscribe();

    // Forwarding task: broadcast → WebSocket client
    let fwd_handle = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            let Ok(json) = serde_json::to_string(&msg) else {
                tracing::warn!("failed to serialize server message");
                continue;
            };

            if ws_tx.send(WsMessage::text(json)).await.is_err() {
                break; // client disconnected
            }
        }
    });

    // Read loop: WebSocket client → inbound channel
    while let Some(frame) = ws_rx.next().await {
        let raw = match frame {
            Ok(WsMessage::Text(txt)) => txt,
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => continue, // ignore binary, ping, pong
            Err(e) => {
                tracing::debug!(error = %e, "websocket read error");
                break;
            }
        };

        let client_msg: ClientMessage = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(e) => {
                let err_msg = ServerMessage::Error {
                    reply_to: None,
                    message: format!("malformed message: {e}"),
                };
                tracing::warn!(error = %e, "malformed WebSocket message from client");
                if state.broadcast_tx.send(err_msg).is_err() {
                    tracing::trace!("no broadcast receivers for error");
                }
                continue;
            }
        };

        if !handle_client_message(client_msg, &state).await {
            break;
        }
    }

    // Clean up: abort forwarding task when client disconnects
    fwd_handle.abort();
    tracing::debug!("client disconnected");
}

/// Dispatch a single client message. Returns `false` to break the read loop.
async fn handle_client_message(msg: ClientMessage, state: &GatewayState) -> bool {
    match msg {
        ClientMessage::SendMessage { id, content } => {
            let origin = MessageOrigin {
                interface: "websocket".to_string(),
                sender_name: "ws-client".to_string(),
                sender_id: "ws-client".to_string(),
            };
            let inbound = InboundMessage {
                id: id.clone(),
                content,
                origin,
                timestamp: chrono::Utc::now(),
                images: vec![],
            };
            let reply = Arc::new(WsReplyHandle::new(state.broadcast_tx.clone(), id));
            let routed = RoutedMessage {
                message: inbound,
                reply,
            };
            if state.inbound_tx.send(routed).await.is_err() {
                tracing::warn!("inbound channel closed, dropping message");
                return false;
            }
        }
        ClientMessage::SetVerbose { .. } => {
            // Verbose filtering is handled client-side; acknowledge silently.
        }
        ClientMessage::Ping => {
            // Send pong through broadcast (all clients will filter; only this
            // one would care, but pong is cheap and non-verbose)
            if state.broadcast_tx.send(ServerMessage::Pong).is_err() {
                tracing::trace!("no broadcast receivers for pong");
            }
        }
        ClientMessage::Reload => {
            tracing::info!("reload requested by client");
            state
                .broadcast_tx
                .send(ServerMessage::Notice {
                    message: "reloading configuration...".to_string(),
                })
                .ok();
            state
                .reload_tx
                .send(crate::gateway::types::ReloadSignal::Root)
                .ok();
        }
        ClientMessage::ServerCommand { name, args } => {
            tracing::info!(command = %name, "server command from client");
            state
                .command_tx
                .send(crate::gateway::types::ServerCommand {
                    name,
                    args,
                    reply_tx: None,
                })
                .await
                .ok();
        }
        ClientMessage::InboxAdd { body } => {
            tracing::info!("inbox add requested by client");
            let dir = state.inbox_dir.clone();
            let tz = state.tz;
            let tx = state.broadcast_tx.clone();
            tokio::spawn(async move {
                let title: String = body
                    .lines()
                    .next()
                    .unwrap_or("Inbox message")
                    .chars()
                    .take(60)
                    .collect();
                match crate::inbox::quick_add(&dir, &title, &body, "cli", tz).await {
                    Ok(_filename) => {
                        tx.send(ServerMessage::Notice {
                            message: "[inbox] item added".to_string(),
                        })
                        .ok();
                    }
                    Err(e) => {
                        tx.send(ServerMessage::Error {
                            reply_to: None,
                            message: format!("inbox add failed: {e}"),
                        })
                        .ok();
                    }
                }
            });
        }
    }
    true
}

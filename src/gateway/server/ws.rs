//! WebSocket connection handler.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};

use crate::channels::types::{InboundMessage, MessageOrigin, RoutedMessage};
use crate::channels::websocket::WsReplyHandle;
use crate::gateway::protocol::{ClientMessage, ServerMessage};

use super::GatewayState;

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
                // Send directly to this client, not broadcast
                if state.broadcast_tx.send(err_msg).is_err() {
                    tracing::trace!("no broadcast receivers for error");
                }
                continue;
            }
        };

        match client_msg {
            ClientMessage::SendMessage { id, content } => {
                let origin = MessageOrigin {
                    channel: "websocket".to_string(),
                    sender_name: "ws-client".to_string(),
                    sender_id: "ws-client".to_string(),
                };
                let inbound = InboundMessage {
                    id: id.clone(),
                    content,
                    origin,
                    timestamp: chrono::Utc::now(),
                };
                let reply = Arc::new(WsReplyHandle::new(state.broadcast_tx.clone(), id));
                let routed = RoutedMessage {
                    message: inbound,
                    reply,
                };
                if state.inbound_tx.send(routed).await.is_err() {
                    tracing::warn!("inbound channel closed, dropping message");
                    break;
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
                // Notify all connected clients before the connection drops
                state.broadcast_tx.send(ServerMessage::Reloading).ok();
                // Signal the main loop and HTTP server
                state.reload_sender.send(true).ok();
            }
            ClientMessage::Observe => {
                tracing::info!("observe requested by client");
                state.observe_notify.notify_one();
            }
            ClientMessage::Reflect => {
                tracing::info!("reflect requested by client");
                state.reflect_notify.notify_one();
            }
        }
    }

    // Clean up: abort forwarding task when client disconnects
    fwd_handle.abort();
    tracing::debug!("client disconnected");
}

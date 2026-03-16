//! WebSocket connection handler.

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::bus::{EndpointName, TopicId};
use crate::gateway::protocol::{ClientMessage, ServerMessage};
use crate::gateway::types::GatewayState;
use crate::interfaces::types::MessageOrigin;
use crate::interfaces::websocket::subscriber::translate_bus_event;

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
pub(super) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

/// Handle a single WebSocket connection.
///
/// Each connection subscribes to `Interactive("ws")` and `SystemBroadcast` on
/// the bus. A local channel carries per-connection messages (pong, errors,
/// inbox confirmations) that bypass the bus. A forwarding task merges all
/// three sources and writes `ServerMessage` frames to the WebSocket.
async fn handle_connection(socket: WebSocket, state: GatewayState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Subscribe to bus topics for this connection
    let interactive_sub = state
        .bus_handle
        .subscribe(TopicId::Interactive(EndpointName::from("ws")))
        .await;
    let broadcast_sub = state.bus_handle.subscribe(TopicId::SystemBroadcast).await;

    let (mut interactive_sub, mut broadcast_sub) = match (interactive_sub, broadcast_sub) {
        (Ok(i), Ok(b)) => (i, b),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(error = %e, "failed to subscribe to bus topics for ws connection");
            return;
        }
    };

    // Local channel for per-connection messages (pong, errors, inbox responses)
    let (local_tx, mut local_rx) = mpsc::unbounded_channel::<ServerMessage>();

    // Forwarding task: bus subscribers + local channel → WebSocket client
    let fwd_handle = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                event = interactive_sub.recv() => {
                    match event {
                        Some(e) => translate_bus_event(e),
                        None => break,
                    }
                }
                event = broadcast_sub.recv() => {
                    match event {
                        Some(e) => translate_bus_event(e),
                        None => break,
                    }
                }
                msg = local_rx.recv() => {
                    match msg {
                        Some(m) => Some(m),
                        None => break,
                    }
                }
            };

            if let Some(msg) = msg {
                let Ok(json) = serde_json::to_string(&msg) else {
                    tracing::warn!("failed to serialize server message");
                    continue;
                };
                if ws_tx.send(WsMessage::text(json)).await.is_err() {
                    break; // client disconnected
                }
            }
        }
    });

    // Read loop: WebSocket client → bus / local channel
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
                drop(local_tx.send(err_msg));
                continue;
            }
        };

        if !handle_client_message(client_msg, &state, &local_tx).await {
            break;
        }
    }

    // Clean up: abort forwarding task when client disconnects
    fwd_handle.abort();
    tracing::debug!("client disconnected");
}

/// Dispatch a single client message. Returns `false` to break the read loop.
async fn handle_client_message(
    msg: ClientMessage,
    state: &GatewayState,
    local_tx: &mpsc::UnboundedSender<ServerMessage>,
) -> bool {
    match msg {
        ClientMessage::SendMessage {
            id,
            content,
            images,
        } => {
            let origin = MessageOrigin {
                endpoint: "ws".to_string(),
                sender_name: "ws-client".to_string(),
                sender_id: "ws-client".to_string(),
            };
            let msg_event = crate::bus::MessageEvent {
                id: id.clone(),
                content,
                origin,
                timestamp: crate::time::now_local(chrono_tz::UTC),
                images,
            };
            if let Err(e) = state
                .publisher
                .publish_typed(crate::bus::topics::UserMessage, msg_event)
                .await
            {
                tracing::warn!(error = %e, "failed to publish message to bus");
                return false;
            }
        }
        ClientMessage::SetVerbose { .. } => {
            // Verbose filtering is handled client-side; acknowledge silently.
        }
        ClientMessage::Ping => {
            drop(local_tx.send(ServerMessage::Pong));
        }
        ClientMessage::Reload => {
            tracing::info!("reload requested by client");
            drop(local_tx.send(ServerMessage::Notice {
                message: "reloading configuration...".to_string(),
            }));
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
            let tx = local_tx.clone();
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
                        drop(tx.send(ServerMessage::Notice {
                            message: "[inbox] item added".to_string(),
                        }));
                    }
                    Err(e) => {
                        drop(tx.send(ServerMessage::Error {
                            reply_to: None,
                            message: format!("inbox add failed: {e}"),
                        }));
                    }
                }
            });
        }
    }
    true
}

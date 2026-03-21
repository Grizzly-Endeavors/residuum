//! WebSocket connection handler.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::bus::EndpointName;
use crate::gateway::protocol::{ClientMessage, ServerMessage};
use crate::gateway::types::GatewayState;
use crate::interfaces::types::MessageOrigin;
use crate::interfaces::websocket::subscriber::WsSubscribers;
use crate::models::ImageData;

/// Axum handler that upgrades an HTTP request to a WebSocket connection.
pub(super) async fn ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<GatewayState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

/// Handle a single WebSocket connection.
///
/// Each connection subscribes to typed topics (`Endpoint` for responses, tool
/// activity, turn lifecycle, and intermediates; `Notification` for system
/// notices and errors) on the bus. A local channel
/// carries per-connection messages (pong, errors, inbox confirmations) that
/// bypass the bus. A forwarding task merges all sources and writes
/// `ServerMessage` frames to the WebSocket.
///
/// Verbose filtering is server-side: `ToolCall` and `ToolResult` events are
/// dropped in the forwarding task when verbose mode is off.
async fn handle_connection(socket: WebSocket, state: GatewayState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Subscribe to typed bus topics for this connection
    let mut subs = match WsSubscribers::new(&state.bus_handle, EndpointName::from("ws")).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to bus topics for ws connection");
            return;
        }
    };

    // Local channel for per-connection messages (pong, errors, inbox responses)
    let (local_tx, mut local_rx) = mpsc::unbounded_channel::<ServerMessage>();

    // Per-connection verbose flag shared between read loop and forwarding task
    let verbose = Arc::new(AtomicBool::new(false));
    let verbose_fwd = Arc::clone(&verbose);

    // Forwarding task: bus subscribers + local channel → WebSocket client
    let fwd_handle = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                bus_msg = subs.recv() => {
                    match bus_msg {
                        Some(m) => Some(m),
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
                // Skip tool events when verbose mode is off
                if !verbose_fwd.load(Ordering::Relaxed)
                    && matches!(
                        msg,
                        ServerMessage::ToolCall { .. } | ServerMessage::ToolResult { .. }
                    )
                {
                    continue;
                }

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

        if !handle_client_message(client_msg, &state, &local_tx, &verbose).await {
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
    verbose: &AtomicBool,
) -> bool {
    match msg {
        ClientMessage::SendMessage {
            id,
            content,
            images,
        } => {
            if !images.is_empty()
                && let Err(reason) = validate_images(&images)
            {
                drop(local_tx.send(ServerMessage::Error {
                    reply_to: Some(id),
                    message: reason,
                }));
                return true;
            }

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
                .publish(crate::bus::topics::UserMessage, msg_event)
                .await
            {
                tracing::warn!(error = %e, "failed to publish message to bus");
                return false;
            }
        }
        ClientMessage::SetVerbose { enabled } => {
            verbose.store(enabled, Ordering::Relaxed);
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

/// Maximum number of images per message.
const MAX_IMAGES: usize = 5;

/// Maximum raw image size in bytes (5 MB).
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Allowed MIME types for image uploads.
const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Validate image attachments before publishing to the bus.
///
/// Enforces the same limits as the client: max 5 images, 5 MB each,
/// and only JPEG/PNG/GIF/WebP MIME types.
///
/// # Errors
///
/// Returns a human-readable error describing the first violation found.
fn validate_images(images: &[ImageData]) -> Result<(), String> {
    if images.len() > MAX_IMAGES {
        return Err(format!(
            "too many images: {} (max {MAX_IMAGES})",
            images.len()
        ));
    }

    for img in images {
        if !ALLOWED_MIME_TYPES.contains(&img.media_type.as_str()) {
            return Err(format!(
                "unsupported image type: {} (allowed: {})",
                img.media_type,
                ALLOWED_MIME_TYPES.join(", ")
            ));
        }

        // Estimate raw bytes from base64 length: every 4 base64 chars = 3 bytes
        let estimated_bytes = img.data.len() * 3 / 4;
        if estimated_bytes > MAX_IMAGE_BYTES {
            return Err(format!(
                "image too large: ~{estimated_bytes} bytes (max {MAX_IMAGE_BYTES})"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_verbose_updates_flag() {
        let flag = AtomicBool::new(false);

        flag.store(true, Ordering::Relaxed);
        assert!(
            flag.load(Ordering::Relaxed),
            "should be true after storing true"
        );

        flag.store(false, Ordering::Relaxed);
        assert!(
            !flag.load(Ordering::Relaxed),
            "should be false after storing false"
        );
    }

    fn make_image(media_type: &str, data_len: usize) -> ImageData {
        ImageData {
            media_type: media_type.to_string(),
            data: "A".repeat(data_len),
        }
    }

    #[test]
    fn validate_images_accepts_valid_input() {
        let images = vec![make_image("image/jpeg", 100), make_image("image/png", 200)];
        assert!(
            validate_images(&images).is_ok(),
            "valid images should pass validation"
        );
    }

    #[test]
    fn validate_images_accepts_empty() {
        assert!(
            validate_images(&[]).is_ok(),
            "empty images should pass validation"
        );
    }

    #[test]
    fn validate_images_rejects_too_many() {
        let images: Vec<_> = (0..6).map(|_| make_image("image/png", 100)).collect();
        let result = validate_images(&images);
        assert!(result.is_err(), "should reject more than 5 images");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("too many")),
            "error should mention 'too many'"
        );
    }

    #[test]
    fn validate_images_rejects_bad_mime_type() {
        let images = vec![make_image("image/bmp", 100)];
        let result = validate_images(&images);
        assert!(result.is_err(), "should reject unsupported MIME type");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("unsupported")),
            "error should mention 'unsupported'"
        );
    }

    #[test]
    fn validate_images_rejects_oversized() {
        // 7 MB worth of base64 (~9.3M base64 chars encode ~7M raw bytes)
        let oversized_len = 7 * 1024 * 1024 * 4 / 3;
        let images = vec![make_image("image/jpeg", oversized_len)];
        let result = validate_images(&images);
        assert!(result.is_err(), "should reject oversized image");
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("too large")),
            "error should mention 'too large'"
        );
    }

    #[test]
    fn validate_images_allows_all_mime_types() {
        for mime in ALLOWED_MIME_TYPES {
            let images = vec![make_image(mime, 100)];
            assert!(validate_images(&images).is_ok(), "{mime} should be allowed");
        }
    }
}

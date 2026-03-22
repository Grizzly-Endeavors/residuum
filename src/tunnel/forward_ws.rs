//! WebSocket connection forwarding to the local residuum instance.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, warn};

use super::protocol::TunnelFrame;
use super::{TunnelSink, send_frame};

/// Channel capacity for messages flowing from the tunnel to local WebSocket.
const LOCAL_WS_CHANNEL_CAPACITY: usize = 64;

/// Handle a `WsOpen` frame by connecting to the local WebSocket endpoint.
///
/// On success, sends a `WsOpenResult { success: true }` through the tunnel and
/// returns an `mpsc::Sender` for forwarding messages from the tunnel to the
/// local WebSocket. On failure, sends `WsOpenResult { success: false }` and
/// returns `None`.
///
/// # Errors
///
/// This function does not return errors directly. Connection failures are
/// communicated back through the tunnel as `WsOpenResult` frames.
pub(super) async fn handle_ws_open(
    port: u16,
    channel_id: String,
    path: String,
    headers: HashMap<String, String>,
    tunnel_tx: Arc<Mutex<TunnelSink>>,
) -> Option<mpsc::Sender<String>> {
    let url = format!("ws://localhost:{port}{path}");
    debug!(channel_id, url, "opening local WebSocket connection");

    // Build the local WS connect request with forwarded headers.
    let host = format!("localhost:{port}");
    let mut request = match super::build_ws_upgrade_request(&url, &host) {
        Ok(r) => r,
        Err(e) => {
            warn!(channel_id, error = %e, "failed to build local WS request");
            send_ws_open_result(&tunnel_tx, &channel_id, false).await;
            return None;
        }
    };

    // Add forwarded headers (skip WebSocket handshake headers).
    for (name, value) in &headers {
        if !super::is_hop_by_hop(name)
            && let Ok(v) = ws_http::HeaderValue::from_str(value)
            && let Ok(n) = ws_http::HeaderName::from_bytes(name.as_bytes())
        {
            request.headers_mut().insert(n, v);
        }
    }

    let (ws_stream, _response) = match tokio_tungstenite::connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(channel_id, url, error = %e, "failed to connect to local WebSocket");
            send_ws_open_result(&tunnel_tx, &channel_id, false).await;
            return None;
        }
    };

    // Connection succeeded — notify the relay.
    send_ws_open_result(&tunnel_tx, &channel_id, true).await;
    debug!(channel_id, url, "local WebSocket channel established");
    let (mut local_write, mut local_read) = ws_stream.split();
    // Channel for messages flowing from tunnel → local WS.
    let (tx, mut rx) = mpsc::channel::<String>(LOCAL_WS_CHANNEL_CAPACITY);

    // Task: read from local WS, send through tunnel.
    let tunnel_tx_reader = Arc::clone(&tunnel_tx);
    let ch_id_reader = channel_id.clone();
    tokio::spawn(async move {
        while let Some(msg) = local_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let frame = TunnelFrame::WsMessage {
                        channel_id: ch_id_reader.clone(),
                        data: text.to_string(),
                    };
                    if let Err(e) = send_frame(&tunnel_tx_reader, &frame).await {
                        warn!(channel_id = ch_id_reader, error = %e, "failed to forward local WS message to tunnel");
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!(channel_id = ch_id_reader, "local WebSocket closed");
                    break;
                }
                Ok(_) => {
                    // Ignore binary, ping, pong frames from local.
                }
                Err(e) => {
                    warn!(channel_id = ch_id_reader, error = %e, "local WebSocket read error");
                    break;
                }
            }
        }

        // Notify relay that the local WS has closed.
        let close_frame = TunnelFrame::WsClose {
            channel_id: ch_id_reader.clone(),
        };
        if let Err(e) = send_frame(&tunnel_tx_reader, &close_frame).await {
            warn!(channel_id = ch_id_reader, error = %e, "failed to send WsClose to relay; relay may hold a zombie channel");
        }
    });

    // Task: read from mpsc rx, send to local WS.
    let ch_id_writer = channel_id;
    tokio::spawn(async move {
        loop {
            let Some(data) = rx.recv().await else {
                debug!(channel_id = ch_id_writer, "tunnel→local WS channel closed");
                break;
            };
            if let Err(e) = local_write.send(Message::Text(data.into())).await {
                warn!(channel_id = ch_id_writer, error = %e, "failed to forward tunnel message to local WS");
                break;
            }
        }
    });

    Some(tx)
}

/// Send a `WsOpenResult` frame through the tunnel.
async fn send_ws_open_result(tunnel_tx: &Arc<Mutex<TunnelSink>>, channel_id: &str, success: bool) {
    let frame = TunnelFrame::WsOpenResult {
        channel_id: channel_id.to_string(),
        success,
    };
    if let Err(e) = send_frame(tunnel_tx, &frame).await {
        warn!(channel_id, success, error = %e, "failed to send WsOpenResult");
    }
}

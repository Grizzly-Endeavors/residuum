//! Core tunnel connection logic with automatic reconnection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, error, info, warn};

use super::TunnelStatus;
use super::forward_http;
use super::forward_ws;
use super::forward_ws::TunnelSink;
use super::protocol::TunnelFrame;
use crate::config::CloudConfig;

/// Minimum backoff duration between reconnection attempts.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff duration between reconnection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Calculate the next backoff duration by doubling the current value, capped at
/// [`MAX_BACKOFF`].
#[must_use]
fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > MAX_BACKOFF {
        MAX_BACKOFF
    } else {
        doubled
    }
}

/// Start the tunnel client, maintaining a persistent connection to the cloud
/// relay with exponential backoff reconnection.
///
/// The tunnel forwards HTTP requests and WebSocket connections from the relay
/// to the local residuum instance running on `cfg.local_port`.
///
/// # Errors
///
/// This function runs until the shutdown signal is received. Transient
/// connection errors are logged and retried automatically.
pub(crate) async fn start_tunnel(
    cfg: CloudConfig,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: watch::Sender<TunnelStatus>,
) {
    let client = reqwest::Client::new();
    let mut backoff = MIN_BACKOFF;

    loop {
        // Check for shutdown before attempting connection.
        if *shutdown_rx.borrow() {
            info!("tunnel shutting down before reconnect");
            status_tx.send(TunnelStatus::Disconnected).ok();
            return;
        }

        status_tx.send(TunnelStatus::Connecting).ok();
        info!(url = %cfg.relay_url, "connecting to relay at {}", cfg.relay_url);

        let request = match build_ws_request(&cfg) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "failed to build WebSocket request");
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        let (ws_stream, _response) = match tokio_tungstenite::connect_async(request).await {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "failed to connect to relay, retrying in {:?}", backoff);
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        let (write, mut read) = ws_stream.split();
        let write = Arc::new(Mutex::new(write));

        // Wait for the Connected frame.
        let Some(connected) = wait_for_connected(&mut read).await else {
            warn!("relay closed connection before sending Connected frame");
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff);
            continue;
        };

        if let TunnelFrame::Connected {
            ref user_id,
            keepalive_interval_secs,
        } = connected
        {
            status_tx
                .send(TunnelStatus::Connected {
                    user_id: user_id.clone(),
                })
                .ok();
            info!(
                user_id = %user_id,
                keepalive_interval_secs,
                "tunnel connected for user {user_id}, keepalive every {keepalive_interval_secs}s"
            );
        }

        // Reset backoff on successful connection.
        backoff = MIN_BACKOFF;

        let action = run_tunnel_loop(
            &client,
            cfg.local_port,
            &mut read,
            &write,
            &mut shutdown_rx,
            &status_tx,
        )
        .await;

        match action {
            LoopExit::Shutdown => return,
            LoopExit::Reconnect(reason) => {
                warn!(reason = %reason, "disconnected from relay, reconnecting");
            }
        }
    }
}

/// Result of the inner frame-processing loop.
enum LoopExit {
    /// Graceful shutdown was requested.
    Shutdown,
    /// Connection was lost; includes the reason string for logging.
    Reconnect(String),
}

/// Process tunnel frames until disconnection or shutdown.
async fn run_tunnel_loop<S>(
    client: &reqwest::Client,
    local_port: u16,
    read: &mut S,
    write: &Arc<Mutex<TunnelSink>>,
    shutdown_rx: &mut watch::Receiver<bool>,
    status_tx: &watch::Sender<TunnelStatus>,
) -> LoopExit
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut local_ws_channels: HashMap<String, mpsc::Sender<String>> = HashMap::new();

    let disconnect_reason: String = loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<TunnelFrame>(&text) {
                            Ok(frame) => {
                                handle_frame(frame, client, local_port, write, &mut local_ws_channels).await;
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to parse tunnel frame");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => break "relay sent close frame".to_string(),
                    Some(Ok(_)) => { debug!("ignoring non-text WebSocket message"); }
                    Some(Err(e)) => break format!("WebSocket error: {e}"),
                    None => break "WebSocket stream ended".to_string(),
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("tunnel shutting down");
                    status_tx.send(TunnelStatus::Disconnected).ok();
                    drop(send_close(write).await);
                    local_ws_channels.clear();
                    return LoopExit::Shutdown;
                }
            }
        }
    };

    local_ws_channels.clear();
    LoopExit::Reconnect(disconnect_reason)
}

/// Build the HTTP request used to initiate the WebSocket connection with auth.
fn build_ws_request(cfg: &CloudConfig) -> Result<ws_http::Request<()>, ws_http::Error> {
    let host = cfg
        .relay_url
        .strip_prefix("wss://")
        .or_else(|| cfg.relay_url.strip_prefix("ws://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or("localhost");

    ws_http::Request::builder()
        .uri(&cfg.relay_url)
        .header("Host", host)
        .header("Authorization", format!("Bearer {}", cfg.token))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
}

/// Wait for the initial `Connected` frame from the relay.
async fn wait_for_connected<S>(read: &mut S) -> Option<TunnelFrame>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => match serde_json::from_str::<TunnelFrame>(&text) {
                Ok(frame @ TunnelFrame::Connected { .. }) => return Some(frame),
                Ok(other) => {
                    debug!(?other, "ignoring non-Connected frame during handshake");
                }
                Err(e) => {
                    warn!(error = %e, "failed to parse frame during handshake");
                }
            },
            Ok(Message::Close(_)) => return None,
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "WebSocket error during handshake");
                return None;
            }
        }
    }
    None
}

/// Process a single tunnel frame.
async fn handle_frame(
    frame: TunnelFrame,
    client: &reqwest::Client,
    local_port: u16,
    write: &Arc<Mutex<TunnelSink>>,
    local_ws_channels: &mut HashMap<String, mpsc::Sender<String>>,
) {
    match frame {
        TunnelFrame::Ping => {
            let pong = TunnelFrame::Pong;
            if let Err(e) = send_frame(write, &pong).await {
                warn!(error = %e, "failed to send Pong");
            }
        }
        TunnelFrame::HttpRequest {
            request_id,
            method,
            path,
            headers,
            body,
        } => {
            let client = client.clone();
            let write = Arc::clone(write);
            tokio::spawn(async move {
                let response = forward_http::forward(
                    &client, local_port, request_id, method, path, headers, body,
                )
                .await;
                if let Err(e) = send_frame(&write, &response).await {
                    warn!(error = %e, "failed to send HttpResponse");
                }
            });
        }
        TunnelFrame::WsOpen {
            channel_id,
            path,
            headers,
        } => {
            let write = Arc::clone(write);
            let ch_id = channel_id.clone();
            let sender =
                forward_ws::handle_ws_open(local_port, channel_id, path, headers, write).await;
            if let Some(tx) = sender {
                local_ws_channels.insert(ch_id, tx);
            }
        }
        TunnelFrame::WsMessage { channel_id, data } => {
            if let Some(tx) = local_ws_channels.get(&channel_id) {
                if let Err(e) = tx.send(data).await {
                    warn!(channel_id, error = %e, "failed to forward WsMessage to local");
                    local_ws_channels.remove(&channel_id);
                }
            } else {
                debug!(channel_id, "WsMessage for unknown channel, ignoring");
            }
        }
        TunnelFrame::WsClose { channel_id } => {
            if local_ws_channels.remove(&channel_id).is_some() {
                debug!(channel_id, "closed local WS channel");
            } else {
                debug!(channel_id, "WsClose for unknown channel, ignoring");
            }
        }
        TunnelFrame::Connected { .. }
        | TunnelFrame::Pong
        | TunnelFrame::HttpResponse { .. }
        | TunnelFrame::WsOpenResult { .. } => {
            warn!("received unexpected frame type from relay");
        }
    }
}

/// Serialize and send a `TunnelFrame` over the WebSocket.
async fn send_frame(
    write: &Arc<Mutex<TunnelSink>>,
    frame: &TunnelFrame,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(frame)?;
    let mut guard = write.lock().await;
    guard.send(Message::Text(json.into())).await?;
    Ok(())
}

/// Send a WebSocket close frame.
async fn send_close(
    write: &Arc<Mutex<TunnelSink>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut guard = write.lock().await;
    guard.send(Message::Close(None)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles() {
        let current = Duration::from_secs(1);
        let next = next_backoff(current);
        assert!(
            next == Duration::from_secs(2),
            "backoff should double from 1s to 2s"
        );
    }

    #[test]
    fn backoff_caps_at_max() {
        let current = Duration::from_secs(45);
        let next = next_backoff(current);
        assert!(
            next == MAX_BACKOFF,
            "backoff should cap at {MAX_BACKOFF:?}, got {next:?}"
        );
    }

    #[test]
    fn backoff_stays_at_max() {
        let next = next_backoff(MAX_BACKOFF);
        assert!(next == MAX_BACKOFF, "backoff at max should stay at max");
    }

    #[test]
    fn backoff_sequence() {
        let mut current = MIN_BACKOFF;
        let expected = [1, 2, 4, 8, 16, 32, 60, 60];
        for &expected_secs in &expected {
            assert!(
                current == Duration::from_secs(expected_secs),
                "expected {expected_secs}s, got {current:?}"
            );
            current = next_backoff(current);
        }
    }
}

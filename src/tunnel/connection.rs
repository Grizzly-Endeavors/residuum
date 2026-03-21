//! Core tunnel connection logic with automatic reconnection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, error, info, warn};

use super::TunnelStatus;
use super::forward_http;
use super::forward_ws;
use super::protocol::TunnelFrame;
use super::{TunnelSink, send_frame};
use crate::config::CloudConfig;

/// Minimum backoff duration between reconnection attempts.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff duration between reconnection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Default keepalive timeout — 3x the relay's default 30s keepalive interval.
/// Used as a fallback when the server does not provide `keepalive_interval_secs`.
const DEFAULT_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(90);

/// Calculate the next backoff duration by doubling the current value, capped at
/// [`MAX_BACKOFF`], with random jitter (0.5x–1.5x) to avoid thundering herd.
#[must_use]
fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    let base = doubled.min(MAX_BACKOFF);
    let jitter = rand::thread_rng().gen_range(0.5_f64..1.5);
    base.mul_f64(jitter)
}

/// Extract the keepalive timeout from a `Connected` frame.
fn keepalive_timeout_from_frame(connected: &TunnelFrame) -> Duration {
    if let TunnelFrame::Connected {
        ref user_id,
        keepalive_interval_secs,
    } = *connected
    {
        let timeout = Duration::from_secs(keepalive_interval_secs * 3);
        info!(
            user_id = %user_id,
            keepalive_interval_secs,
            keepalive_timeout_secs = timeout.as_secs(),
            "tunnel connected for user {user_id}, keepalive every {keepalive_interval_secs}s"
        );
        timeout
    } else {
        warn!("received non-Connected frame where Connected was expected, using default keepalive");
        DEFAULT_KEEPALIVE_TIMEOUT
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
    status_tx: Arc<watch::Sender<TunnelStatus>>,
) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .unwrap_or_else(|e| {
            warn!(error = %e, "failed to build reqwest client with timeout, falling back to default");
            reqwest::Client::default()
        });
    let mut backoff = MIN_BACKOFF;
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        // Check for shutdown before attempting connection.
        if *shutdown_rx.borrow() {
            info!("tunnel shutting down before reconnect");
            status_tx.send(TunnelStatus::Disconnected).ok();
            return;
        }

        status_tx
            .send(TunnelStatus::Connecting)
            .unwrap_or_else(|_| {
                debug!("status receiver dropped");
            });
        info!(url = %cfg.relay_url, "connecting to relay");

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

        // Wait for the Connected frame (15s timeout to avoid blocking
        // indefinitely if the relay accepts the WS but never sends Connected).
        let connected_result =
            tokio::time::timeout(Duration::from_secs(15), wait_for_connected(&mut read)).await;
        let connected = match connected_result {
            Err(_) => {
                warn!(url = %cfg.relay_url, "timed out waiting for Connected frame from relay");
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
            Ok(None) => {
                warn!(url = %cfg.relay_url, "relay closed connection before sending Connected frame");
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
            Ok(Some(frame)) => frame,
        };

        if let TunnelFrame::Connected { ref user_id, .. } = connected {
            status_tx
                .send(TunnelStatus::Connected {
                    user_id: user_id.clone(),
                })
                .unwrap_or_else(|_| {
                    debug!("status receiver dropped");
                });
        }
        let keepalive_timeout = keepalive_timeout_from_frame(&connected);

        // Reset backoff on successful connection.
        backoff = MIN_BACKOFF;

        let action = run_tunnel_loop(
            &client,
            cfg.local_port,
            &mut read,
            &write,
            &mut shutdown_rx,
            &status_tx,
            keepalive_timeout,
        )
        .await;

        match action {
            LoopExit::Shutdown => return,
            LoopExit::Reconnect(reason, open_ws_channels) => {
                warn!(reason = %reason, attempt, open_ws_channels, "disconnected from relay, reconnecting");
            }
        }
    }
}

/// Result of the inner frame-processing loop.
enum LoopExit {
    /// Graceful shutdown was requested.
    Shutdown,
    /// Connection was lost; includes the reason string and open channel count.
    Reconnect(String, usize),
}

/// Process tunnel frames until disconnection or shutdown.
async fn run_tunnel_loop<S>(
    client: &reqwest::Client,
    local_port: u16,
    read: &mut S,
    write: &Arc<Mutex<TunnelSink>>,
    shutdown_rx: &mut watch::Receiver<bool>,
    status_tx: &watch::Sender<TunnelStatus>,
    keepalive_timeout: Duration,
) -> LoopExit
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut local_ws_channels: HashMap<String, mpsc::Sender<String>> = HashMap::new();
    let mut last_frame = tokio::time::Instant::now();

    // Channel for completed WsOpen results — spawned tasks send back the
    // channel_id and sender so the frame loop isn't blocked.
    let (ws_open_tx, mut ws_open_rx) = mpsc::channel::<(String, mpsc::Sender<String>)>(16);

    let disconnect_reason: String = loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_frame = tokio::time::Instant::now();
                        match serde_json::from_str::<TunnelFrame>(&text) {
                            Ok(frame) => {
                                handle_frame(frame, client, local_port, write, &mut local_ws_channels, &ws_open_tx).await;
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to parse tunnel frame");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => break "relay sent close frame".to_string(),
                    Some(Ok(_)) => {
                        last_frame = tokio::time::Instant::now();
                        debug!("ignoring non-text WebSocket message");
                    }
                    Some(Err(e)) => break format!("WebSocket error: {e}"),
                    None => break "WebSocket stream ended".to_string(),
                }
            }
            Some((channel_id, sender)) = ws_open_rx.recv() => {
                local_ws_channels.insert(channel_id.clone(), sender);
                debug!(channel_id, total = local_ws_channels.len(), "registered local WS channel");
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("tunnel shutting down");
                    status_tx.send(TunnelStatus::Disconnected).ok();
                    if let Err(e) = send_close(write).await {
                        warn!(error = %e, "failed to send WebSocket close frame during shutdown");
                    }
                    local_ws_channels.clear();
                    return LoopExit::Shutdown;
                }
            }
            () = tokio::time::sleep_until(last_frame + keepalive_timeout) => {
                break "keepalive timeout".to_string();
            }
        }
    };

    let open_ws_channels = local_ws_channels.len();
    local_ws_channels.clear();
    LoopExit::Reconnect(disconnect_reason, open_ws_channels)
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
    ws_open_tx: &mpsc::Sender<(String, mpsc::Sender<String>)>,
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
            let ws_open_tx = ws_open_tx.clone();
            tokio::spawn(async move {
                let ch_id = channel_id.clone();
                let sender =
                    forward_ws::handle_ws_open(local_port, channel_id, path, headers, write).await;
                if let Some(tx) = sender {
                    // Send back to the frame loop; if the loop has exited the
                    // channel will be dropped and this is harmless.
                    if let Err(e) = ws_open_tx.send((ch_id.clone(), tx)).await {
                        warn!(channel_id = ch_id, error = %e, "failed to register WS channel with frame loop");
                    }
                }
            });
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
        TunnelFrame::Connected { .. } => {
            warn!(
                frame_type = "Connected",
                "received unexpected frame type from relay"
            );
        }
        TunnelFrame::Pong => {
            warn!(
                frame_type = "Pong",
                "received unexpected frame type from relay"
            );
        }
        TunnelFrame::HttpResponse { .. } => {
            warn!(
                frame_type = "HttpResponse",
                "received unexpected frame type from relay"
            );
        }
        TunnelFrame::WsOpenResult { .. } => {
            warn!(
                frame_type = "WsOpenResult",
                "received unexpected frame type from relay"
            );
        }
    }
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
    fn backoff_doubles_with_jitter() {
        let current = Duration::from_secs(2);
        let next = next_backoff(current);
        // Doubled = 4s, jitter range 0.5..1.5 → result in 2s..6s
        assert!(
            next >= Duration::from_secs(2) && next <= Duration::from_secs(6),
            "backoff from 2s should be in 2s..6s (4s ± jitter), got {next:?}"
        );
    }

    #[test]
    fn backoff_caps_at_max_with_jitter() {
        let current = Duration::from_secs(45);
        let next = next_backoff(current);
        // Base is capped at MAX_BACKOFF (60s), jitter range → 30s..90s
        let max_with_jitter = MAX_BACKOFF.mul_f64(1.5);
        assert!(
            next <= max_with_jitter,
            "backoff should not exceed {max_with_jitter:?}, got {next:?}"
        );
    }

    #[test]
    fn backoff_at_max_stays_bounded() {
        let next = next_backoff(MAX_BACKOFF);
        let max_with_jitter = MAX_BACKOFF.mul_f64(1.5);
        assert!(
            next <= max_with_jitter,
            "backoff at max should stay bounded by {max_with_jitter:?}, got {next:?}"
        );
    }

    #[test]
    fn default_keepalive_timeout_is_3x_relay_interval() {
        // Relay sends keepalive pings every 30s; default timeout must be 3x that.
        assert!(
            DEFAULT_KEEPALIVE_TIMEOUT == Duration::from_secs(90),
            "default keepalive timeout should be 90s (3 × 30s relay interval), got {DEFAULT_KEEPALIVE_TIMEOUT:?}"
        );
    }

    #[test]
    fn backoff_increases_monotonically_on_average() {
        // With jitter, individual values may vary, but the base should increase.
        // Run multiple samples to verify the trend.
        let mut current = MIN_BACKOFF;
        for _ in 0..5 {
            let next = next_backoff(current);
            // The base doubles, so even with 0.5x jitter the minimum should
            // be >= current * 0.5 (since doubled * 0.5 = current).
            let floor = current.mul_f64(0.5);
            assert!(
                next >= floor,
                "backoff should not drop below {floor:?}, got {next:?} from {current:?}"
            );
            current = next;
        }
    }
}

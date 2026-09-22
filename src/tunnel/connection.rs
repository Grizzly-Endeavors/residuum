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
use super::protocol::{Surface, TunnelFrame};
use super::{ForwardTargets, TunnelSink, send_frame};
use crate::config::CloudConfig;

/// Minimum backoff duration between reconnection attempts.
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum backoff duration between reconnection attempts.
const MAX_BACKOFF: Duration = Duration::from_mins(1);

/// Calculate the next backoff duration by doubling the current value, capped at
/// [`MAX_BACKOFF`], with random jitter (0.5x–1.5x) to avoid thundering herd.
#[must_use]
fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    let base = doubled.min(MAX_BACKOFF);
    let jitter = rand::thread_rng().gen_range(0.5_f64..1.5);
    base.mul_f64(jitter)
}

/// Start the tunnel client, maintaining a persistent connection to the cloud
/// relay with exponential backoff reconnection.
///
/// The tunnel forwards HTTP requests and WebSocket connections from the relay
/// to the local residuum instance: the main listener on `cfg.local_port`, and
/// workbench tool requests to `workbench_port` when that listener is running.
///
/// # Errors
///
/// This function runs until the shutdown signal is received. Transient
/// connection errors are logged and retried automatically.
#[tracing::instrument(skip_all, fields(relay_url = %cfg.relay_url))]
pub(crate) async fn start_tunnel(
    cfg: CloudConfig,
    workbench_port: Option<u16>,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: Arc<watch::Sender<TunnelStatus>>,
) {
    let targets = ForwardTargets {
        main: cfg.local_port,
        workbench: workbench_port,
    };
    let Ok(client) = forward_http::forwarding_client() else {
        error!("failed to build reqwest client");
        return;
    };
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
        debug!(url = %cfg.relay_url, "connecting to relay");

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
                warn!(error = %e, backoff_ms = backoff.as_millis(), "failed to connect to relay, will retry");
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
        let (user_id, keepalive_interval_secs, origins) = match connected_result {
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
            Ok(Some(connected)) => connected,
        };

        status_tx
            .send(TunnelStatus::Connected {
                user_id: user_id.clone(),
                origin: origins.origin,
                workbench_origin: origins.workbench_origin,
            })
            .unwrap_or_else(|_| {
                debug!("status receiver dropped");
            });

        let keepalive_timeout = Duration::from_secs(keepalive_interval_secs * 3);
        info!(
            user_id = %user_id,
            keepalive_interval_secs,
            keepalive_timeout_secs = keepalive_timeout.as_secs(),
            "tunnel connected"
        );

        // Reset backoff on successful connection.
        backoff = MIN_BACKOFF;

        let action = run_tunnel_loop(
            &client,
            targets,
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
    targets: ForwardTargets,
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
                                handle_frame(frame, client, targets, write, &mut local_ws_channels, &ws_open_tx).await;
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
                    {
                        let mut guard = write.lock().await;
                        if let Err(e) = guard.send(Message::Close(None)).await {
                            warn!(error = %e, "failed to send WebSocket close frame during shutdown");
                        }
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
    LoopExit::Reconnect(disconnect_reason, open_ws_channels)
}

/// Build the HTTP request used to initiate the WebSocket connection with auth.
fn build_ws_request(cfg: &CloudConfig) -> Result<ws_http::Request<()>, ws_http::Error> {
    let host = url::Url::parse(&cfg.relay_url)
        .ok()
        .and_then(|u| {
            let h = u.host_str()?.to_string();
            Some(match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h,
            })
        })
        .unwrap_or_else(|| "localhost".to_string());
    let host = host.as_str();

    let mut request = super::build_ws_upgrade_request(&cfg.relay_url, host)?;
    request.headers_mut().insert(
        ws_http::header::AUTHORIZATION,
        ws_http::HeaderValue::from_str(&format!("Bearer {}", cfg.token))?,
    );
    request.headers_mut().insert(
        super::CAPABILITIES_HEADER,
        ws_http::HeaderValue::from_static(super::WORKBENCH_SURFACE_CAPABILITY),
    );
    Ok(request)
}

/// Public origins the relay announced for this user.
#[derive(Debug, Default)]
struct AnnouncedOrigins {
    origin: Option<String>,
    workbench_origin: Option<String>,
}

/// Wait for the initial `Connected` frame from the relay.
async fn wait_for_connected<S>(read: &mut S) -> Option<(String, u64, AnnouncedOrigins)>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => match serde_json::from_str::<TunnelFrame>(&text) {
                Ok(TunnelFrame::Connected {
                    user_id,
                    keepalive_interval_secs,
                    origin,
                    workbench_origin,
                }) => {
                    return Some((
                        user_id,
                        keepalive_interval_secs,
                        AnnouncedOrigins {
                            origin,
                            workbench_origin,
                        },
                    ));
                }
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

/// The local port for a request's surface, or why it can't be served.
///
/// A workbench request is never sent to the main listener: that would serve
/// the web UI and API on the tools' origin.
fn forward_port(targets: ForwardTargets, surface: Option<Surface>) -> Result<u16, &'static str> {
    match surface {
        None => Ok(targets.main),
        Some(Surface::Workbench) => targets.workbench.ok_or(
            "Workbench tools aren't available on this Residuum instance right now: its tools listener isn't running. Check Residuum's logs for why it couldn't start.",
        ),
    }
}

/// Process a single tunnel frame.
async fn handle_frame(
    frame: TunnelFrame,
    client: &reqwest::Client,
    targets: ForwardTargets,
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
            surface,
        } => {
            let client = client.clone();
            let write = Arc::clone(write);
            tokio::spawn(async move {
                let response = match forward_port(targets, surface) {
                    Ok(port) => {
                        forward_http::forward(
                            &client, port, request_id, method, path, headers, body,
                        )
                        .await
                    }
                    Err(message) => forward_http::text_response(request_id, 503, message),
                };
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
                    forward_ws::handle_ws_open(targets.main, channel_id, path, headers, write)
                        .await;
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
        let min_with_jitter = MAX_BACKOFF.mul_f64(0.5);
        let max_with_jitter = MAX_BACKOFF.mul_f64(1.5);
        assert!(
            next >= min_with_jitter,
            "backoff should be at least {min_with_jitter:?}, got {next:?}"
        );
        assert!(
            next <= max_with_jitter,
            "backoff should not exceed {max_with_jitter:?}, got {next:?}"
        );
    }

    #[test]
    fn backoff_at_max_stays_bounded() {
        let next = next_backoff(MAX_BACKOFF);
        let min_with_jitter = MAX_BACKOFF.mul_f64(0.5);
        let max_with_jitter = MAX_BACKOFF.mul_f64(1.5);
        assert!(
            next >= min_with_jitter,
            "backoff at max should be at least {min_with_jitter:?}, got {next:?}"
        );
        assert!(
            next <= max_with_jitter,
            "backoff at max should stay bounded by {max_with_jitter:?}, got {next:?}"
        );
    }

    #[test]
    fn backoff_floor_is_never_below_half_current() {
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

    #[test]
    fn build_ws_request_wss_url() {
        let cfg = crate::config::CloudConfig {
            relay_url: "wss://relay.example.com/ws".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_ws_url() {
        let cfg = crate::config::CloudConfig {
            relay_url: "ws://relay.example.com".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_url_with_path() {
        let cfg = crate::config::CloudConfig {
            relay_url: "ws://relay.example.com/some/path".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_malformed_url_falls_back_to_localhost() {
        let cfg = crate::config::CloudConfig {
            relay_url: "not-a-url".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg).unwrap();
        assert_eq!(req.headers()["host"], "localhost");
    }

    #[tokio::test]
    async fn wait_for_connected_returns_some_on_connected_frame() {
        use futures_util::stream;
        let frame = TunnelFrame::Connected {
            user_id: "user-1".to_string(),
            keepalive_interval_secs: 30,
            origin: Some("https://user-1.agent-residuum.com".to_string()),
            workbench_origin: Some("https://user-1.workbench.agent-residuum.com".to_string()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let messages: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> =
            vec![Ok(Message::Text(json.into()))];
        let mut stream = stream::iter(messages);
        let result = wait_for_connected(&mut stream).await;
        let (user_id, keepalive, origins) = result.unwrap();
        assert_eq!((user_id.as_str(), keepalive), ("user-1", 30));
        assert_eq!(
            origins.workbench_origin.as_deref(),
            Some("https://user-1.workbench.agent-residuum.com")
        );
    }

    #[test]
    fn workbench_requests_only_reach_a_running_tools_listener() {
        let running = ForwardTargets {
            main: 7700,
            workbench: Some(7702),
        };
        assert_eq!(forward_port(running, None), Ok(7700));
        assert_eq!(forward_port(running, Some(Surface::Workbench)), Ok(7702));

        let down = ForwardTargets {
            main: 7700,
            workbench: None,
        };
        assert!(
            forward_port(down, Some(Surface::Workbench)).is_err(),
            "a workbench request must never fall back to the main listener"
        );
    }

    #[test]
    fn upgrade_request_advertises_the_workbench_capability() {
        let cfg = CloudConfig {
            relay_url: "wss://agent-residuum.com/tunnel/register".to_string(),
            token: "rst_test".to_string(),
            local_port: 7700,
        };
        let req = build_ws_request(&cfg).unwrap();
        assert_eq!(
            req.headers()
                .get("x-residuum-capabilities")
                .and_then(|v| v.to_str().ok()),
            Some("workbench-surface")
        );
    }

    #[tokio::test]
    async fn wait_for_connected_returns_none_on_close_frame() {
        use futures_util::stream;
        let messages: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> =
            vec![Ok(Message::Close(None))];
        let mut stream = stream::iter(messages);
        let result = wait_for_connected(&mut stream).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_connected_skips_non_connected_frames() {
        use futures_util::stream;
        let ping_json = serde_json::to_string(&TunnelFrame::Ping).unwrap();
        let connected_json = serde_json::to_string(&TunnelFrame::Connected {
            user_id: "user-2".to_string(),
            keepalive_interval_secs: 15,
            origin: None,
            workbench_origin: None,
        })
        .unwrap();
        let messages: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> = vec![
            Ok(Message::Text(ping_json.into())),
            Ok(Message::Text(connected_json.into())),
        ];
        let mut stream = stream::iter(messages);
        let result = wait_for_connected(&mut stream).await;
        let (user_id, keepalive, origins) = result.unwrap();
        assert_eq!((user_id.as_str(), keepalive), ("user-2", 15));
        assert!(origins.origin.is_none() && origins.workbench_origin.is_none());
    }

    #[tokio::test]
    async fn wait_for_connected_returns_none_on_ws_error() {
        use futures_util::stream;
        let messages: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> =
            vec![Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed)];
        let mut stream = stream::iter(messages);
        let result = wait_for_connected(&mut stream).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn wait_for_connected_returns_none_when_stream_ends() {
        use futures_util::stream;
        let messages: Vec<Result<Message, tokio_tungstenite::tungstenite::Error>> = vec![];
        let mut stream = stream::iter(messages);
        let result = wait_for_connected(&mut stream).await;
        assert!(result.is_none());
    }
}

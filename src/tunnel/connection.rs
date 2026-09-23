//! Core tunnel connection logic with automatic reconnection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, error, info, warn};

use super::TunnelStatus;
use super::forward_a2a;
use super::forward_http;
use super::forward_ws;
use super::protocol::{Surface, TunnelFrame};
use super::{ForwardRequest, ForwardTargets, TunnelA2a, TunnelSink, send_frame};
use crate::config::CloudConfig;

/// The HTTP clients used to forward requests to local listeners: the
/// buffered client for the main and workbench surfaces, and the streaming
/// client (no total body timeout) for the A2A surface.
struct TunnelClients<'a> {
    client: &'a reqwest::Client,
    a2a_client: &'a reqwest::Client,
}

/// The bookkeeping needed to track in-flight A2A streaming forwards so a
/// later `HttpCancel` can abort the matching one.
struct A2aStreamTracker<'a> {
    streams: &'a mut HashMap<String, tokio::task::AbortHandle>,
    /// Streams report their own `request_id` here on completion, so
    /// `streams` doesn't grow without bound over the tunnel's lifetime.
    done_tx: &'a mpsc::Sender<String>,
}

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
/// to the local residuum instance: the main listener on `cfg.local_port`,
/// workbench artifact requests to `workbench_port` when that listener is
/// running, and A2A requests to `a2a_port` when that listener is running.
/// `a2a` controls whether the `a2a`/`a2a-private` capabilities are advertised
/// on the upgrade at all, independent of whether `a2a_port` is currently
/// serving.
///
/// # Errors
///
/// This function runs until the shutdown signal is received. Transient
/// connection errors are logged and retried automatically.
#[tracing::instrument(skip_all, fields(relay_url = %cfg.relay_url))]
pub(crate) async fn start_tunnel(
    cfg: CloudConfig,
    workbench_port: Option<u16>,
    a2a_port: Option<u16>,
    a2a: Option<TunnelA2a>,
    mut shutdown_rx: watch::Receiver<bool>,
    status_tx: Arc<watch::Sender<TunnelStatus>>,
) {
    let targets = ForwardTargets {
        main: cfg.local_port,
        workbench: workbench_port,
        a2a: a2a_port,
    };
    let Ok(client) = forward_http::forwarding_client() else {
        error!("failed to build reqwest client");
        return;
    };
    let Ok(a2a_client) = forward_a2a::forwarding_client() else {
        error!("failed to build A2A reqwest client");
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

        let (write, mut read, user_id, keepalive_interval_secs, origins) =
            match connect_and_handshake(&cfg, a2a).await {
                ConnectAttempt::Ready {
                    write,
                    read,
                    user_id,
                    keepalive_interval_secs,
                    origins,
                } => (write, read, user_id, keepalive_interval_secs, origins),
                ConnectAttempt::Retry => {
                    tokio::time::sleep(backoff).await;
                    backoff = next_backoff(backoff);
                    continue;
                }
            };

        status_tx
            .send(TunnelStatus::Connected {
                user_id: user_id.clone(),
                origin: origins.origin,
                workbench_origin: origins.workbench_origin,
                instance: origins.instance,
                a2a_token: origins.a2a_token,
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

        let clients = TunnelClients {
            client: &client,
            a2a_client: &a2a_client,
        };
        let action = run_tunnel_loop(
            clients,
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

/// Outcome of one connect-and-handshake attempt.
enum ConnectAttempt {
    /// Connected and handshaked; ready to run the frame loop.
    Ready {
        write: Arc<Mutex<TunnelSink>>,
        read: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
        >,
        user_id: String,
        keepalive_interval_secs: u64,
        origins: AnnouncedOrigins,
    },
    /// Failed; the reason is already logged. The caller should back off and
    /// retry.
    Retry,
}

/// Open the WebSocket connection to the relay and wait for its `Connected`
/// handshake frame.
async fn connect_and_handshake(cfg: &CloudConfig, a2a: Option<TunnelA2a>) -> ConnectAttempt {
    let request = match build_ws_request(cfg, a2a) {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "failed to build WebSocket request");
            return ConnectAttempt::Retry;
        }
    };

    let (ws_stream, _response) = match tokio_tungstenite::connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(error = %e, "failed to connect to relay, will retry");
            return ConnectAttempt::Retry;
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
            return ConnectAttempt::Retry;
        }
        Ok(None) => {
            warn!(url = %cfg.relay_url, "relay closed connection before sending Connected frame");
            return ConnectAttempt::Retry;
        }
        Ok(Some(connected)) => connected,
    };

    ConnectAttempt::Ready {
        write,
        read,
        user_id,
        keepalive_interval_secs,
        origins,
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
    clients: TunnelClients<'_>,
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

    // In-flight A2A streaming forwards, keyed by request_id, so a `HttpCancel`
    // can abort the matching local request. Streams report their own
    // completion on `a2a_done_tx` so the map doesn't grow without bound over
    // the tunnel's lifetime.
    let mut a2a_streams: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    let (a2a_done_tx, mut a2a_done_rx) = mpsc::channel::<String>(64);

    let disconnect_reason: String = loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_frame = tokio::time::Instant::now();
                        match serde_json::from_str::<TunnelFrame>(&text) {
                            Ok(frame) => {
                                let mut a2a_tracker = A2aStreamTracker {
                                    streams: &mut a2a_streams,
                                    done_tx: &a2a_done_tx,
                                };
                                handle_frame(
                                    frame,
                                    &clients,
                                    targets,
                                    write,
                                    &mut local_ws_channels,
                                    &ws_open_tx,
                                    &mut a2a_tracker,
                                )
                                .await;
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
            Some(request_id) = a2a_done_rx.recv() => {
                a2a_streams.remove(&request_id);
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
                    for (_, handle) in a2a_streams.drain() {
                        handle.abort();
                    }
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
fn build_ws_request(
    cfg: &CloudConfig,
    a2a: Option<TunnelA2a>,
) -> Result<ws_http::Request<()>, ws_http::Error> {
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
        ws_http::HeaderValue::from_str(&super::build_capabilities_header(a2a))?,
    );
    Ok(request)
}

/// Public origins and A2A identity the relay announced for this connection.
#[derive(Debug, Default)]
struct AnnouncedOrigins {
    origin: Option<String>,
    workbench_origin: Option<String>,
    instance: Option<String>,
    a2a_token: Option<String>,
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
                    instance,
                    a2a_token,
                }) => {
                    return Some((
                        user_id,
                        keepalive_interval_secs,
                        AnnouncedOrigins {
                            origin,
                            workbench_origin,
                            instance,
                            a2a_token,
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
/// A workbench or A2A request is never sent to the main listener: that would
/// serve the web UI and API on that surface's origin.
fn forward_port(targets: ForwardTargets, surface: Option<Surface>) -> Result<u16, &'static str> {
    match surface {
        None => Ok(targets.main),
        Some(Surface::Workbench) => targets.workbench.ok_or(
            "Workbench artifacts aren't available on this Residuum instance right now: its artifacts listener isn't running. Check Residuum's logs for why it couldn't start.",
        ),
        Some(Surface::A2a) => targets.a2a.ok_or(
            "The A2A endpoint isn't available on this Residuum instance right now: its A2A listener isn't running. Check Residuum's logs for why it couldn't start.",
        ),
    }
}

/// Spawn the streaming forward for an A2A-surface request and register it in
/// `tracker.streams` so a later `HttpCancel` can abort it.
fn spawn_a2a_forward(
    a2a_client: &reqwest::Client,
    targets: ForwardTargets,
    write: &Arc<Mutex<TunnelSink>>,
    tracker: &mut A2aStreamTracker<'_>,
    request: ForwardRequest,
) {
    let client = a2a_client.clone();
    let write = Arc::clone(write);
    let done_tx = tracker.done_tx.clone();
    let request_id = request.request_id.clone();
    let request_id_for_task = request_id.clone();
    let handle = tokio::spawn(async move {
        match forward_port(targets, Some(Surface::A2a)) {
            Ok(port) => {
                forward_a2a::stream_forward(&client, port, request, &write).await;
            }
            Err(message) => {
                forward_a2a::stream_error(&write, &request_id_for_task, 503, message).await;
            }
        }
        if let Err(e) = done_tx.send(request_id_for_task).await {
            debug!(
                request_id = %e.0,
                "A2A stream completion channel closed, tunnel is reconnecting or shutting down"
            );
        }
    });
    tracker.streams.insert(request_id, handle.abort_handle());
}

/// Forward a non-A2A `HttpRequest` with the existing buffered behavior: one
/// local request, one `HttpResponse` frame back.
fn spawn_buffered_forward(
    client: &reqwest::Client,
    targets: ForwardTargets,
    write: &Arc<Mutex<TunnelSink>>,
    surface: Option<Surface>,
    request: ForwardRequest,
) {
    let client = client.clone();
    let write = Arc::clone(write);
    let ForwardRequest {
        request_id,
        method,
        path,
        headers,
        body,
    } = request;
    tokio::spawn(async move {
        let response = match forward_port(targets, surface) {
            Ok(port) => {
                forward_http::forward(&client, port, request_id, method, path, headers, body).await
            }
            Err(message) => forward_http::text_response(request_id, 503, message),
        };
        if let Err(e) = send_frame(&write, &response).await {
            warn!(error = %e, "failed to send HttpResponse");
        }
    });
}

/// Connect to the local main listener for a `WsOpen` frame and register the
/// resulting channel with the frame loop once it's ready.
fn spawn_ws_open(
    targets: ForwardTargets,
    write: &Arc<Mutex<TunnelSink>>,
    ws_open_tx: &mpsc::Sender<(String, mpsc::Sender<String>)>,
    channel_id: String,
    path: String,
    headers: HashMap<String, String>,
) {
    let write = Arc::clone(write);
    let ws_open_tx = ws_open_tx.clone();
    tokio::spawn(async move {
        let ch_id = channel_id.clone();
        let sender =
            forward_ws::handle_ws_open(targets.main, channel_id, path, headers, write).await;
        if let Some(tx) = sender {
            // Send back to the frame loop; if the loop has exited the channel
            // will be dropped and this is harmless.
            if let Err(e) = ws_open_tx.send((ch_id.clone(), tx)).await {
                warn!(channel_id = ch_id, error = %e, "failed to register WS channel with frame loop");
            }
        }
    });
}

/// Process a single tunnel frame.
async fn handle_frame(
    frame: TunnelFrame,
    clients: &TunnelClients<'_>,
    targets: ForwardTargets,
    write: &Arc<Mutex<TunnelSink>>,
    local_ws_channels: &mut HashMap<String, mpsc::Sender<String>>,
    ws_open_tx: &mpsc::Sender<(String, mpsc::Sender<String>)>,
    a2a_tracker: &mut A2aStreamTracker<'_>,
) {
    // Computed unconditionally (cheap) so the catch-all arm below can log
    // which kind of frame it was without holding onto `frame` past the match.
    let frame_type = frame.type_name();

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
            let request = ForwardRequest {
                request_id,
                method,
                path,
                headers,
                body,
            };
            if matches!(surface, Some(Surface::A2a)) {
                spawn_a2a_forward(clients.a2a_client, targets, write, a2a_tracker, request);
            } else {
                spawn_buffered_forward(clients.client, targets, write, surface, request);
            }
        }
        TunnelFrame::HttpCancel { request_id } => {
            if let Some(handle) = a2a_tracker.streams.remove(&request_id) {
                handle.abort();
                debug!(request_id, "aborted A2A stream on HttpCancel");
            } else {
                debug!(
                    request_id,
                    "HttpCancel for unknown or already-finished stream, ignoring"
                );
            }
        }
        TunnelFrame::WsOpen {
            channel_id,
            path,
            headers,
        } => {
            spawn_ws_open(targets, write, ws_open_tx, channel_id, path, headers);
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
        | TunnelFrame::HttpResponseStart { .. }
        | TunnelFrame::HttpResponseChunk { .. }
        | TunnelFrame::HttpResponseEnd { .. }
        | TunnelFrame::WsOpenResult { .. } => {
            warn!(frame_type, "received unexpected frame type from relay");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::A2aVisibility;
    use super::*;

    /// Bound on every test's individual waits, so a real regression (a
    /// forward that never answers, a cancel that doesn't actually stop the
    /// upstream request, ...) fails the test instead of hanging the suite.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Runs `fut`, bounded by [`TEST_TIMEOUT`], panicking with a clear
    /// message instead of hanging the test suite if it doesn't resolve.
    async fn with_timeout<F: std::future::Future>(fut: F) -> F::Output {
        tokio::time::timeout(TEST_TIMEOUT, fut)
            .await
            .expect("operation timed out; this would otherwise hang the test suite")
    }

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
        let req = build_ws_request(&cfg, None).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_ws_url() {
        let cfg = crate::config::CloudConfig {
            relay_url: "ws://relay.example.com".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg, None).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_url_with_path() {
        let cfg = crate::config::CloudConfig {
            relay_url: "ws://relay.example.com/some/path".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg, None).unwrap();
        assert_eq!(req.headers()["host"], "relay.example.com");
    }

    #[test]
    fn build_ws_request_malformed_url_falls_back_to_localhost() {
        let cfg = crate::config::CloudConfig {
            relay_url: "not-a-url".to_string(),
            token: "tok".to_string(),
            local_port: 8080,
        };
        let req = build_ws_request(&cfg, None).unwrap();
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
            instance: Some("laptop".to_string()),
            a2a_token: Some("rsa_abc123".to_string()),
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
        assert_eq!(origins.instance.as_deref(), Some("laptop"));
        assert_eq!(origins.a2a_token.as_deref(), Some("rsa_abc123"));
    }

    #[test]
    fn workbench_requests_only_reach_a_running_artifacts_listener() {
        let running = ForwardTargets {
            main: 7700,
            workbench: Some(7702),
            a2a: None,
        };
        assert_eq!(forward_port(running, None), Ok(7700));
        assert_eq!(forward_port(running, Some(Surface::Workbench)), Ok(7702));

        let down = ForwardTargets {
            main: 7700,
            workbench: None,
            a2a: None,
        };
        assert!(
            forward_port(down, Some(Surface::Workbench)).is_err(),
            "a workbench request must never fall back to the main listener"
        );
    }

    #[test]
    fn a2a_requests_only_reach_a_running_a2a_listener() {
        let running = ForwardTargets {
            main: 7700,
            workbench: None,
            a2a: Some(7703),
        };
        assert_eq!(forward_port(running, Some(Surface::A2a)), Ok(7703));

        let down = ForwardTargets {
            main: 7700,
            workbench: None,
            a2a: None,
        };
        assert!(
            forward_port(down, Some(Surface::A2a)).is_err(),
            "an A2A request must never fall back to the main listener"
        );
    }

    #[test]
    fn upgrade_request_advertises_capabilities_without_a2a_by_default() {
        let cfg = CloudConfig {
            relay_url: "wss://agent-residuum.com/tunnel/register".to_string(),
            token: "rst_test".to_string(),
            local_port: 7700,
        };
        let req = build_ws_request(&cfg, None).unwrap();
        assert_eq!(
            req.headers()
                .get("x-residuum-capabilities")
                .and_then(|v| v.to_str().ok()),
            Some("workbench-surface,http-streaming")
        );
    }

    #[test]
    fn upgrade_request_advertises_a2a_when_configured() {
        let cfg = CloudConfig {
            relay_url: "wss://agent-residuum.com/tunnel/register".to_string(),
            token: "rst_test".to_string(),
            local_port: 7700,
        };
        let req = build_ws_request(
            &cfg,
            Some(TunnelA2a {
                visibility: A2aVisibility::Private,
            }),
        )
        .unwrap();
        assert_eq!(
            req.headers()
                .get("x-residuum-capabilities")
                .and_then(|v| v.to_str().ok()),
            Some("workbench-surface,http-streaming,a2a,a2a-private")
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
            instance: None,
            a2a_token: None,
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

    /// A tunnel sink backed by an in-memory loopback WebSocket, paired with the
    /// server-side end so tests can read back what a `handle_frame` call sent
    /// without a real relay.
    async fn loopback_ws() -> (
        Arc<Mutex<TunnelSink>>,
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        });
        let (client_stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();
        let server_stream = accept.await.unwrap();
        let (write, _read) = client_stream.split();
        (Arc::new(Mutex::new(write)), server_stream)
    }

    /// Reads the next tunnel frame, bounded by [`TEST_TIMEOUT`] so a stream
    /// that never sends one fails the test instead of hanging it.
    async fn recv_ws_frame(
        server: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) -> TunnelFrame {
        with_timeout(async {
            loop {
                match server.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("expected a text frame, got {other:?}"),
                }
            }
        })
        .await
    }

    #[tokio::test]
    async fn handle_frame_streams_503_when_a2a_port_is_missing() {
        let (write, mut relay) = loopback_ws().await;
        let client = forward_http::forwarding_client().unwrap();
        let a2a_client = forward_a2a::forwarding_client().unwrap();
        let targets = ForwardTargets {
            main: 7700,
            workbench: None,
            a2a: None,
        };
        let clients = TunnelClients {
            client: &client,
            a2a_client: &a2a_client,
        };
        let mut local_ws_channels = HashMap::new();
        let (ws_open_tx, _ws_open_rx) = mpsc::channel(1);
        let mut a2a_streams = HashMap::new();
        let (a2a_done_tx, mut a2a_done_rx) = mpsc::channel(1);
        let mut a2a_tracker = A2aStreamTracker {
            streams: &mut a2a_streams,
            done_tx: &a2a_done_tx,
        };

        with_timeout(handle_frame(
            TunnelFrame::HttpRequest {
                request_id: "req-503".to_string(),
                method: "GET".to_string(),
                path: "/a2a/laptop/.well-known/agent-card.json".to_string(),
                headers: HashMap::new(),
                body: None,
                surface: Some(Surface::A2a),
            },
            &clients,
            targets,
            &write,
            &mut local_ws_channels,
            &ws_open_tx,
            &mut a2a_tracker,
        ))
        .await;

        // Wait for the spawned forward to report itself done before reading
        // the frames it sent.
        let done_id = with_timeout(a2a_done_rx.recv())
            .await
            .expect("the spawned A2A forward should report completion");
        assert_eq!(done_id, "req-503");

        let start = recv_ws_frame(&mut relay).await;
        assert!(
            matches!(start, TunnelFrame::HttpResponseStart { status: 503, .. }),
            "a missing A2A listener should stream a 503, got {start:?}"
        );
        let chunk = recv_ws_frame(&mut relay).await;
        assert!(matches!(chunk, TunnelFrame::HttpResponseChunk { .. }));
        let end = recv_ws_frame(&mut relay).await;
        assert!(matches!(
            end,
            TunnelFrame::HttpResponseEnd { error: None, .. }
        ));
    }

    #[tokio::test]
    async fn http_cancel_aborts_the_in_flight_a2a_stream() {
        // An SSE server whose first event is delayed well past this test's
        // assertions, so an unaborted stream could not possibly finish (or
        // report itself done) within the timeout below.
        let app = axum::Router::new().route(
            "/slow",
            axum::routing::get(|| async {
                let events = futures_util::stream::once(async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default().data("late"),
                    )
                });
                axum::response::sse::Sse::new(events)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (write, _relay) = loopback_ws().await;
        let client = forward_http::forwarding_client().unwrap();
        let a2a_client = forward_a2a::forwarding_client().unwrap();
        let targets = ForwardTargets {
            main: 7700,
            workbench: None,
            a2a: Some(addr.port()),
        };
        let clients = TunnelClients {
            client: &client,
            a2a_client: &a2a_client,
        };
        let mut local_ws_channels = HashMap::new();
        let (ws_open_tx, _ws_open_rx) = mpsc::channel(1);
        let mut a2a_streams = HashMap::new();
        let (a2a_done_tx, mut a2a_done_rx) = mpsc::channel(1);
        let mut a2a_tracker = A2aStreamTracker {
            streams: &mut a2a_streams,
            done_tx: &a2a_done_tx,
        };

        with_timeout(handle_frame(
            TunnelFrame::HttpRequest {
                request_id: "req-cancel".to_string(),
                method: "GET".to_string(),
                path: "/slow".to_string(),
                headers: HashMap::new(),
                body: None,
                surface: Some(Surface::A2a),
            },
            &clients,
            targets,
            &write,
            &mut local_ws_channels,
            &ws_open_tx,
            &mut a2a_tracker,
        ))
        .await;
        assert!(
            a2a_tracker.streams.contains_key("req-cancel"),
            "the forward should be registered for cancellation"
        );

        with_timeout(handle_frame(
            TunnelFrame::HttpCancel {
                request_id: "req-cancel".to_string(),
            },
            &clients,
            targets,
            &write,
            &mut local_ws_channels,
            &ws_open_tx,
            &mut a2a_tracker,
        ))
        .await;
        assert!(
            !a2a_tracker.streams.contains_key("req-cancel"),
            "HttpCancel should remove the stream from tracking immediately"
        );

        // A forward that ran to completion (or was merely dropped without
        // being aborted) would still report itself done. An aborted task is
        // killed at its next await point and never reaches that line, so no
        // completion should arrive within a window far shorter than the
        // server's 5s delay.
        let result = tokio::time::timeout(Duration::from_millis(500), a2a_done_rx.recv()).await;
        server.abort();
        assert!(
            result.is_err(),
            "an aborted A2A forward must not report completion, got {result:?}"
        );
    }
}

//! WebSocket connection forwarding to the local residuum instance.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tracing::{debug, warn};

use super::protocol::TunnelFrame;
use super::{TUNNEL_NONCE_HEADER, TunnelSink, send_frame, tunnel_nonce};

/// Channel capacity for messages flowing from the tunnel to local WebSocket.
const LOCAL_WS_CHANNEL_CAPACITY: usize = 64;

/// Handle a `WsOpen` frame by connecting to the local WebSocket endpoint on
/// `port`: the main listener, or the artifacts listener for a workbench-surface
/// open.
///
/// On success, sends a `WsOpenResult { success: true }` through the tunnel and
/// returns an `mpsc::Sender` for forwarding messages from the tunnel to the
/// local WebSocket. On failure, sends `WsOpenResult { success: false }` with a
/// reason and returns `None`.
///
/// The loopback request carries this process's tunnel nonce and never a
/// client-supplied one, exactly as `forward_http::forward` does, so a socket
/// the relay opens is marked as tunnel-forwarded and a remote client can't
/// forge or erase the mark.
///
/// # Errors
///
/// This function does not return errors directly. Connection failures are
/// communicated back through the tunnel as `WsOpenResult` frames.
#[tracing::instrument(skip_all, fields(channel_id = %channel_id, path = %path))]
pub(super) async fn handle_ws_open(
    port: u16,
    channel_id: String,
    path: String,
    headers: HashMap<String, String>,
    tunnel_tx: Arc<Mutex<TunnelSink>>,
) -> Option<mpsc::Sender<String>> {
    let url = format!("ws://localhost:{port}{path}");
    debug!(channel_id, url, "opening local WebSocket connection");

    let request = match build_local_request(&url, port, &headers) {
        Ok(r) => r,
        Err(e) => {
            warn!(channel_id, error = %e, "failed to build local WS request");
            refuse_ws_open(
                &tunnel_tx,
                &channel_id,
                "Residuum couldn't make sense of the socket request.",
            )
            .await;
            return None;
        }
    };

    let (ws_stream, _response) = match tokio_tungstenite::connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(channel_id, url, error = %e, "failed to connect to local WebSocket");
            refuse_ws_open(&tunnel_tx, &channel_id, &connect_failure_reason(&e)).await;
            return None;
        }
    };

    // Connection succeeded — notify the relay.
    send_ws_open_result(&tunnel_tx, &channel_id, true, None).await;
    debug!(channel_id, url, "local WebSocket channel established");
    let (mut local_write, mut local_read) = ws_stream.split();
    // Channel for messages flowing from tunnel → local WS.
    let (tx, mut rx) = mpsc::channel::<String>(LOCAL_WS_CHANNEL_CAPACITY);

    // Task: read from local WS, send through tunnel.
    let tunnel_tx_reader = Arc::clone(&tunnel_tx);
    let ch_id_reader = channel_id.clone();
    crate::util::spawn_in_span(async move {
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
    crate::util::spawn_in_span(async move {
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

/// The upgrade request for the local socket: the relay's headers minus
/// hop-by-hop ones, with the tunnel nonce set to this process's own value.
fn build_local_request(
    url: &str,
    port: u16,
    forwarded: &HashMap<String, String>,
) -> Result<ws_http::Request<()>, ws_http::Error> {
    let mut request = super::build_ws_upgrade_request(url, &format!("localhost:{port}"))?;
    for (name, value) in forwarded {
        if !super::is_hop_by_hop(name)
            && !name.eq_ignore_ascii_case(TUNNEL_NONCE_HEADER)
            && let Ok(v) = ws_http::HeaderValue::from_str(value)
            && let Ok(n) = ws_http::HeaderName::from_bytes(name.as_bytes())
        {
            request.headers_mut().insert(n, v);
        }
    }
    request.headers_mut().insert(
        ws_http::HeaderName::from_bytes(TUNNEL_NONCE_HEADER.as_bytes())?,
        ws_http::HeaderValue::from_str(tunnel_nonce())?,
    );
    Ok(request)
}

/// What to tell the relay when the local socket couldn't be opened: the
/// status when the listener answered the upgrade with a refusal, otherwise a
/// pointer to the logs.
fn connect_failure_reason(error: &tokio_tungstenite::tungstenite::Error) -> String {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        format!(
            "Residuum refused to open the socket (status {}).",
            response.status().as_u16()
        )
    } else {
        "Residuum couldn't open the socket locally. Check Residuum's logs for why.".to_string()
    }
}

/// Answer a `WsOpen` with a failed `WsOpenResult` carrying `reason`.
pub(super) async fn refuse_ws_open(
    tunnel_tx: &Arc<Mutex<TunnelSink>>,
    channel_id: &str,
    reason: &str,
) {
    send_ws_open_result(tunnel_tx, channel_id, false, Some(reason.to_string())).await;
}

/// Send a `WsOpenResult` frame through the tunnel.
async fn send_ws_open_result(
    tunnel_tx: &Arc<Mutex<TunnelSink>>,
    channel_id: &str,
    success: bool,
    reason: Option<String>,
) {
    let frame = TunnelFrame::WsOpenResult {
        channel_id: channel_id.to_string(),
        success,
        reason,
    };
    if let Err(e) = send_frame(tunnel_tx, &frame).await {
        warn!(channel_id, success, error = %e, "failed to send WsOpenResult");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(request: &'a ws_http::Request<()>, name: &str) -> Option<&'a str> {
        request.headers().get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn the_local_request_carries_the_real_nonce_not_a_spoofed_one() {
        for spoofed_name in [TUNNEL_NONCE_HEADER, "X-Residuum-Tunnel"] {
            let forwarded =
                HashMap::from([(spoofed_name.to_string(), "spoofed-value".to_string())]);
            let request =
                build_local_request("ws://localhost:7702/api/hub/ws", 7702, &forwarded).unwrap();
            assert_eq!(
                header(&request, TUNNEL_NONCE_HEADER),
                Some(tunnel_nonce()),
                "a client-supplied '{spoofed_name}' must be replaced by this process's nonce"
            );
            assert_eq!(
                request
                    .headers()
                    .get_all(TUNNEL_NONCE_HEADER)
                    .iter()
                    .count(),
                1,
                "the request must not carry the spoofed value beside the real one"
            );
        }
    }

    #[test]
    fn the_local_request_gets_the_nonce_when_the_client_sent_none() {
        let request =
            build_local_request("ws://localhost:7700/api/hub/ws", 7700, &HashMap::new()).unwrap();
        assert_eq!(
            header(&request, TUNNEL_NONCE_HEADER),
            Some(tunnel_nonce()),
            "a remote client must not be able to erase the tunnel mark by omitting it"
        );
    }

    #[test]
    fn the_local_request_forwards_end_to_end_headers_and_drops_hop_by_hop_ones() {
        let forwarded = HashMap::from([
            (
                "origin".to_string(),
                "https://bear.workbench.example".to_string(),
            ),
            ("sec-fetch-site".to_string(), "same-origin".to_string()),
            ("sec-websocket-key".to_string(), "client-key".to_string()),
            ("host".to_string(), "bear.workbench.example".to_string()),
        ]);
        let request =
            build_local_request("ws://localhost:7702/api/hub/ws", 7702, &forwarded).unwrap();
        assert_eq!(
            header(&request, "origin"),
            Some("https://bear.workbench.example"),
            "end-to-end headers reach the local listener"
        );
        assert_eq!(
            header(&request, "sec-fetch-site"),
            Some("same-origin"),
            "the cross-site guard needs the browser's own fetch metadata"
        );
        assert_eq!(
            header(&request, "host"),
            Some("localhost:7702"),
            "the relay's host header must not replace the loopback host"
        );
        assert_ne!(
            header(&request, "sec-websocket-key"),
            Some("client-key"),
            "the handshake key is the tunnel's own"
        );
    }

    #[test]
    fn a_refused_upgrade_reports_its_status_and_other_failures_point_to_the_logs() {
        let refused = tokio_tungstenite::tungstenite::Error::Http(
            ws_http::Response::builder().status(403).body(None).unwrap(),
        );
        assert_eq!(
            connect_failure_reason(&refused),
            "Residuum refused to open the socket (status 403)."
        );
        let other =
            connect_failure_reason(&tokio_tungstenite::tungstenite::Error::ConnectionClosed);
        assert!(other.contains("Check Residuum's logs"), "{other}");
    }
}

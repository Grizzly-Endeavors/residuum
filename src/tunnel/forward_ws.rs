//! WebSocket connection forwarding to the local residuum instance.
//!
//! One relay channel is one loopback connection to a local listener, carried by
//! one task until the channel ends. The channel ends from either side, and
//! either way the whole connection goes with it: the local socket gets a
//! Close frame, and both halves of the connection are dropped.
//!
//! - **The tunnel side ends it** when the relay sends `WsClose` or the tunnel
//!   connection is lost: the frame loop drops the channel's sender. The local
//!   socket is closed with the closing handshake, and the relay is told
//!   nothing, since it ended the channel or lost it with the tunnel.
//! - **The local side ends it** when the local socket closes or breaks. The
//!   relay gets one `WsClose`, and the tunnel's half of the closing handshake
//!   is finished before the connection is dropped.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use super::protocol::TunnelFrame;
use super::{TUNNEL_NONCE_HEADER, TunnelSink, send_frame, tunnel_nonce};

/// Channel capacity for messages flowing from the tunnel to local WebSocket.
const LOCAL_WS_CHANNEL_CAPACITY: usize = 64;

/// How long closing a local socket waits for the local listener (to take our
/// Close frame, or to answer it) before the connection is dropped regardless.
const LOCAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

type LocalSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type LocalSink = SplitSink<LocalSocket, Message>;
type LocalStream = SplitStream<LocalSocket>;

/// What a local socket's task reports to the tunnel's frame loop.
#[derive(Debug)]
pub(super) enum WsChannelEvent {
    /// The socket is connected: messages for the channel go to `sender`.
    Opened {
        channel_id: String,
        sender: mpsc::Sender<String>,
    },
    /// The socket's connection is gone, so the loop can drop the channel's
    /// entry. Always follows `Opened`.
    Closed { channel_id: String },
}

/// Which side ended a channel.
#[derive(Debug, Clone, Copy)]
enum ChannelEnd {
    /// The relay closed the channel, or the tunnel connection went away.
    Tunnel,
    /// The local socket closed or broke.
    Local,
}

impl ChannelEnd {
    fn closed_by(self) -> &'static str {
        match self {
            Self::Tunnel => "tunnel",
            Self::Local => "local",
        }
    }
}

/// Why the tunnel→local half stopped before the local→tunnel half did.
enum WriterEnd {
    /// The channel's sender was dropped: the tunnel side ended the channel.
    TunnelClosed,
    /// Writing to the local socket failed.
    LocalWriteFailed,
}

/// Handle a `WsOpen` frame by connecting to the local WebSocket endpoint on
/// `port`: the main listener, or the artifacts listener for a workbench-surface
/// open.
///
/// On success, sends a `WsOpenResult { success: true }` through the tunnel and
/// registers the channel with the frame loop through `events`: the `Opened`
/// event carries the `mpsc::Sender` for forwarding messages from the tunnel to
/// the local WebSocket, and a `Closed` event follows once the connection is
/// gone. On failure, sends `WsOpenResult { success: false }` with a reason and
/// registers nothing.
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
    events: mpsc::Sender<WsChannelEvent>,
) {
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
            return;
        }
    };

    let (mut socket, _response) = match tokio_tungstenite::connect_async(request).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(channel_id, url, error = %e, "failed to connect to local WebSocket");
            refuse_ws_open(&tunnel_tx, &channel_id, &connect_failure_reason(&e)).await;
            return;
        }
    };

    // Channel for messages flowing from tunnel → local WS. It's registered
    // before the relay hears the open succeeded, so the relay can't send to a
    // channel the frame loop doesn't know yet.
    let (tx, rx) = mpsc::channel::<String>(LOCAL_WS_CHANNEL_CAPACITY);
    let opened = WsChannelEvent::Opened {
        channel_id: channel_id.clone(),
        sender: tx,
    };
    if events.send(opened).await.is_err() {
        // The frame loop ended while the socket was connecting, so nothing
        // will ever send to this channel. The tunnel connection that asked
        // for it is gone, and the disconnect was logged where it happened.
        debug!(
            channel_id,
            "tunnel ended while a local WebSocket was opening; closing it"
        );
        close_local(&channel_id, &mut socket).await;
        return;
    }

    send_ws_open_result(&tunnel_tx, &channel_id, true, None).await;
    debug!(channel_id, url, "local WebSocket channel established");
    crate::util::spawn_in_span(run_channel(channel_id, socket, rx, tunnel_tx, events));
}

/// Carry a channel's traffic in both directions until it ends, then report it
/// closed to the frame loop. The two directions run concurrently in this one
/// task, so a stalled write to the local socket never holds up reading from it.
async fn run_channel(
    channel_id: String,
    socket: LocalSocket,
    rx: mpsc::Receiver<String>,
    tunnel_tx: Arc<Mutex<TunnelSink>>,
    events: mpsc::Sender<WsChannelEvent>,
) {
    let (local_write, local_read) = socket.split();
    let (writer_end_tx, writer_end_rx) = oneshot::channel();
    let (reader_done_tx, reader_done_rx) = oneshot::channel();
    let (end, ()) = tokio::join!(
        local_to_tunnel(
            &channel_id,
            local_read,
            &tunnel_tx,
            writer_end_rx,
            reader_done_tx
        ),
        tunnel_to_local(&channel_id, local_write, rx, writer_end_tx, reader_done_rx),
    );
    // Both halves of the connection are dropped by now, and the receiver with
    // them, so the loop's entry for the channel is dead.
    debug!(
        channel_id,
        closed_by = end.closed_by(),
        "local WebSocket channel closed"
    );
    // Fails only when the frame loop has ended, and with it the entry.
    events
        .send(WsChannelEvent::Closed { channel_id })
        .await
        .ok();
}

/// Read from the local WS and send through the tunnel, until the channel ends
/// from either side. When the local side ended it, tell the relay once.
async fn local_to_tunnel(
    channel_id: &str,
    mut local_read: LocalStream,
    tunnel_tx: &Arc<Mutex<TunnelSink>>,
    mut writer_end: oneshot::Receiver<WriterEnd>,
    reader_done: oneshot::Sender<()>,
) -> ChannelEnd {
    let end = loop {
        tokio::select! {
            biased;
            stopped = &mut writer_end => match stopped {
                Ok(WriterEnd::TunnelClosed) => {
                    await_close_reply(channel_id, &mut local_read).await;
                    break ChannelEnd::Tunnel;
                }
                Ok(WriterEnd::LocalWriteFailed) | Err(_) => break ChannelEnd::Local,
            },
            msg = local_read.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let frame = TunnelFrame::WsMessage {
                        channel_id: channel_id.to_string(),
                        data: text.to_string(),
                    };
                    if let Err(e) = send_frame(tunnel_tx, &frame).await {
                        warn!(channel_id, error = %e, "failed to forward local WS message to tunnel");
                        break ChannelEnd::Tunnel;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break ChannelEnd::Local,
                Some(Ok(_)) => {
                    // Ignore binary, ping, pong frames from local.
                }
                Some(Err(e)) => {
                    warn!(channel_id, error = %e, "local WebSocket read error");
                    break ChannelEnd::Local;
                }
            },
        }
    };

    // The writer finishes the local side's closing handshake and drops its half
    // of the connection once it hears this.
    reader_done.send(()).ok();

    if matches!(end, ChannelEnd::Local) {
        let close_frame = TunnelFrame::WsClose {
            channel_id: channel_id.to_string(),
        };
        if let Err(e) = send_frame(tunnel_tx, &close_frame).await {
            warn!(channel_id, error = %e, "failed to send WsClose to relay; relay may hold a zombie channel");
        }
    }
    end
}

/// Read from the channel's mpsc and send to the local WS, until the channel
/// ends from either side. The local socket is closed on the way out.
async fn tunnel_to_local(
    channel_id: &str,
    mut local_write: LocalSink,
    mut rx: mpsc::Receiver<String>,
    writer_end: oneshot::Sender<WriterEnd>,
    mut reader_done: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            biased;
            // The local side closed, or the tunnel failed under the reader.
            // Either way, the local socket still owes its peer a Close frame.
            _ = &mut reader_done => {
                close_local(channel_id, &mut local_write).await;
                return;
            }
            data = rx.recv() => {
                let Some(data) = data else {
                    // Tell the reader first, so it stops forwarding whatever
                    // the listener sends while it closes.
                    writer_end.send(WriterEnd::TunnelClosed).ok();
                    close_local(channel_id, &mut local_write).await;
                    return;
                };
                if let Err(e) = local_write.send(Message::Text(data.into())).await {
                    warn!(channel_id, error = %e, "failed to forward tunnel message to local WS");
                    writer_end.send(WriterEnd::LocalWriteFailed).ok();
                    return;
                }
            }
        }
    }
}

/// Send the local socket its Close frame (or, when the local side closed
/// first, the reply to its own) and flush it, so the listener sees the
/// socket close instead of a connection that dies without a word.
async fn close_local<S>(channel_id: &str, socket: &mut S)
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match tokio::time::timeout(LOCAL_CLOSE_TIMEOUT, socket.close()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            debug!(channel_id, error = %e, "local WebSocket was already gone when closing it");
        }
        Err(_) => {
            warn!(
                channel_id,
                timeout_secs = LOCAL_CLOSE_TIMEOUT.as_secs(),
                "local WebSocket didn't take the close frame in time; dropping the connection"
            );
        }
    }
}

/// Wait for the local listener to answer the Close frame the tunnel sent, so
/// the closing handshake finishes before the connection is dropped. Whatever
/// else the listener still sends is discarded: the channel is closed.
async fn await_close_reply(channel_id: &str, local_read: &mut LocalStream) {
    let reply = async {
        // The stream ending or failing also means the connection is over.
        while let Some(Ok(frame)) = local_read.next().await {
            if matches!(frame, Message::Close(_)) {
                return;
            }
        }
    };
    if tokio::time::timeout(LOCAL_CLOSE_TIMEOUT, reply)
        .await
        .is_err()
    {
        warn!(
            channel_id,
            timeout_secs = LOCAL_CLOSE_TIMEOUT.as_secs(),
            "local WebSocket didn't answer the close frame in time; dropping the connection"
        );
    }
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

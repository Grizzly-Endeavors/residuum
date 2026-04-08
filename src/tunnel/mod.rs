//! Tunnel client module.
//!
//! Maintains a persistent WebSocket connection to the cloud relay, forwarding
//! HTTP requests and WebSocket connections to the local residuum instance.

mod connection;
mod forward_http;
mod forward_ws;
pub(crate) mod protocol;

pub(crate) use connection::start_tunnel;

use std::sync::Arc;

use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http as ws_http;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use protocol::TunnelFrame;

/// Hop-by-hop headers that must not be forwarded between proxy and backend.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "transfer-encoding",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
    "host",
    "sec-websocket-version",
    "sec-websocket-key",
];

/// Returns `true` if the given header name is a hop-by-hop header.
#[must_use]
fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP_HEADERS
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

type TunnelSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

fn build_ws_upgrade_request(uri: &str, host: &str) -> Result<ws_http::Request<()>, ws_http::Error> {
    ws_http::Request::builder()
        .uri(uri)
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
}

async fn send_frame(
    write: &Arc<Mutex<TunnelSink>>,
    frame: &TunnelFrame,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(frame)?;
    let mut guard = write.lock().await;
    guard.send(Message::Text(json.into())).await?;
    Ok(())
}

/// Current status of the tunnel connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunnelStatus {
    /// Not connected to the relay.
    Disconnected,
    /// Attempting to connect to the relay.
    Connecting,
    /// Connected and authenticated with the relay.
    Connected {
        /// The user ID associated with this tunnel.
        user_id: String,
    },
}

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
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use protocol::TunnelFrame;

type TunnelSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

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

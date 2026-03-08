//! Tunnel client module.
//!
//! Maintains a persistent WebSocket connection to the cloud relay, forwarding
//! HTTP requests and WebSocket connections to the local residuum instance.

mod connection;
mod forward_http;
mod forward_ws;
pub(crate) mod protocol;

pub(crate) use connection::start_tunnel;

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

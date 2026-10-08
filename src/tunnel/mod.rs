//! Tunnel client module.
//!
//! Keeps a persistent WebSocket connection to the cloud relay open on the
//! relay's `/tunnel/v2/register` endpoint. The relay hands this instance raw
//! byte streams, one per browser connection, and the instance terminates TLS
//! itself (see [`v2`]).

mod connection;
pub(crate) mod v2;

pub(crate) use connection::start_tunnel;

/// Current status of the tunnel connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelStatus {
    /// Not connected to the relay.
    Disconnected,
    /// Attempting to connect to the relay.
    Connecting,
    /// Connected to the relay, which Residuum accepted as the one it was set
    /// up with.
    Connected {
        /// The user ID associated with this tunnel.
        user_id: String,
        /// Public origin of the web UI through the relay, when the relay
        /// announces it.
        origin: Option<String>,
        /// Public origin of the workbench artifacts through the relay, when the
        /// relay announces it.
        workbench_origin: Option<String>,
        /// This instance's slug, as the relay knows it.
        instance: Option<String>,
        /// Public origin of this instance's own address,
        /// `https://{slug}.{user}.{base}`, where its A2A and Teams endpoints
        /// are served.
        instance_origin: Option<String>,
    },
}

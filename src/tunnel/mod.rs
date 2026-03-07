//! Tunnel client module.
//!
//! Maintains a persistent WebSocket connection to the cloud relay, forwarding
//! HTTP requests and WebSocket connections to the local residuum instance.

mod connection;
mod forward_http;
mod forward_ws;
pub(crate) mod protocol;

pub(crate) use connection::start_tunnel;

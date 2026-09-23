//! Tunnel client module.
//!
//! Maintains a persistent WebSocket connection to the cloud relay, forwarding
//! HTTP requests and WebSocket connections to the local residuum instance.

mod connection;
mod forward_a2a;
mod forward_http;
mod forward_ws;
pub(crate) mod protocol;

pub(crate) use connection::start_tunnel;

use std::sync::{Arc, OnceLock};

use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use rand::Rng;
use rand::distributions::Alphanumeric;
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

/// Local listeners the tunnel forwards to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForwardTargets {
    /// The main gateway listener: web UI, API, WebSocket.
    pub main: u16,
    /// The workbench artifacts listener, when it is running.
    pub workbench: Option<u16>,
    /// The A2A listener, when it is running.
    pub a2a: Option<u16>,
}

/// Header listing optional features this tunnel client supports.
const CAPABILITIES_HEADER: &str = "x-residuum-capabilities";

/// Capability: requests tagged [`protocol::Surface::Workbench`] are routed to
/// the workbench artifacts listener. Advertised even while that listener is down,
/// so the relay forwards artifact requests and they get an explanation back
/// rather than the relay's generic "update Residuum" page.
const WORKBENCH_SURFACE_CAPABILITY: &str = "workbench-surface";

/// Capability: this tunnel client answers A2A-surface (and future streamed)
/// requests with `HttpResponseStart`/`HttpResponseChunk`/`HttpResponseEnd`
/// instead of a single buffered `HttpResponse`. Always advertised.
const HTTP_STREAMING_CAPABILITY: &str = "http-streaming";

/// Capability: this instance has A2A enabled and will answer requests on the
/// [`protocol::Surface::A2a`] surface.
const A2A_CAPABILITY: &str = "a2a";

/// Capability: this instance's A2A agent is private — only a caller with a
/// valid key, or an attested sibling, can see or reach it.
const A2A_PRIVATE_CAPABILITY: &str = "a2a-private";

/// Whether an A2A-enabled instance is publicly listed and reachable, or
/// hidden behind a caller key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum A2aVisibility {
    /// Listed in the directory and reachable by anyone. Residuum's agent
    /// tools default to open, so this is the default visibility too.
    #[default]
    Public,
    /// Hidden from anyone without a valid key or sibling attestation.
    Private,
}

/// A2A capability advertised on the tunnel upgrade. `None` means A2A is
/// disabled and the `a2a`/`a2a-private` capabilities are omitted entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TunnelA2a {
    pub visibility: A2aVisibility,
}

/// Build the `x-residuum-capabilities` header value: always
/// `workbench-surface,http-streaming`, plus `a2a` when A2A is enabled and
/// `a2a-private` when its visibility is private.
#[must_use]
fn build_capabilities_header(a2a: Option<TunnelA2a>) -> String {
    let mut capabilities = vec![WORKBENCH_SURFACE_CAPABILITY, HTTP_STREAMING_CAPABILITY];
    if let Some(a2a) = a2a {
        capabilities.push(A2A_CAPABILITY);
        if a2a.visibility == A2aVisibility::Private {
            capabilities.push(A2A_PRIVATE_CAPABILITY);
        }
    }
    capabilities.join(",")
}

/// Header the tunnel adds to every request it forwards to the local A2A
/// listener, carrying [`tunnel_nonce`]. The A2A listener's auth layer uses it
/// to tell a genuinely tunnel-forwarded sibling request apart from anyone who
/// connects to the A2A port directly and forges the sibling header themselves.
pub(crate) const TUNNEL_NONCE_HEADER: &str = "x-residuum-tunnel";

/// Length, in characters, of the per-process tunnel nonce.
const TUNNEL_NONCE_LEN: usize = 32;

static TUNNEL_NONCE: OnceLock<String> = OnceLock::new();

/// The per-process nonce sent as [`TUNNEL_NONCE_HEADER`] on every request the
/// tunnel forwards to the local A2A listener.
///
/// Generated once per process with a CSPRNG and never persisted, so it changes
/// on every restart. That's fine because both sides of the comparison — this
/// function and the A2A listener's auth layer — live in the same process.
#[must_use]
pub(crate) fn tunnel_nonce() -> &'static str {
    TUNNEL_NONCE.get_or_init(|| {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(TUNNEL_NONCE_LEN)
            .map(char::from)
            .collect()
    })
}

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

/// Fields of an HTTP request being forwarded to a local listener, bundled so
/// the forwarding functions that need all of them stay within the arg-count
/// lint.
pub(crate) struct ForwardRequest {
    pub request_id: String,
    pub method: String,
    pub path: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: Option<String>,
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
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum TunnelStatus {
    /// Not connected to the relay.
    Disconnected,
    /// Attempting to connect to the relay.
    Connecting,
    /// Connected and authenticated with the relay.
    Connected {
        /// The user ID associated with this tunnel.
        user_id: String,
        /// Public origin of the web UI through the relay, when the relay
        /// announces it.
        origin: Option<String>,
        /// Public origin of the workbench artifacts through the relay, when the
        /// relay announces it.
        workbench_origin: Option<String>,
        /// This instance's slug, as the relay knows it. Combined with `origin`
        /// this builds the public A2A URL `{origin}/a2a/{instance}`.
        instance: Option<String>,
        /// Sibling credential minted for this connection, valid only while
        /// this tunnel is connected. Never logged: [`TunnelStatus`]'s `Debug`
        /// impl redacts it.
        a2a_token: Option<String>,
    },
}

impl std::fmt::Debug for TunnelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected {
                user_id,
                origin,
                workbench_origin,
                instance,
                a2a_token,
            } => f
                .debug_struct("Connected")
                .field("user_id", user_id)
                .field("origin", origin)
                .field("workbench_origin", workbench_origin)
                .field("instance", instance)
                .field("a2a_token", &a2a_token.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_header_without_a2a() {
        assert_eq!(
            build_capabilities_header(None),
            "workbench-surface,http-streaming"
        );
    }

    #[test]
    fn capabilities_header_with_public_a2a() {
        let header = build_capabilities_header(Some(TunnelA2a {
            visibility: A2aVisibility::Public,
        }));
        assert_eq!(header, "workbench-surface,http-streaming,a2a");
    }

    #[test]
    fn capabilities_header_with_private_a2a() {
        let header = build_capabilities_header(Some(TunnelA2a {
            visibility: A2aVisibility::Private,
        }));
        assert_eq!(header, "workbench-surface,http-streaming,a2a,a2a-private");
    }

    #[test]
    fn tunnel_nonce_is_stable_and_alphanumeric() {
        let first = tunnel_nonce();
        let second = tunnel_nonce();
        assert_eq!(first, second, "the nonce must not change within a process");
        assert_eq!(first.len(), TUNNEL_NONCE_LEN);
        assert!(
            first.chars().all(|c| c.is_ascii_alphanumeric()),
            "nonce should be alphanumeric, got {first}"
        );
    }

    #[test]
    fn connected_status_debug_redacts_the_a2a_token() {
        let status = TunnelStatus::Connected {
            user_id: "bear".to_string(),
            origin: None,
            workbench_origin: None,
            instance: Some("laptop".to_string()),
            a2a_token: Some("rsa_super-secret-token".to_string()),
        };
        let debug = format!("{status:?}");
        assert!(
            !debug.contains("rsa_super-secret-token"),
            "Debug output must never contain the raw a2a_token, got: {debug}"
        );
        assert!(debug.contains("laptop"), "other fields should still print");
    }
}

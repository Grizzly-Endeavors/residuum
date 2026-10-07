//! Tunnel v2 client: raw byte streams for TLS passthrough plus the control
//! frames around them (keepalive, agent list, challenge claims, pin grants).
//!
//! The relay opens a stream for each browser connection that belongs to this
//! instance. [`SessionHandler`] receives those streams as [`TunnelIo`]
//! handles and owns every trust decision about the relay's claims.

pub(crate) mod frames;
mod link;
mod session;
mod stream;
#[cfg(test)]
pub(crate) mod test_relay;
#[cfg(test)]
mod tests;

use async_trait::async_trait;

use crate::remote_access::types::Hostnames;

pub(crate) use link::{ClaimError, RelayLink};
pub(super) use session::{ConnectOutcome, SessionEnd, SessionInputs, connect, run_session};
pub(crate) use stream::TunnelIo;

/// Path of the v1 registration endpoint.
const V1_REGISTER_PATH: &str = "/tunnel/register";

/// Path of the v2 registration endpoint.
const V2_REGISTER_PATH: &str = "/tunnel/v2/register";

/// The v2 registration URL for a v1 relay URL, or `None` when the URL does
/// not end in the v1 registration path (v2 is then not attempted).
#[must_use]
pub(crate) fn register_url(relay_url: &str) -> Option<String> {
    let mut url = url::Url::parse(relay_url).ok()?;
    let prefix = url.path().strip_suffix(V1_REGISTER_PATH)?.to_string();
    url.set_path(&format!("{prefix}{V2_REGISTER_PATH}"));
    Some(url.to_string())
}

/// A browser connection the relay handed to this instance.
pub(crate) struct IncomingStream {
    /// The host the browser asked for, normalized by the relay.
    pub host: String,
    /// The browser's IP address as the relay saw it.
    pub peer_ip: String,
    /// The connection's bytes.
    pub io: TunnelIo,
}

/// What the relay claimed in its first frame. Untrusted: the handler decides.
#[derive(Debug, Clone)]
pub(crate) struct ConnectedInfo {
    pub user: String,
    pub instance: String,
    /// The host names exactly as the relay claimed them.
    pub hosts: Hostnames,
    pub keepalive_interval_secs: u64,
}

/// The handler's decision about a relay's `connected` frame.
pub(crate) enum Verdict {
    /// Proceed; the tunnel client publishes this identity and origins.
    Accept {
        user: String,
        instance: String,
        ui_origin: Option<String>,
        workbench_origin: Option<String>,
    },
    /// Do not use this relay; the reason is logged.
    Refuse(String),
}

/// Consumer of a v2 tunnel session.
#[async_trait]
pub(crate) trait SessionHandler: Send + Sync {
    /// First frame after registration. The handler verifies identity (it owns
    /// all trust decisions). The tunnel keeps serving the relay's frames
    /// while this runs, so `link` requests work from inside it.
    async fn on_connected(&self, connected: &ConnectedInfo, link: RelayLink) -> Verdict;

    /// A new browser stream. Must not block.
    fn on_stream(&self, stream: IncomingStream);

    /// The session ended (any reason). Streams are already closed.
    fn on_disconnected(&self);

    /// Whether a relay that answers 404/426 on the v2 endpoint may be served
    /// by the v1 client.
    fn allow_v1_fallback(&self) -> bool;
}

#[cfg(test)]
mod url_tests {
    use super::register_url;

    #[test]
    fn v1_path_becomes_v2_path() {
        assert_eq!(
            register_url("wss://agent-residuum.com/tunnel/register").as_deref(),
            Some("wss://agent-residuum.com/tunnel/v2/register")
        );
        assert_eq!(
            register_url("ws://127.0.0.1:9/pre/tunnel/register?x=1").as_deref(),
            Some("ws://127.0.0.1:9/pre/tunnel/v2/register?x=1")
        );
    }

    #[test]
    fn other_paths_do_not_try_v2() {
        assert_eq!(register_url("wss://relay.example.com/ws"), None);
        assert_eq!(register_url("wss://relay.example.com"), None);
        assert_eq!(register_url("not a url"), None);
        assert_eq!(
            register_url("wss://relay.example.com/tunnel/register/x"),
            None
        );
    }
}

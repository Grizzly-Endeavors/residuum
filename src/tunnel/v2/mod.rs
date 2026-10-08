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

/// Path of the registration endpoint.
const REGISTER_PATH: &str = "/tunnel/v2/register";

/// Path an earlier configuration's `relay_url` ends in; the registration
/// endpoint replaces it.
const PREVIOUS_REGISTER_PATH: &str = "/tunnel/register";

/// The registration URL for the relay URL in `[cloud] relay_url`, or `None`
/// when it ends in neither `/tunnel/v2/register` nor `/tunnel/register`
/// (the latter is rewritten to the former, keeping any prefix and query).
#[must_use]
pub(crate) fn register_url(relay_url: &str) -> Option<String> {
    let mut url = url::Url::parse(relay_url).ok()?;
    if url.path().ends_with(REGISTER_PATH) {
        return Some(url.to_string());
    }
    let prefix = url.path().strip_suffix(PREVIOUS_REGISTER_PATH)?.to_string();
    url.set_path(&format!("{prefix}{REGISTER_PATH}"));
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
#[derive(Clone)]
pub(crate) struct ConnectedInfo {
    pub user: String,
    pub instance: String,
    /// The host names exactly as the relay claimed them.
    pub hosts: Hostnames,
    pub keepalive_interval_secs: u64,
    /// The credential that lets this connection see the user's private agents
    /// in the apex directory. Nothing else accepts it.
    pub a2a_token: String,
}

/// `Debug` that never prints the directory token.
impl std::fmt::Debug for ConnectedInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectedInfo")
            .field("user", &self.user)
            .field("instance", &self.instance)
            .field("hosts", &self.hosts)
            .field("keepalive_interval_secs", &self.keepalive_interval_secs)
            .field("a2a_token", &"<redacted>")
            .finish()
    }
}

/// The handler's decision about a relay's `connected` frame.
pub(crate) enum Verdict {
    /// Proceed; the tunnel client publishes this identity and origins.
    Accept {
        user: String,
        instance: String,
        ui_origin: Option<String>,
        workbench_origin: Option<String>,
        /// Where this instance's A2A and Teams endpoints are, when known.
        instance_origin: Option<String>,
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

    /// The relay's current list of the user's instances. Untrusted text.
    fn on_instances(&self, _instances: Vec<frames::InstanceSummary>) {}

    /// The session ended (any reason). Streams are already closed.
    fn on_disconnected(&self);

    /// The relay answered 404 or 426 on the registration endpoint: it doesn't
    /// offer the secure tunnel. The tunnel keeps retrying; this is where the
    /// handler reports why remote access is down.
    fn on_relay_unsupported(&self);
}

#[cfg(test)]
mod url_tests {
    use super::register_url;

    #[test]
    fn the_registration_path_is_used_as_is() {
        assert_eq!(
            register_url("wss://agent-residuum.com/tunnel/v2/register").as_deref(),
            Some("wss://agent-residuum.com/tunnel/v2/register")
        );
    }

    #[test]
    fn the_earlier_registration_path_becomes_the_current_one() {
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
    fn other_paths_have_no_registration_url() {
        assert_eq!(register_url("wss://relay.example.com/ws"), None);
        assert_eq!(register_url("wss://relay.example.com"), None);
        assert_eq!(register_url("not a url"), None);
        assert_eq!(
            register_url("wss://relay.example.com/tunnel/register/x"),
            None
        );
    }
}

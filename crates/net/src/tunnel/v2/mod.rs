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

const REGISTER_PATH: &str = "/tunnel/v2/register";

const PREVIOUS_REGISTER_PATH: &str = "/tunnel/register";

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

#[must_use]
pub(crate) fn web_origin(relay_url: &str) -> Option<String> {
    let url = url::Url::parse(relay_url).ok()?;
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        _ => return None,
    };
    let host = url.host_str()?;
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Some(format!("{scheme}://{host}{port}"))
}

pub(crate) struct IncomingStream {
    pub host: String,
    pub peer_ip: String,
    pub io: TunnelIo,
}

#[derive(Clone)]
pub(crate) struct ConnectedInfo {
    pub user: String,
    pub instance: String,
    pub hosts: Hostnames,
    pub keepalive_interval_secs: u64,
    pub a2a_token: String,
}

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

pub(crate) enum Verdict {
    Accept {
        user: String,
        instance: String,
        ui_origin: Option<String>,
        workbench_origin: Option<String>,
        instance_origin: Option<String>,
    },
    Refuse(String),
}

#[async_trait]
pub(crate) trait SessionHandler: Send + Sync {
    async fn on_connected(&self, connected: &ConnectedInfo, link: RelayLink) -> Verdict;

    fn on_stream(&self, stream: IncomingStream);

    fn on_instances(&self, _instances: Vec<frames::InstanceSummary>) {}

    fn on_disconnected(&self);

    /// The relay answered 404 or 426 on the registration endpoint: it doesn't
    /// offer the secure tunnel. The tunnel keeps retrying; this is where the
    /// handler reports why remote access is down.
    fn on_relay_unsupported(&self);
}

#[cfg(test)]
mod url_tests {
    use super::{register_url, web_origin};

    #[test]
    fn the_web_origin_swaps_the_relay_scheme_and_keeps_the_host() {
        assert_eq!(
            web_origin("wss://agent-residuum.com/tunnel/v2/register").as_deref(),
            Some("https://agent-residuum.com")
        );
        assert_eq!(
            web_origin("ws://127.0.0.1:8080/tunnel/register?x=1").as_deref(),
            Some("http://127.0.0.1:8080")
        );
    }

    #[test]
    fn the_web_origin_drops_a_default_port() {
        assert_eq!(
            web_origin("wss://relay.example.com:443/tunnel/v2/register").as_deref(),
            Some("https://relay.example.com")
        );
    }

    #[test]
    fn the_web_origin_keeps_an_ipv6_host_in_brackets() {
        assert_eq!(
            web_origin("wss://[::1]:8443/tunnel/v2/register").as_deref(),
            Some("https://[::1]:8443")
        );
    }

    #[test]
    fn the_web_origin_drops_any_userinfo() {
        assert_eq!(
            web_origin("wss://user:secret@relay.example.com/tunnel/v2/register").as_deref(),
            Some("https://relay.example.com")
        );
    }

    #[test]
    fn a_url_that_isnt_a_websocket_address_has_no_web_origin() {
        assert_eq!(web_origin("https://relay.example.com"), None);
        assert_eq!(web_origin("not a url"), None);
    }

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

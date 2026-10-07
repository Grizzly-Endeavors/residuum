//! Telling a remotely delivered request from a local one, and the checks only
//! a remote request gets.
//!
//! A request is remote when a transport that carries traffic from Residuum
//! Cloud marked it:
//!
//! - the relay tunnel marks every request it forwards with the process's
//!   tunnel nonce header (`tunnel::is_tunnel_forwarded`), and the peer's
//!   address comes from the forwarding headers the relay passes along;
//! - a transport that terminates TLS inside Residuum marks the request itself,
//!   by adding a [`RemoteTransport`] extension before the request enters the
//!   router. The extension can't be sent by a client, so it needs no secret.
//!
//! Anything else arrived on a local port and is not subject to the gate.

use axum::http::{Extensions, HeaderMap, Method, header};

/// Added to a request's extensions by a transport that delivers remote
/// traffic in-process. Its fields override what the headers would say.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteTransport {
    /// The browser's address, when the transport knows it.
    pub(crate) peer_ip: Option<String>,
    /// The origin the browser used to reach this request's host.
    pub(crate) origin: Option<String>,
}

/// What is known about a remotely delivered request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RemoteContext {
    /// The browser's address as the relay reported it.
    pub(crate) peer_ip: Option<String>,
    /// The origin the browser used, when the transport says. Otherwise the
    /// origin Residuum Cloud announced for the surface stands in for it.
    pub(crate) origin: Option<String>,
}

/// Whether the request was delivered remotely, and if so what is known of it.
///
/// The peer address comes from `X-Real-IP`, which the relay's reverse proxy
/// sets to the connecting address, or failing that the last entry of
/// `X-Forwarded-For`. The relay passes both through the tunnel unchanged. A
/// compromised relay can set them to anything, which only lets it dodge or
/// trigger the per-address rate limit.
pub(crate) fn remote_context(
    headers: &HeaderMap,
    extensions: &Extensions,
) -> Option<RemoteContext> {
    if let Some(transport) = extensions.get::<RemoteTransport>() {
        return Some(RemoteContext {
            peer_ip: transport.peer_ip.clone(),
            origin: transport.origin.clone(),
        });
    }
    crate::tunnel::is_tunnel_forwarded(headers).then(|| RemoteContext {
        peer_ip: peer_ip_from_headers(headers),
        origin: None,
    })
}

fn peer_ip_from_headers(headers: &HeaderMap) -> Option<String> {
    let real_ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_ip);
    real_ip.or_else(|| {
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|list| list.rsplit(',').next())
            .and_then(parse_ip)
    })
}

fn parse_ip(text: &str) -> Option<String> {
    text.trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_canonical().to_string())
}

/// Whether the request changes state or opens a socket: anything but a plain
/// `GET`, `HEAD` or `OPTIONS`, and any WebSocket upgrade.
pub(crate) fn is_state_changing(method: &Method, headers: &HeaderMap) -> bool {
    let is_websocket_upgrade = headers
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    is_websocket_upgrade || !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The cross-site rule for a remote request: a state-changing request or
/// WebSocket upgrade must say it came from the same origin.
///
/// `Sec-Fetch-Site` must be `same-origin` or `none`. A browser that doesn't
/// send it must send an `Origin` equal to the request's own origin
/// (`own_origin`). Neither is a rejection: other users' hosts are the same
/// site as this one, so a request without proof of origin could be another
/// page riding the cookie.
pub(crate) fn passes_cross_site_rule(
    method: &Method,
    headers: &HeaderMap,
    own_origin: Option<&str>,
) -> bool {
    if !is_state_changing(method, headers) {
        return true;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        return site.as_bytes().eq_ignore_ascii_case(b"same-origin")
            || site.as_bytes().eq_ignore_ascii_case(b"none");
    }
    match (
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        own_origin,
    ) {
        (Some(origin), Some(own)) => origins_equal(origin, own),
        _ => false,
    }
}

fn origins_equal(a: &str, b: &str) -> bool {
    a.trim()
        .trim_end_matches('/')
        .eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

/// Whether the request is a browser navigation, which gets a redirect to the
/// pairing page where anything else gets a `401`.
pub(crate) fn is_navigation(method: &Method, headers: &HeaderMap) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) || is_state_changing(method, headers) {
        return false;
    }
    match headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) {
        Some(mode) => mode.eq_ignore_ascii_case("navigate"),
        None => headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html")),
    }
}

/// Normalize an origin Residuum Cloud announced to `scheme://host[:port]`, or
/// `None` when it isn't one.
pub(crate) fn normalize_origin(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;
    use crate::tunnel::{TUNNEL_NONCE_HEADER, tunnel_nonce};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn a_request_with_the_tunnel_nonce_is_remote() {
        let h = headers(&[(TUNNEL_NONCE_HEADER, tunnel_nonce())]);
        assert!(remote_context(&h, &Extensions::new()).is_some());
    }

    #[test]
    fn a_request_without_the_nonce_or_with_a_forged_one_is_local() {
        assert!(remote_context(&HeaderMap::new(), &Extensions::new()).is_none());
        let forged = headers(&[(TUNNEL_NONCE_HEADER, "guess")]);
        assert!(remote_context(&forged, &Extensions::new()).is_none());
    }

    #[test]
    fn a_transport_extension_marks_a_request_remote_without_a_header() {
        let mut extensions = Extensions::new();
        extensions.insert(RemoteTransport {
            peer_ip: Some("203.0.113.9".to_string()),
            origin: Some("https://bear.example".to_string()),
        });
        let context = remote_context(&HeaderMap::new(), &extensions).unwrap();
        assert_eq!(context.peer_ip.as_deref(), Some("203.0.113.9"));
        assert_eq!(context.origin.as_deref(), Some("https://bear.example"));
    }

    #[test]
    fn the_peer_address_prefers_x_real_ip_then_the_last_forwarded_entry() {
        let both = headers(&[
            (TUNNEL_NONCE_HEADER, tunnel_nonce()),
            ("x-real-ip", "198.51.100.7"),
            ("x-forwarded-for", "10.0.0.1, 198.51.100.8"),
        ]);
        assert_eq!(
            remote_context(&both, &Extensions::new())
                .unwrap()
                .peer_ip
                .as_deref(),
            Some("198.51.100.7")
        );
        let forwarded_only = headers(&[
            (TUNNEL_NONCE_HEADER, tunnel_nonce()),
            ("x-forwarded-for", "10.0.0.1, 198.51.100.8"),
        ]);
        assert_eq!(
            remote_context(&forwarded_only, &Extensions::new())
                .unwrap()
                .peer_ip
                .as_deref(),
            Some("198.51.100.8"),
            "the entry nearest the relay is the one a client can't choose"
        );
        let junk = headers(&[
            (TUNNEL_NONCE_HEADER, tunnel_nonce()),
            ("x-real-ip", "not an ip"),
        ]);
        assert_eq!(
            remote_context(&junk, &Extensions::new()).unwrap().peer_ip,
            None
        );
    }

    #[test]
    fn ipv4_mapped_addresses_are_the_same_peer_as_plain_ipv4() {
        assert_eq!(parse_ip("::ffff:192.0.2.1").as_deref(), Some("192.0.2.1"));
        assert_eq!(parse_ip("2001:db8::1").as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn reads_never_need_proof_of_origin() {
        assert!(passes_cross_site_rule(
            &Method::GET,
            &HeaderMap::new(),
            None
        ));
    }

    #[test]
    fn a_write_needs_same_origin_or_none() {
        for site in ["same-origin", "none"] {
            let h = headers(&[("sec-fetch-site", site)]);
            assert!(passes_cross_site_rule(&Method::POST, &h, None), "{site}");
        }
        for site in ["same-site", "cross-site"] {
            let h = headers(&[("sec-fetch-site", site)]);
            assert!(!passes_cross_site_rule(&Method::POST, &h, None), "{site}");
        }
    }

    #[test]
    fn a_write_without_fetch_metadata_needs_a_matching_origin() {
        let own = Some("https://bear.agent-residuum.com");
        let matching = headers(&[("origin", "https://bear.agent-residuum.com")]);
        assert!(passes_cross_site_rule(&Method::POST, &matching, own));
        let other = headers(&[("origin", "https://mallory.agent-residuum.com")]);
        assert!(!passes_cross_site_rule(&Method::POST, &other, own));
        assert!(
            !passes_cross_site_rule(&Method::POST, &HeaderMap::new(), own),
            "neither header is a rejection"
        );
        assert!(
            !passes_cross_site_rule(&Method::POST, &matching, None),
            "an origin can't match one that is unknown"
        );
    }

    #[test]
    fn a_websocket_upgrade_is_held_to_the_same_rule_as_a_write() {
        let upgrade = headers(&[("upgrade", "websocket"), ("sec-fetch-site", "same-site")]);
        assert!(!passes_cross_site_rule(&Method::GET, &upgrade, None));
        let same = headers(&[("upgrade", "websocket"), ("sec-fetch-site", "same-origin")]);
        assert!(passes_cross_site_rule(&Method::GET, &same, None));
    }

    #[test]
    fn navigations_are_told_apart_from_script_requests() {
        let nav = headers(&[("sec-fetch-mode", "navigate")]);
        assert!(is_navigation(&Method::GET, &nav));
        let fetch = headers(&[("sec-fetch-mode", "cors")]);
        assert!(!is_navigation(&Method::GET, &fetch));
        let accept = headers(&[("accept", "text/html,application/xhtml+xml")]);
        assert!(is_navigation(&Method::GET, &accept));
        assert!(!is_navigation(&Method::POST, &nav));
        let socket = headers(&[("upgrade", "websocket"), ("sec-fetch-mode", "websocket")]);
        assert!(!is_navigation(&Method::GET, &socket));
    }

    #[test]
    fn origins_normalize() {
        assert_eq!(
            normalize_origin("https://Bear.Agent-Residuum.com/").as_deref(),
            Some("https://bear.agent-residuum.com")
        );
        assert_eq!(
            normalize_origin("http://bear.localhost:7000").as_deref(),
            Some("http://bear.localhost:7000")
        );
        assert_eq!(normalize_origin("javascript:alert(1)"), None);
        assert_eq!(normalize_origin("not a url"), None);
    }
}

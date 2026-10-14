//! Telling a remotely delivered request from a local one, and the checks only
//! a remote request gets.
//!
//! A request is remote when the transport that terminates TLS inside
//! Residuum marked it, by adding a [`RemoteTransport`] extension before the
//! request enters the router. The extension can't be sent by a client, so it
//! needs no secret.
//!
//! Anything else arrived on a local port and is not subject to the gate.

use axum::http::{Extensions, HeaderMap, Method, header};

/// Added to a request's extensions by the transport that delivers remote
/// traffic in-process.
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
    /// The browser's address, when the transport knows it.
    pub(crate) peer_ip: Option<String>,
    /// The origin the browser used, when the transport says. Otherwise the
    /// origin Residuum Cloud announced for the surface stands in for it.
    pub(crate) origin: Option<String>,
}

/// Whether the request was delivered remotely, and if so what is known of it.
///
/// Only the [`RemoteTransport`] extension counts. Headers a client sends
/// (`X-Real-IP`, `X-Forwarded-For`, anything naming the tunnel) never mark a
/// request remote, and never name its peer.
pub(crate) fn remote_context(extensions: &Extensions) -> Option<RemoteContext> {
    extensions
        .get::<RemoteTransport>()
        .map(|transport| RemoteContext {
            peer_ip: transport.peer_ip.clone(),
            origin: transport.origin.clone(),
        })
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

/// Whether the request loads a page a person will see, which gets a redirect
/// to the pairing page where a script's request gets a `401`.
///
/// Any one sign is enough: fetch metadata saying it navigates or loads a
/// document or frame, or an `Accept` asking for HTML. A page load a service
/// worker passes on may arrive with its fetch metadata changed but keeps its
/// `Accept`, and a script's `fetch` asks for HTML only if it means to show it.
pub(crate) fn is_navigation(method: &Method, headers: &HeaderMap) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) || is_state_changing(method, headers) {
        return false;
    }
    let header_is = |name: &str, wanted: &[&str]| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| wanted.iter().any(|w| value.eq_ignore_ascii_case(w)))
    };
    let accepts_html = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    header_is("sec-fetch-mode", &["navigate"])
        || header_is("sec-fetch-dest", &["document", "iframe", "frame"])
        || accepts_html
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

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn a_request_without_the_transport_extension_is_local() {
        assert!(remote_context(&Extensions::new()).is_none());
    }

    #[test]
    fn a_transport_extension_marks_a_request_remote() {
        let mut extensions = Extensions::new();
        extensions.insert(RemoteTransport {
            peer_ip: Some("203.0.113.9".to_string()),
            origin: Some("https://bear.example".to_string()),
        });
        let context = remote_context(&extensions).unwrap();
        assert_eq!(context.peer_ip.as_deref(), Some("203.0.113.9"));
        assert_eq!(context.origin.as_deref(), Some("https://bear.example"));
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
        let script = headers(&[
            ("sec-fetch-mode", "cors"),
            ("sec-fetch-dest", "empty"),
            ("accept", "*/*"),
        ]);
        assert!(!is_navigation(&Method::GET, &script));
    }

    #[test]
    fn a_page_load_passed_on_by_a_service_worker_is_still_a_navigation() {
        let passed_on = headers(&[
            ("sec-fetch-mode", "same-origin"),
            ("accept", "text/html,application/xhtml+xml,*/*;q=0.8"),
        ]);
        assert!(is_navigation(&Method::GET, &passed_on));
        let document = headers(&[
            ("sec-fetch-mode", "same-origin"),
            ("sec-fetch-dest", "document"),
        ]);
        assert!(is_navigation(&Method::GET, &document));
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

//! The instance host's reverse proxy: public A2A and Teams traffic, forwarded
//! to the listeners on this machine.
//!
//! `/a2a/{agent}/{rest..}` goes to the hub's A2A listener at
//! `/agents/{agent}/{rest..}` and `/teams/{agent}` goes to that agent's Teams
//! listener. Both directions stream, so SSE responses and large request bodies
//! never sit in memory. A2A callers authenticate with the credentials the A2A
//! listener already checks; Teams requests are signed by Microsoft and checked
//! by the Teams listener, so this host adds no device gate and Teams is not
//! rate limited.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, Request, Response, StatusCode};
use tracing::{error, warn};

use super::engine_limit::PeerRateLimiter;

/// How long the listener may take to start answering.
const RESPONSE_START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long connecting to a local listener may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Headers that describe one hop and never travel through a proxy.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Request headers the proxy never forwards: the peer's `Host` (the upstream
/// gets its own) and forwarding headers the peer could forge.
const REQUEST_STRIP: [&str; 6] = [
    "host",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
];

/// Where the local listeners are right now.
pub(crate) struct InstanceTargets<'a> {
    /// The hub's A2A listener, when it is running.
    pub(crate) a2a_port: Option<u16>,
    /// Each agent's Teams listener port.
    pub(crate) teams_ports: &'a BTreeMap<String, u16>,
}

/// Serves requests for the instance host.
pub(crate) struct InstanceProxy {
    /// `None` when the HTTP client could not be built; every proxied request
    /// then answers 503.
    client: Option<reqwest::Client>,
    limiter: Mutex<PeerRateLimiter>,
}

impl InstanceProxy {
    /// A proxy with an HTTP client that never follows redirects, ignores proxy
    /// settings (the targets are on this machine) and has no total timeout.
    pub(crate) fn new() -> Self {
        let client = match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
        {
            Ok(client) => Some(client),
            Err(e) => {
                error!(error = %e, "couldn't build the HTTP client for the instance host; A2A and Teams over remote access are unavailable");
                None
            }
        };
        Self {
            client,
            limiter: Mutex::new(PeerRateLimiter::new(Instant::now())),
        }
    }

    /// Answer one request addressed to the instance host.
    pub(crate) async fn handle(
        &self,
        req: Request<Body>,
        peer_ip: &str,
        targets: InstanceTargets<'_>,
    ) -> Response<Body> {
        let path = req.uri().path().to_string();
        if path == "/a2a" || path.starts_with("/a2a/") {
            return self.handle_a2a(req, &path, peer_ip, targets.a2a_port).await;
        }
        if let Some(agent) = path.strip_prefix("/teams/") {
            return self.handle_teams(req, agent, targets.teams_ports).await;
        }
        text_response(
            StatusCode::NOT_FOUND,
            "Nothing is served at this address. This host answers /a2a/{agent} and /teams/{agent}.",
        )
    }

    async fn handle_a2a(
        &self,
        req: Request<Body>,
        path: &str,
        peer_ip: &str,
        a2a_port: Option<u16>,
    ) -> Response<Body> {
        if let Err(wait) = self.check_rate(peer_ip) {
            warn!(
                peer_ip,
                retry_after_secs = wait.as_secs_f64(),
                "rate limited an A2A request"
            );
            let mut response = text_response(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many requests from your address. Wait a moment and try again.",
            );
            let retry_after = wait.as_secs().saturating_add(1).to_string();
            if let Ok(value) = HeaderValue::from_str(&retry_after) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            return response;
        }

        let after_prefix = path.strip_prefix("/a2a/").unwrap_or_default();
        let (agent, tail) = match after_prefix.split_once('/') {
            Some((agent, tail)) => (agent, format!("/{tail}")),
            None => (after_prefix, String::new()),
        };
        if agent.is_empty() {
            return text_response(
                StatusCode::NOT_FOUND,
                "This A2A request didn't name an agent. Agents are reached at /a2a/{agent}.",
            );
        }
        let with_query = match req.uri().query() {
            Some(query) => format!("{tail}?{query}"),
            None => tail,
        };
        let listener_path = match a2a_listener_path(Some(agent), &with_query) {
            Ok(listener_path) => listener_path,
            Err(message) => return text_response(StatusCode::NOT_FOUND, message),
        };
        let Some(port) = a2a_port else {
            return text_response(
                StatusCode::NOT_FOUND,
                "The A2A endpoint isn't available on this Residuum instance right now: its A2A listener isn't running. Check Residuum's logs for why it couldn't start.",
            );
        };
        self.forward(req, format!("http://127.0.0.1:{port}{listener_path}"))
            .await
    }

    async fn handle_teams(
        &self,
        req: Request<Body>,
        agent: &str,
        teams_ports: &BTreeMap<String, u16>,
    ) -> Response<Body> {
        if agent.contains('/') || crate::config::paths::validate_agent_name(agent).is_err() {
            return text_response(
                StatusCode::NOT_FOUND,
                "No agent with that name is available on this Residuum instance.",
            );
        }
        if req.method() != Method::POST {
            let mut response = text_response(
                StatusCode::METHOD_NOT_ALLOWED,
                "Teams messages are delivered with POST.",
            );
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST"));
            return response;
        }
        let Some(port) = teams_ports.get(agent) else {
            warn!(
                agent,
                "a Teams message arrived for an agent with no Teams listener"
            );
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "This agent isn't set up to receive Teams messages right now.",
            );
        };
        self.forward(
            req,
            format!(
                "http://127.0.0.1:{port}{}",
                crate::interfaces::teams::MESSAGES_PATH
            ),
        )
        .await
    }

    fn check_rate(&self, peer_ip: &str) -> Result<(), Duration> {
        match self.limiter.lock() {
            Ok(mut limiter) => limiter.check(peer_ip, Instant::now()),
            Err(poisoned) => poisoned.into_inner().check(peer_ip, Instant::now()),
        }
    }

    /// Send `req` to `url` and stream the answer back.
    async fn forward(&self, req: Request<Body>, url: String) -> Response<Body> {
        let Some(client) = &self.client else {
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Remote access can't reach this Residuum instance's services right now. Check Residuum's logs.",
            );
        };
        let (parts, body) = req.into_parts();
        let carries_body = !matches!(
            parts.method,
            Method::GET | Method::HEAD | Method::DELETE | Method::OPTIONS
        ) || parts.headers.contains_key(header::TRANSFER_ENCODING)
            || parts
                .headers
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim() != "0");

        let mut upstream = client
            .request(parts.method.clone(), &url)
            .headers(forwardable_request_headers(&parts.headers));
        if carries_body {
            upstream = upstream.body(reqwest::Body::wrap_stream(body.into_data_stream()));
        }

        let response = match tokio::time::timeout(RESPONSE_START_TIMEOUT, upstream.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                warn!(error = %e, url = %url, "couldn't reach a local listener for a remote request");
                return text_response(
                    StatusCode::BAD_GATEWAY,
                    "Residuum couldn't reach that service on this machine. Try again in a moment.",
                );
            }
            Err(_) => {
                warn!(url = %url, timeout_secs = RESPONSE_START_TIMEOUT.as_secs(), "a local listener didn't start answering a remote request in time");
                return text_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Residuum's service took too long to answer. Try again in a moment.",
                );
            }
        };

        let status = response.status();
        let headers = hardened_response_headers(response.headers());
        let mut out = Response::new(Body::from_stream(response.bytes_stream()));
        *out.status_mut() = status;
        *out.headers_mut() = headers;
        out
    }
}

/// Names listed in the `Connection` header; they are hop-by-hop too.
fn connection_listed(headers: &HeaderMap) -> Vec<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect()
}

fn forwardable_request_headers(headers: &HeaderMap) -> HeaderMap {
    let listed = connection_listed(headers);
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let n = name.as_str();
        if HOP_BY_HOP.contains(&n) || REQUEST_STRIP.contains(&n) || listed.contains(name) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Upstream response headers minus hop-by-hop headers and cookies: A2A and
/// Teams are API surfaces, and a cookie set here would land on the instance
/// host's origin.
fn hardened_response_headers(headers: &HeaderMap) -> HeaderMap {
    let listed = connection_listed(headers);
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if HOP_BY_HOP.contains(&name.as_str())
            || name == header::SET_COOKIE
            || name.as_str() == "set-cookie2"
            || listed.contains(name)
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// A plain-text answer that no cache keeps.
pub(crate) fn text_response(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(Body::from(format!("{message}\n")));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The path on the hub's A2A listener for an A2A request on the instance host:
/// `/agents/{agent}` followed by the path the request carried (which already
/// includes any query string), or a plain-language reason the request can't be
/// dispatched.
///
/// The hub's A2A listener has no root-level agent, so a request that names no
/// agent has nowhere to go. The name is checked against the agent-name rules
/// because it becomes a path segment on the local listener.
fn a2a_listener_path(agent: Option<&str>, path: &str) -> Result<String, &'static str> {
    let Some(agent) = agent else {
        return Err("This A2A request didn't name an agent. Agents are reached at /a2a/{agent}.");
    };
    if crate::config::paths::validate_agent_name(agent).is_err() {
        return Err("No agent with that name is available on this Residuum instance.");
    }
    if path_escapes_agent(path) {
        return Err("This A2A request's path isn't valid.");
    }
    let separator = if path.starts_with('/') { "" } else { "/" };
    Ok(format!(
        "{}/{agent}{separator}{path}",
        crate::a2a::public_url::AGENTS_PATH_PREFIX
    ))
}

/// Whether a browser-supplied path could climb out of `/agents/{agent}` once
/// the HTTP client normalizes it: any `.` or `..` segment, in plain or
/// percent-encoded form (`%2e`, `%2E`, mixed), or any backslash. The relay
/// gates access per agent, so a path that reaches a different agent's routes
/// would bypass that gating (`/a2a/inst/scout/../vault/...` normalizes to
/// vault's routes). The query string is not part of the path.
fn path_escapes_agent(path: &str) -> bool {
    let path_only = path.split(['?', '#']).next().unwrap_or_default();
    let decoded = percent_decode(path_only);
    decoded.contains('\\')
        || decoded
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

/// Decode `%XX` escapes, leaving malformed ones as they are.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let escaped = if byte == b'%' {
            bytes
                .get(i + 1..i + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        } else {
            None
        };
        if let Some(decoded) = escaped {
            out.push(decoded);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_request_maps_onto_the_agents_prefix() {
        assert_eq!(
            a2a_listener_path(Some("scout"), "/.well-known/agent-card.json").as_deref(),
            Ok("/agents/scout/.well-known/agent-card.json")
        );
        assert_eq!(
            a2a_listener_path(Some("scout"), "/rest/message:send?x=1").as_deref(),
            Ok("/agents/scout/rest/message:send?x=1")
        );
        assert_eq!(
            a2a_listener_path(Some("scout"), "/").as_deref(),
            Ok("/agents/scout/")
        );
        assert_eq!(
            a2a_listener_path(Some("scout"), "").as_deref(),
            Ok("/agents/scout/")
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_agent_is_refused() {
        for bad in [
            "/../vault/.well-known/agent-card.json",
            "/./x",
            "/..",
            "/rest/../../vault/x",
            "/%2e%2e/vault/x",
            "/%2E%2E/vault/x",
            "/%2e./vault/x",
            "/.%2E/vault/x",
            "/%2e/x",
            "/..%2fvault/x",
            "/%2e%2e%2fvault/x",
            "/..\\vault/x",
            "/%5cvault",
            "/%5Cvault",
            "/a\\b",
            "..",
            "/x/..?q=1",
        ] {
            assert!(
                a2a_listener_path(Some("scout"), bad).is_err(),
                "'{bad}' must not reach another agent's routes"
            );
        }
    }

    #[test]
    fn dots_inside_names_and_the_query_string_are_not_traversal() {
        for fine in [
            "/.well-known/agent-card.json",
            "/rest/a..b",
            "/rest/message:send?next=../x",
            "/v1/tasks/1.2.3",
            "/%2e%2eabc/x",
        ] {
            assert!(
                a2a_listener_path(Some("scout"), fine).is_ok(),
                "'{fine}' is an ordinary path"
            );
        }
    }

    #[test]
    fn a_request_without_a_usable_agent_is_refused() {
        assert!(a2a_listener_path(None, "/").is_err());
        for bad in ["", "../hub", "Scout", "a/b", "-x", "x y"] {
            assert!(
                a2a_listener_path(Some(bad), "/").is_err(),
                "'{bad}' must not become a listener path"
            );
        }
    }
}

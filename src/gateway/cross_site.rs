//! Guard against browsers issuing requests to the gateway on behalf of other sites.
//!
//! The gateway has no authentication of its own, so any web page open in the
//! user's browser could otherwise POST to `localhost:<port>/api/shutdown` or
//! open `/ws` and drive the agent. Browsers label every request with
//! `Sec-Fetch-Site`, which the relay passes through unchanged, so it identifies
//! the initiator both locally and through the tunnel (where the `Host` header is
//! rewritten and an `Origin`-vs-`Host` comparison would reject the real UI).
//!
//! Non-browser clients (the CLI, webhook senders, the tunnel itself) send no
//! `Sec-Fetch-Site` and are unaffected. Plain `GET`/`HEAD`/`OPTIONS` requests are
//! let through because they have no side effects and a cross-site page cannot
//! read their responses without CORS headers, which the gateway never sends.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Reject state-changing requests and WebSocket upgrades initiated by another site.
pub(crate) async fn reject_cross_site_requests(req: Request, next: Next) -> Response {
    if is_cross_site_request(req.method(), req.headers()) {
        tracing::warn!(
            method = %req.method(),
            path = %req.uri().path(),
            sec_fetch_site = ?req.headers().get("sec-fetch-site"),
            origin = ?req.headers().get(header::ORIGIN),
            "rejected cross-site request to the gateway"
        );
        return (
            StatusCode::FORBIDDEN,
            "This request came from another website and was blocked. Open Residuum directly to make changes.",
        )
            .into_response();
    }
    next.run(req).await
}

fn is_cross_site_request(method: &Method, headers: &HeaderMap) -> bool {
    let is_websocket_upgrade = headers
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let is_side_effect_free = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if is_side_effect_free && !is_websocket_upgrade {
        return false;
    }

    match headers.get("sec-fetch-site") {
        // `none` is a user-initiated navigation (address bar, bookmark).
        Some(site) => {
            !(site.as_bytes().eq_ignore_ascii_case(b"same-origin")
                || site.as_bytes().eq_ignore_ascii_case(b"none"))
        }
        // Browsers without Fetch Metadata still mark sandboxed and opaque
        // initiators with a literal `null` origin.
        None => headers
            .get(header::ORIGIN)
            .is_some_and(|v| v.as_bytes() == b"null"),
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, Request};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn allows_same_origin_post() {
        let h = headers(&[("sec-fetch-site", "same-origin")]);
        assert!(!is_cross_site_request(&Method::POST, &h));
    }

    #[test]
    fn rejects_cross_site_and_same_site_post() {
        for site in ["cross-site", "same-site"] {
            let mut h = HeaderMap::new();
            h.insert("sec-fetch-site", HeaderValue::from_str(site).unwrap());
            assert!(
                is_cross_site_request(&Method::POST, &h),
                "{site} POST should be rejected"
            );
        }
    }

    #[test]
    fn allows_non_browser_clients_without_fetch_metadata() {
        assert!(!is_cross_site_request(&Method::POST, &HeaderMap::new()));
        assert!(!is_cross_site_request(&Method::DELETE, &HeaderMap::new()));
    }

    #[test]
    fn rejects_null_origin_without_fetch_metadata() {
        let h = headers(&[("origin", "null")]);
        assert!(is_cross_site_request(&Method::PUT, &h));
    }

    #[test]
    fn allows_cross_site_plain_get() {
        let h = headers(&[("sec-fetch-site", "cross-site")]);
        assert!(!is_cross_site_request(&Method::GET, &h));
    }

    #[test]
    fn rejects_cross_site_websocket_upgrade() {
        let h = headers(&[("sec-fetch-site", "cross-site"), ("upgrade", "websocket")]);
        assert!(is_cross_site_request(&Method::GET, &h));
    }

    #[test]
    fn allows_same_origin_websocket_upgrade() {
        let h = headers(&[("sec-fetch-site", "same-origin"), ("upgrade", "websocket")]);
        assert!(!is_cross_site_request(&Method::GET, &h));
    }

    fn app() -> Router {
        Router::new()
            .route("/api/shutdown", post(|| async { "ok" }))
            .fallback(get(|| async { "spa" }))
            .layer(axum::middleware::from_fn(reject_cross_site_requests))
    }

    #[tokio::test]
    async fn middleware_blocks_cross_site_post() {
        let req = Request::post("/api/shutdown")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn middleware_passes_same_origin_post() {
        let req = Request::post("/api/shutdown")
            .header("sec-fetch-site", "same-origin")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn middleware_covers_the_fallback() {
        let req = Request::post("/anything")
            .header("origin", "null")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

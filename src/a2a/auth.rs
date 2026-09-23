//! Axum auth middleware for the A2A listener.
//!
//! Every request is authenticated as either a caller-key holder or a sibling
//! instance attested by this process's own tunnel connection, per
//! `docs/systems-usage/a2a.md`. The resolved caller is injected as
//! [`CALLER_HEADER`] for the handler and executor to read; nothing
//! downstream of this layer should trust that header from anywhere else.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config::A2aVisibility;

use super::keys_runtime::SharedA2aKeys;

/// Header the auth layer injects with the resolved caller identity
/// (`key:<name>` or `sibling:<slug>`). Stripped from every incoming request
/// before it is ever inspected, so a caller cannot forge it.
pub const CALLER_HEADER: &str = "x-residuum-a2a-caller";

/// Header the tunnel forwarder sets to this process's per-process nonce when
/// forwarding a request that another of this user's instances sent as a
/// sibling.
const TUNNEL_HEADER: &str = "x-residuum-tunnel";

/// Header naming the calling sibling's slug. Only trusted when
/// [`TUNNEL_HEADER`] matches this process's own nonce.
const SIBLING_HEADER: &str = "x-residuum-sibling";

/// Path the relay's directory probes and local health checks use to test
/// whether a bearer token or sibling attestation is currently valid, without
/// needing a full A2A call.
pub const AUTH_CHECK_PATH: &str = "/_a2a/auth-check";

/// Who is calling the A2A listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// A caller key minted with `residuum a2a keys create`.
    Key(String),
    /// Another of this user's instances, attested by this process's own
    /// tunnel connection (never by anything a client can present directly).
    Sibling(String),
}

impl Caller {
    /// The value written to [`CALLER_HEADER`].
    #[must_use]
    pub fn header_value(&self) -> String {
        match self {
            Self::Key(name) => format!("key:{name}"),
            Self::Sibling(slug) => format!("sibling:{slug}"),
        }
    }
}

/// Supplies this process's current tunnel nonce, which authenticates a
/// sibling-forwarded request as genuinely coming from this instance's own
/// tunnel connection rather than a forged header from anywhere else.
///
/// A trait rather than a bare `Option<Arc<str>>` so the tunnel stream can
/// hand over a value that changes across reconnects without the auth layer
/// needing to be told about each rotation.
pub trait TunnelNonceSource: Send + Sync {
    /// The current nonce, or `None` if no tunnel is connected (sibling
    /// requests are never trusted while this is `None`).
    fn tunnel_nonce(&self) -> Option<Arc<str>>;
}

/// No tunnel is wired up: every sibling attestation is rejected. The
/// listener's default until the tunnel stream supplies a real nonce source.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTunnel;

impl TunnelNonceSource for NoTunnel {
    fn tunnel_nonce(&self) -> Option<Arc<str>> {
        None
    }
}

impl<F> TunnelNonceSource for F
where
    F: Fn() -> Option<Arc<str>> + Send + Sync,
{
    fn tunnel_nonce(&self) -> Option<Arc<str>> {
        self()
    }
}

/// Shared state for the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    /// Caller-key store, for verifying `Authorization: Bearer` tokens.
    pub keys: SharedA2aKeys,
    /// Source of this process's tunnel nonce, for sibling attestation.
    pub tunnel_nonce: Arc<dyn TunnelNonceSource>,
    /// Fixed for the listener's lifetime — a visibility change restarts it.
    pub visibility: A2aVisibility,
}

/// Whether `req` is the public Agent Card GET, which stays open to everyone
/// in public visibility.
fn is_card_get(req: &Request<Body>) -> bool {
    req.method() == Method::GET && req.uri().path() == a2a_server::WELL_KNOWN_AGENT_CARD_PATH
}

fn is_auth_check(req: &Request<Body>) -> bool {
    req.uri().path() == AUTH_CHECK_PATH
}

/// Resolve the caller from the (already-extracted) tunnel/sibling/bearer
/// headers, per the precedence in the module doc comment.
async fn resolve_caller(
    state: &AuthState,
    tunnel: Option<&str>,
    sibling: Option<&str>,
    authorization: Option<&str>,
) -> Option<Caller> {
    if let (Some(tunnel), Some(sibling)) = (tunnel, sibling)
        && let Some(nonce) = state.tunnel_nonce.tunnel_nonce()
        && crate::util::secrets_match(tunnel, &nonce)
    {
        return Some(Caller::Sibling(sibling.to_string()));
    }

    let token = authorization.and_then(|v| v.strip_prefix("Bearer "))?;
    let name = state.keys.verify(token).await?;
    Some(Caller::Key(name))
}

/// The auth middleware. Install with
/// `axum::middleware::from_fn_with_state(state, auth_middleware)`.
pub async fn auth_middleware(
    State(state): State<AuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let headers = req.headers_mut();
    // Never trust a client-supplied caller header, in or out.
    headers.remove(CALLER_HEADER);
    let tunnel = headers
        .get(TUNNEL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let sibling = headers
        .get(SIBLING_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // Stripped in every case, whether or not they led to a sibling caller.
    headers.remove(TUNNEL_HEADER);
    headers.remove(SIBLING_HEADER);

    let card_get = is_card_get(&req);
    let auth_check = is_auth_check(&req);
    let caller = resolve_caller(
        &state,
        tunnel.as_deref(),
        sibling.as_deref(),
        authorization.as_deref(),
    )
    .await;

    if auth_check {
        return if caller.is_some() {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        };
    }

    match (caller, state.visibility) {
        (Some(caller), _) => {
            if let Ok(value) = HeaderValue::from_str(&caller.header_value()) {
                req.headers_mut().insert(CALLER_HEADER, value);
            }
            next.run(req).await
        }
        (None, A2aVisibility::Public) if card_get => next.run(req).await,
        (None, A2aVisibility::Public) => {
            let mut resp = StatusCode::UNAUTHORIZED.into_response();
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            resp
        }
        (None, A2aVisibility::Private) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::keys_runtime::A2aKeys;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn echo_caller(req: Request<Body>) -> Response {
        let caller = req
            .headers()
            .get(CALLER_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("<none>")
            .to_string();
        let sibling_present = req.headers().contains_key(SIBLING_HEADER);
        let tunnel_present = req.headers().contains_key(TUNNEL_HEADER);
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(format!(
                "{caller}|sibling_hdr={sibling_present}|tunnel_hdr={tunnel_present}"
            )))
            .unwrap()
    }

    struct FixedNonce(&'static str);
    impl TunnelNonceSource for FixedNonce {
        fn tunnel_nonce(&self) -> Option<Arc<str>> {
            Some(Arc::from(self.0))
        }
    }

    fn app(state: AuthState) -> Router {
        Router::new()
            .route("/.well-known/agent-card.json", get(echo_caller))
            .route(AUTH_CHECK_PATH, get(echo_caller))
            .route("/", axum::routing::post(echo_caller))
            .layer(axum::middleware::from_fn_with_state(state, auth_middleware))
    }

    async fn state_with_key(
        dir: &std::path::Path,
        visibility: A2aVisibility,
    ) -> (AuthState, String) {
        let keys = A2aKeys::new(dir);
        let token = keys.create("laptop", None).await.unwrap();
        (
            AuthState {
                keys: Arc::new(keys),
                tunnel_nonce: Arc::new(NoTunnel),
                visibility,
            },
            token,
        )
    }

    fn get_req(uri: &str, bearer: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn valid_key_is_authenticated_and_header_injected() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path(), A2aVisibility::Public).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.starts_with("key:laptop|"));
    }

    #[tokio::test]
    async fn bad_key_is_401_in_public_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path(), A2aVisibility::Public).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, "Bearer rsdm_a2a_totallywrongtoken")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    #[tokio::test]
    async fn bad_key_is_404_in_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path(), A2aVisibility::Private).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn card_get_is_open_in_public_mode_without_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path(), A2aVisibility::Public).await;
        let resp = app(state)
            .oneshot(get_req("/.well-known/agent-card.json", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn card_get_is_404_in_private_mode_without_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path(), A2aVisibility::Private).await;
        let resp = app(state)
            .oneshot(get_req("/.well-known/agent-card.json", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn card_get_succeeds_in_private_mode_with_a_valid_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path(), A2aVisibility::Private).await;
        let resp = app(state)
            .oneshot(get_req("/.well-known/agent-card.json", Some(&token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn forged_sibling_header_without_matching_nonce_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let state = AuthState {
            keys: Arc::new(keys),
            tunnel_nonce: Arc::new(FixedNonce("real-nonce")),
            visibility: A2aVisibility::Public,
        };
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(TUNNEL_HEADER, "wrong-nonce")
            .header(SIBLING_HEADER, "alpha")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn sibling_header_with_no_tunnel_wired_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let state = AuthState {
            keys: Arc::new(keys),
            tunnel_nonce: Arc::new(NoTunnel),
            visibility: A2aVisibility::Public,
        };
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(TUNNEL_HEADER, "anything")
            .header(SIBLING_HEADER, "alpha")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn matching_tunnel_nonce_authenticates_as_sibling_and_strips_headers() {
        let dir = tempfile::tempdir().unwrap();
        let keys = A2aKeys::new(dir.path());
        let state = AuthState {
            keys: Arc::new(keys),
            tunnel_nonce: Arc::new(FixedNonce("real-nonce")),
            visibility: A2aVisibility::Public,
        };
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(TUNNEL_HEADER, "real-nonce")
            .header(SIBLING_HEADER, "alpha")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(body, "sibling:alpha|sibling_hdr=false|tunnel_hdr=false");
    }

    #[tokio::test]
    async fn client_supplied_caller_header_is_stripped_and_never_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path(), A2aVisibility::Public).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(CALLER_HEADER, "key:someone-else")
            .body(Body::empty())
            .unwrap();
        let resp = app(state).oneshot(req).await.unwrap();
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            body.starts_with("key:laptop|"),
            "the real verified caller must win over a forged header: {body}"
        );
    }

    #[tokio::test]
    async fn auth_check_is_204_when_authenticated_and_404_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path(), A2aVisibility::Public).await;

        let ok = app(state.clone())
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&token)))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);

        let denied = app(state)
            .oneshot(get_req(AUTH_CHECK_PATH, None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auth_check_behaves_the_same_in_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path(), A2aVisibility::Private).await;

        let ok = app(state.clone())
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&token)))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);

        let denied = app(state)
            .oneshot(get_req(AUTH_CHECK_PATH, None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }
}

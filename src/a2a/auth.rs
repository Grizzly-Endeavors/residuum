//! Request authentication for the A2A listener.
//!
//! Every request is authenticated as either a caller-key holder or a joined
//! sibling instance presenting the key it was issued, per
//! `docs/systems-usage/a2a.md`. The check runs per request, before the
//! request is dispatched to an agent, with the target agent's visibility.
//! The resolved caller is injected as [`CALLER_HEADER`] for the handler and
//! executor to read; nothing downstream of this layer should trust that
//! header from anywhere else.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::config::A2aVisibility;

use super::keys_runtime::SharedA2aKeys;
use crate::remote_access::siblings::SiblingKeyVerifier;

/// Header the auth layer injects with the resolved caller identity
/// (`key:<name>` or `sibling:<slug>`). Stripped from every incoming request
/// before it is ever inspected, so a caller cannot forge it.
pub const CALLER_HEADER: &str = "x-residuum-a2a-caller";

/// Path the relay's directory probes and local health checks use to test
/// whether a bearer token is currently valid, without
/// needing a full A2A call.
pub const AUTH_CHECK_PATH: &str = "/_a2a/auth-check";

/// Who is calling the A2A listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// A caller key minted with `residuum a2a keys create`.
    Key(String),
    /// Another of this user's instances: it presented a key issued to it when
    /// it joined this one.
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

/// Hub-level state for request authentication: the one caller-key store and
/// the joined siblings' keys, shared by every agent.
#[derive(Clone)]
pub struct AuthState {
    /// Caller-key store, for verifying `Authorization: Bearer` tokens.
    pub keys: SharedA2aKeys,
    /// Keys issued to joined siblings, each tagged with the sibling's slug.
    pub sibling_keys: Arc<dyn SiblingKeyVerifier>,
}

/// Whether `req` is the public Agent Card GET, which stays open to everyone
/// in public visibility.
fn is_card_get(req: &Request<Body>) -> bool {
    req.method() == Method::GET && req.uri().path() == a2a_server::WELL_KNOWN_AGENT_CARD_PATH
}

fn is_auth_check(req: &Request<Body>) -> bool {
    req.uri().path() == AUTH_CHECK_PATH
}

/// Resolve the caller from the `Authorization` bearer token: a caller key, or
/// the key a joined sibling was issued.
async fn resolve_caller(state: &AuthState, authorization: Option<&str>) -> Option<Caller> {
    let token = authorization.and_then(|v| v.strip_prefix("Bearer "))?;
    if let Some(name) = state.keys.verify(token).await {
        return Some(Caller::Key(name));
    }
    state.sibling_keys.verify(token).map(Caller::Sibling)
}

/// The result of [`authorize`].
pub enum Admission {
    /// The caller may proceed. The request has its credentials replaced by
    /// the verified [`CALLER_HEADER`].
    Admitted(Request<Body>),
    /// The caller is refused, or the request was fully answered here (the
    /// auth-check probe). Send this response instead of dispatching.
    Answered(Response),
}

/// Authenticate `req` against an agent with the given `visibility`.
///
/// A caller key or a joined sibling's key admits the caller to any agent. With
/// neither, a public agent serves only its Agent Card (everything else is
/// `401`) and a private agent answers `404` to every route. The
/// [`AUTH_CHECK_PATH`] probe is answered here: `204` when the caller is
/// authenticated, `404` otherwise.
pub async fn authorize(
    state: &AuthState,
    visibility: A2aVisibility,
    mut req: Request<Body>,
) -> Admission {
    let headers = req.headers_mut();
    // Never trust a client-supplied caller header, in or out.
    headers.remove(CALLER_HEADER);
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let card_get = is_card_get(&req);
    let auth_check = is_auth_check(&req);
    let caller = resolve_caller(state, authorization.as_deref()).await;

    if auth_check {
        return Admission::Answered(if caller.is_some() {
            StatusCode::NO_CONTENT.into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        });
    }

    match (caller, visibility) {
        (Some(caller), _) => {
            // The credential has done its job; keep the raw key out of the
            // SDK's service params, which the handler and executor see.
            req.headers_mut().remove(header::AUTHORIZATION);
            match HeaderValue::from_str(&caller.header_value()) {
                Ok(value) => {
                    req.headers_mut().insert(CALLER_HEADER, value);
                    Admission::Admitted(req)
                }
                Err(e) => {
                    tracing::error!(error = %e, "a2a caller identity is not a valid header value");
                    Admission::Answered(StatusCode::INTERNAL_SERVER_ERROR.into_response())
                }
            }
        }
        (None, A2aVisibility::Public) if card_get => Admission::Admitted(req),
        (None, A2aVisibility::Public) => {
            let mut resp = StatusCode::UNAUTHORIZED.into_response();
            resp.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            Admission::Answered(resp)
        }
        (None, A2aVisibility::Private) => {
            Admission::Answered(StatusCode::NOT_FOUND.into_response())
        }
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
        let authorization_present = req.headers().contains_key(header::AUTHORIZATION);
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(format!(
                "{caller}|authorization_hdr={authorization_present}"
            )))
            .unwrap()
    }

    /// The test stand-in for the hub listener: authorize, then hand the
    /// admitted request to a router that echoes what the handler would see.
    struct TestApp {
        state: AuthState,
        visibility: A2aVisibility,
    }

    impl TestApp {
        async fn oneshot(self, req: Request<Body>) -> Result<Response, std::convert::Infallible> {
            let inner = Router::new()
                .route("/.well-known/agent-card.json", get(echo_caller))
                .route("/", axum::routing::post(echo_caller));
            match authorize(&self.state, self.visibility, req).await {
                Admission::Admitted(req) => inner.oneshot(req).await,
                Admission::Answered(resp) => Ok(resp),
            }
        }
    }

    fn app(state: AuthState, visibility: A2aVisibility) -> TestApp {
        TestApp { state, visibility }
    }

    async fn state_with_key(dir: &std::path::Path) -> (AuthState, String) {
        let keys = A2aKeys::new(dir);
        let token = keys.create("laptop", None).await.unwrap();
        (
            AuthState {
                keys: Arc::new(keys),
                sibling_keys: Arc::new(crate::remote_access::siblings::NoSiblings),
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
        let (state, token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Public;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(state, visibility).oneshot(req).await.unwrap();
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
        let (state, _token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Public;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, "Bearer rsdm_a2a_totallywrongtoken")
            .body(Body::empty())
            .unwrap();
        let resp = app(state, visibility).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    #[tokio::test]
    async fn bad_key_is_404_in_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Private;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = app(state, visibility).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn card_get_is_open_in_public_mode_without_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Public;
        let resp = app(state, visibility)
            .oneshot(get_req("/.well-known/agent-card.json", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn card_get_is_404_in_private_mode_without_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Private;
        let resp = app(state, visibility)
            .oneshot(get_req("/.well-known/agent-card.json", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn card_get_succeeds_in_private_mode_with_a_valid_key() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Private;
        let resp = app(state, visibility)
            .oneshot(get_req("/.well-known/agent-card.json", Some(&token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn sibling_and_tunnel_headers_are_not_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _token) = state_with_key(dir.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("x-residuum-tunnel", "anything")
            .header("x-residuum-sibling", "alpha")
            .body(Body::empty())
            .unwrap();
        let resp = app(state, A2aVisibility::Public)
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A hub whose only credential is a joined sibling's key.
    async fn state_with_sibling_key(dir: &std::path::Path) -> (AuthState, String) {
        let siblings = crate::remote_access::siblings::SiblingKeys::open(dir);
        let inbound = crate::remote_access::siblings::keys::issue_key();
        siblings
            .upsert(crate::remote_access::siblings::keys::NewSibling {
                slug: "desktop".to_string(),
                display_name: "Desk".to_string(),
                account_uri: "https://acme.test/acct/desktop".to_string(),
                outbound_key: crate::remote_access::siblings::keys::issue_key(),
                inbound_key: inbound.clone(),
            })
            .await
            .unwrap();
        (
            AuthState {
                keys: Arc::new(A2aKeys::new(dir.join("a2a"))),
                sibling_keys: Arc::new(siblings),
            },
            inbound,
        )
    }

    #[tokio::test]
    async fn a_sibling_key_reaches_a_private_agent_as_that_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let (state, key) = state_with_sibling_key(dir.path()).await;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(state, A2aVisibility::Private)
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            body, "sibling:desktop|authorization_hdr=false",
            "the key is spent on authenticating and not passed on"
        );
    }

    #[tokio::test]
    async fn without_a_sibling_key_a_private_agent_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let (state, key) = state_with_sibling_key(dir.path()).await;
        let none = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let without_key = app(state.clone(), A2aVisibility::Private)
            .oneshot(none)
            .await
            .unwrap();
        assert_eq!(without_key.status(), StatusCode::NOT_FOUND);

        // A key that no sibling holds, and a real key with a character changed.
        let mut wrong = key.clone();
        wrong.replace_range(
            wrong.len() - 1..,
            if key.ends_with('a') { "b" } else { "a" },
        );
        for token in [crate::remote_access::siblings::keys::issue_key(), wrong] {
            let req = Request::builder()
                .method("POST")
                .uri("/")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let resp = app(state.clone(), A2aVisibility::Private)
                .oneshot(req)
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn a_removed_sibling_key_stops_working_and_auth_check_follows_it() {
        let dir = tempfile::tempdir().unwrap();
        let siblings = Arc::new(crate::remote_access::siblings::SiblingKeys::open(
            dir.path(),
        ));
        let inbound = crate::remote_access::siblings::keys::issue_key();
        siblings
            .upsert(crate::remote_access::siblings::keys::NewSibling {
                slug: "desktop".to_string(),
                display_name: "Desk".to_string(),
                account_uri: "https://acme.test/acct/desktop".to_string(),
                outbound_key: crate::remote_access::siblings::keys::issue_key(),
                inbound_key: inbound.clone(),
            })
            .await
            .unwrap();
        let state = AuthState {
            keys: Arc::new(A2aKeys::new(dir.path().join("a2a"))),
            sibling_keys: Arc::clone(&siblings) as Arc<dyn SiblingKeyVerifier>,
        };
        let ok = app(state.clone(), A2aVisibility::Private)
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&inbound)))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);
        siblings.remove("desktop").await.unwrap();
        let gone = app(state, A2aVisibility::Private)
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&inbound)))
            .await
            .unwrap();
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn client_supplied_caller_header_is_stripped_and_never_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Public;
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(CALLER_HEADER, "key:someone-else")
            .body(Body::empty())
            .unwrap();
        let resp = app(state, visibility).oneshot(req).await.unwrap();
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
        assert!(
            body.ends_with("|authorization_hdr=false"),
            "the verified key must not be forwarded to the handler: {body}"
        );
    }

    #[tokio::test]
    async fn auth_check_is_204_when_authenticated_and_404_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Public;

        let ok = app(state.clone(), visibility)
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&token)))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);

        let denied = app(state, visibility)
            .oneshot(get_req(AUTH_CHECK_PATH, None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auth_check_behaves_the_same_in_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let (state, token) = state_with_key(dir.path()).await;
        let visibility = A2aVisibility::Private;

        let ok = app(state.clone(), visibility)
            .oneshot(get_req(AUTH_CHECK_PATH, Some(&token)))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);

        let denied = app(state, visibility)
            .oneshot(get_req(AUTH_CHECK_PATH, None))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }
}

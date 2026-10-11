//! The instance that asks to join: it reaches the other instance at the host
//! name it derived itself, asks for a nonce, sends a request signed with its
//! certificate account key, and polls for the answer.
//!
//! Calls to a sibling go to `https://{slug}.{user}.{base}` only. The host
//! comes from the stored user and the configured base domain, never from the
//! relay, certificates must be publicly trusted (or chain to the extra root
//! the configuration names), and redirects are never followed, so nothing
//! this instance sends can be steered to another address.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::protocol::{
    JoinPayload, NONCE_PATH, NonceReply, PATH_PREFIX, PollReply, REQUEST_PATH, SubmitReply,
    confirmation_code, is_account_uri,
};
use crate::remote_access::jws::{AccountSigner, sign_jws, thumbprint};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// An answer from a sibling.
pub(crate) struct ChannelReply {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

/// A sibling couldn't be reached or answered nonsense.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ChannelError(pub(crate) String);

/// How requests reach a sibling's host.
#[async_trait]
pub(crate) trait SiblingChannel: Send + Sync {
    /// `GET {path}` at `host`.
    async fn get(&self, host: &str, path: &str) -> Result<ChannelReply, ChannelError>;
    /// `POST {path}` at `host` with a JSON body.
    async fn post(
        &self,
        host: &str,
        path: &str,
        body: &Value,
    ) -> Result<ChannelReply, ChannelError>;
}

/// The real channel: HTTPS to the sibling's instance host.
pub(crate) struct HttpChannel {
    client: reqwest::Client,
    scheme: &'static str,
}

impl HttpChannel {
    /// HTTPS only to hosts with a publicly trusted certificate, plus the
    /// roots in `extra_root` (a PEM file) when the configuration names one.
    ///
    /// # Errors
    /// Returns an error if the root file can't be read or the client can't be built.
    pub(crate) fn https(extra_root: Option<&Path>) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(CALL_TIMEOUT)
            .https_only(true);
        if let Some(path) = extra_root {
            let pem = std::fs::read(path).map_err(|e| {
                anyhow::anyhow!(
                    "failed to read the extra root certificate {}: {e}",
                    path.display()
                )
            })?;
            for certificate in reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
                anyhow::anyhow!(
                    "failed to parse the extra root certificate {}: {e}",
                    path.display()
                )
            })? {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build the sibling HTTP client: {e}"))?;
        Ok(Self {
            client,
            scheme: "https",
        })
    }

    /// Plain HTTP to a loopback test server.
    #[cfg(test)]
    pub(crate) fn plain_http_for_tests() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(CALL_TIMEOUT)
                .build()
                .expect("a test client"),
            scheme: "http",
        }
    }

    async fn finish(request: reqwest::RequestBuilder) -> Result<ChannelReply, ChannelError> {
        let response = request
            .send()
            .await
            .map_err(|e| ChannelError(format!("the request failed: {e}")))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ChannelError(format!("the answer couldn't be read: {e}")))?;
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok(ChannelReply { status, body })
    }
}

#[async_trait]
impl SiblingChannel for HttpChannel {
    async fn get(&self, host: &str, path: &str) -> Result<ChannelReply, ChannelError> {
        Self::finish(self.client.get(format!("{}://{host}{path}", self.scheme))).await
    }

    async fn post(
        &self,
        host: &str,
        path: &str,
        body: &Value,
    ) -> Result<ChannelReply, ChannelError> {
        Self::finish(
            self.client
                .post(format!("{}://{host}{path}", self.scheme))
                .json(body),
        )
        .await
    }
}

/// Why a join couldn't start or continue, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum JoinFailure {
    /// The other instance couldn't be reached.
    #[error(
        "Couldn't reach \"{slug}\". It has to be online and set up for remote access. ({detail})"
    )]
    Unreachable { slug: String, detail: String },
    /// The other instance said no, with its reason.
    #[error("\"{slug}\" refused the join: {reason}")]
    Refused { slug: String, reason: String },
    /// The other instance's answer wasn't what a Residuum instance sends.
    #[error(
        "\"{slug}\" answered in a way Residuum doesn't understand. Make sure it runs a current Residuum."
    )]
    Malformed { slug: String },
    /// The person on the other instance said no.
    #[error("\"{slug}\" denied the join.")]
    Denied { slug: String },
    /// The request expired before anyone answered.
    #[error(
        "Nobody approved the join on \"{slug}\" in time. Start it again when someone is there."
    )]
    Expired { slug: String },
}

/// What a started join knows.
pub(crate) struct JoinSubmitted {
    /// The address only this instance polls with.
    pub(crate) join_id: String,
    /// The six digits both people compare.
    pub(crate) code: String,
    /// The key this instance issued for the other to present.
    pub(crate) issued_key: String,
    /// The other instance's host name.
    pub(crate) host: String,
    /// The other instance's account, as it said at the start.
    pub(crate) host_account_uri: String,
    pub(crate) host_account_jwk: Value,
}

/// Who is joining whom.
pub(crate) struct JoinRequest<'a> {
    pub(crate) account: &'a dyn AccountSigner,
    pub(crate) user: &'a str,
    pub(crate) own_slug: &'a str,
    pub(crate) display_name: &'a str,
    pub(crate) base_domain: &'a str,
    pub(crate) target_slug: &'a str,
}

fn parse<T: DeserializeOwned>(reply: ChannelReply, slug: &str) -> Result<T, JoinFailure> {
    if !(200..300).contains(&reply.status) {
        let reason = reply
            .body
            .get("error")
            .and_then(Value::as_str)
            .map_or_else(|| format!("status {}", reply.status), str::to_string);
        return Err(JoinFailure::Refused {
            slug: slug.to_string(),
            reason,
        });
    }
    serde_json::from_value(reply.body).map_err(|_unparsed| JoinFailure::Malformed {
        slug: slug.to_string(),
    })
}

/// Fetch a nonce from the target and send the signed request.
///
/// # Errors
/// Returns why the other instance couldn't be joined.
pub(crate) async fn submit_join(
    channel: &dyn SiblingChannel,
    request: &JoinRequest<'_>,
    key: String,
) -> Result<JoinSubmitted, JoinFailure> {
    let slug = request.target_slug;
    let host = format!("{slug}.{}.{}", request.user, request.base_domain);
    let unreachable = |e: ChannelError| JoinFailure::Unreachable {
        slug: slug.to_string(),
        detail: e.to_string(),
    };

    let nonce_reply: NonceReply = parse(
        channel.get(&host, NONCE_PATH).await.map_err(unreachable)?,
        slug,
    )?;
    let malformed = || JoinFailure::Malformed {
        slug: slug.to_string(),
    };
    if nonce_reply.slug != slug
        || !is_account_uri(&nonce_reply.account_uri)
        || thumbprint(&nonce_reply.account_jwk).is_err()
    {
        return Err(malformed());
    }

    let url = format!("https://{host}{REQUEST_PATH}");
    let header = json!({ "alg": "ES256", "url": url, "jwk": request.account.jwk() });
    let payload = JoinPayload {
        nonce: nonce_reply.nonce.clone(),
        slug: request.own_slug.to_string(),
        display_name: request.display_name.to_string(),
        account_uri: request.account.uri().to_string(),
        key: key.clone(),
    };
    let payload = serde_json::to_value(&payload).map_err(|_unserializable| malformed())?;
    let signed =
        sign_jws(request.account, &header, &payload).map_err(|e| JoinFailure::Unreachable {
            slug: slug.to_string(),
            detail: format!("couldn't sign the request: {e:#}"),
        })?;
    let submitted: SubmitReply = parse(
        channel
            .post(&host, REQUEST_PATH, &signed)
            .await
            .map_err(unreachable)?,
        slug,
    )?;
    let code = confirmation_code(
        &nonce_reply.account_jwk,
        &request.account.jwk(),
        &nonce_reply.nonce,
    )
    .map_err(|_bad_key| malformed())?;
    Ok(JoinSubmitted {
        join_id: submitted.join_id,
        code,
        issued_key: key,
        host,
        host_account_uri: nonce_reply.account_uri,
        host_account_jwk: nonce_reply.account_jwk,
    })
}

/// What a poll found.
pub(crate) enum PollOutcome {
    Waiting,
    Approved {
        /// The other instance's key for this one to present.
        key: String,
    },
}

/// Ask the other instance where the request stands.
///
/// # Errors
/// Returns why the join can't continue: it was denied, it expired, or the
/// other instance answered wrongly.
pub(crate) async fn poll_join(
    channel: &dyn SiblingChannel,
    target_slug: &str,
    submitted: &JoinSubmitted,
) -> Result<PollOutcome, JoinFailure> {
    let path = format!("{PATH_PREFIX}/{}", submitted.join_id);
    let reply =
        channel
            .get(&submitted.host, &path)
            .await
            .map_err(|e| JoinFailure::Unreachable {
                slug: target_slug.to_string(),
                detail: e.to_string(),
            })?;
    if reply.status == 404 {
        return Err(JoinFailure::Expired {
            slug: target_slug.to_string(),
        });
    }
    match parse::<PollReply>(reply, target_slug)? {
        PollReply::Pending => Ok(PollOutcome::Waiting),
        PollReply::Denied => Err(JoinFailure::Denied {
            slug: target_slug.to_string(),
        }),
        PollReply::Approved {
            key,
            account_uri,
            account_jwk,
        } => {
            // The same instance that was asked, not another one answering the poll.
            let same_account = account_uri == submitted.host_account_uri
                && thumbprint(&account_jwk).ok() == thumbprint(&submitted.host_account_jwk).ok();
            if same_account && crate::remote_access::siblings::keys::is_key_shaped(&key) {
                Ok(PollOutcome::Approved { key })
            } else {
                Err(JoinFailure::Malformed {
                    slug: target_slug.to_string(),
                })
            }
        }
    }
}

/// A channel that reaches a sibling's router in-process, as the engine would
/// after TLS, for tests.
#[cfg(test)]
pub(crate) mod in_process {
    use async_trait::async_trait;
    use axum::Router;
    use serde_json::Value;

    use super::{ChannelError, ChannelReply, SiblingChannel};

    /// Reaches a router in-process, as the engine would after TLS.
    pub(crate) struct InProcess {
        router: Router,
        peer_ip: String,
    }

    impl InProcess {
        pub(crate) fn new(router: Router, peer_ip: &str) -> Self {
            Self {
                router,
                peer_ip: peer_ip.to_string(),
            }
        }

        async fn call(&self, request: axum::http::Request<axum::body::Body>) -> ChannelReply {
            use tower::ServiceExt as _;
            let mut request = request;
            request
                .extensions_mut()
                .insert(crate::pairing::remote::RemoteTransport {
                    peer_ip: Some(self.peer_ip.clone()),
                    origin: None,
                });
            let response = self.router.clone().oneshot(request).await.unwrap();
            let status = response.status().as_u16();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            ChannelReply {
                status,
                body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            }
        }
    }

    #[async_trait]
    impl SiblingChannel for InProcess {
        async fn get(&self, _host: &str, path: &str) -> Result<ChannelReply, ChannelError> {
            Ok(self
                .call(
                    axum::http::Request::get(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await)
        }

        async fn post(
            &self,
            _host: &str,
            path: &str,
            body: &Value,
        ) -> Result<ChannelReply, ChannelError> {
            Ok(self
                .call(
                    axum::http::Request::post(path)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(body.to_string()))
                        .unwrap(),
                )
                .await)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::Redirect;
    use axum::routing::get;

    use super::in_process::InProcess;
    use super::*;
    use crate::remote_access::jws::test_key::TestAccount;
    use crate::remote_access::siblings::host::{HostContext, JoinHost};
    use crate::remote_access::siblings::keys::issue_key;
    use crate::remote_access::siblings::routes;

    fn host_with_account() -> (Arc<JoinHost>, Arc<TestAccount>) {
        let account = Arc::new(TestAccount::new("https://acme.test/acct/host"));
        let host = Arc::new(JoinHost::default());
        host.set_context(Some(Arc::new(HostContext {
            user: "bear".into(),
            slug: "laptop".into(),
            base_domain: "relay.test".into(),
            account: Arc::clone(&account) as Arc<dyn AccountSigner>,
        })));
        (host, account)
    }

    fn request<'a>(account: &'a TestAccount, target: &'a str) -> JoinRequest<'a> {
        JoinRequest {
            account,
            user: "bear",
            own_slug: "desktop",
            display_name: "Desk",
            base_domain: "relay.test",
            target_slug: target,
        }
    }

    #[tokio::test]
    async fn a_join_runs_from_nonce_to_approval_with_matching_codes() {
        let (host, host_account) = host_with_account();
        let channel = InProcess::new(routes::router(Arc::clone(&host)), "203.0.113.9");
        let joiner = TestAccount::new("https://acme.test/acct/joiner");

        let submitted = submit_join(&channel, &request(&joiner, "laptop"), issue_key())
            .await
            .unwrap();
        assert_eq!(submitted.host, "laptop.bear.relay.test");
        assert!(matches!(
            poll_join(&channel, "laptop", &submitted).await.unwrap(),
            PollOutcome::Waiting
        ));

        // The code the host's person sees is the one the joiner shows.
        let [shown] = host
            .pending(chrono::Utc::now())
            .try_into()
            .unwrap_or_else(|v: Vec<_>| panic!("one pending request, got {}", v.len()));
        assert_eq!(shown.code, submitted.code);
        assert_eq!(shown.slug, "desktop");

        let host_key = issue_key();
        host.approve(
            &submitted.join_id,
            PollReply::Approved {
                key: host_key.clone(),
                account_uri: host_account.uri().to_string(),
                account_jwk: host_account.jwk(),
            },
            chrono::Utc::now(),
        );
        let PollOutcome::Approved { key } =
            poll_join(&channel, "laptop", &submitted).await.unwrap()
        else {
            panic!("expected an approval");
        };
        assert_eq!(key, host_key);
    }

    #[tokio::test]
    async fn a_denial_and_an_unknown_join_are_reported() {
        let (host, _) = host_with_account();
        let channel = InProcess::new(routes::router(Arc::clone(&host)), "203.0.113.9");
        let joiner = TestAccount::new("https://acme.test/acct/joiner");
        let submitted = submit_join(&channel, &request(&joiner, "laptop"), issue_key())
            .await
            .unwrap();
        let approval_id = host.pending(chrono::Utc::now()).remove(0).approval_id;
        assert!(host.deny(&approval_id, chrono::Utc::now()));
        assert!(matches!(
            poll_join(&channel, "laptop", &submitted).await,
            Err(JoinFailure::Denied { .. })
        ));

        let mut forgotten = submitted;
        forgotten.join_id = "0".repeat(32);
        assert!(matches!(
            poll_join(&channel, "laptop", &forgotten).await,
            Err(JoinFailure::Expired { .. })
        ));
    }

    #[tokio::test]
    async fn an_answer_from_a_different_account_than_the_one_asked_is_refused() {
        let (host, _) = host_with_account();
        let channel = InProcess::new(routes::router(Arc::clone(&host)), "203.0.113.9");
        let joiner = TestAccount::new("https://acme.test/acct/joiner");
        let submitted = submit_join(&channel, &request(&joiner, "laptop"), issue_key())
            .await
            .unwrap();
        let impostor = TestAccount::new("https://acme.test/acct/impostor");
        host.approve(
            &submitted.join_id,
            PollReply::Approved {
                key: issue_key(),
                account_uri: impostor.uri().to_string(),
                account_jwk: impostor.jwk(),
            },
            chrono::Utc::now(),
        );
        assert!(matches!(
            poll_join(&channel, "laptop", &submitted).await,
            Err(JoinFailure::Malformed { .. })
        ));
    }

    #[tokio::test]
    async fn the_target_must_say_it_is_the_instance_that_was_asked_for() {
        let (host, _) = host_with_account();
        let channel = InProcess::new(routes::router(host), "203.0.113.9");
        let joiner = TestAccount::new("https://acme.test/acct/joiner");
        // The host calls itself "laptop"; the joiner asked for "server".
        let outcome = submit_join(&channel, &request(&joiner, "server"), issue_key()).await;
        assert!(matches!(outcome, Err(JoinFailure::Malformed { .. })));
    }

    #[tokio::test]
    async fn the_join_endpoints_allow_ten_requests_a_minute_per_address() {
        let (host, _) = host_with_account();
        let router = routes::router(host);
        let noisy = InProcess::new(router.clone(), "203.0.113.50");
        for _ in 0..10 {
            let reply = noisy.get("h", NONCE_PATH).await.unwrap();
            assert_eq!(reply.status, 200);
        }
        assert_eq!(noisy.get("h", NONCE_PATH).await.unwrap().status, 429);
        let other = InProcess::new(router, "203.0.113.51");
        assert_eq!(other.get("h", NONCE_PATH).await.unwrap().status, 200);
    }

    #[tokio::test]
    async fn a_host_that_is_not_ready_answers_503() {
        let channel = InProcess::new(routes::router(Arc::new(JoinHost::default())), "203.0.113.9");
        assert_eq!(channel.get("h", NONCE_PATH).await.unwrap().status, 503);
    }

    #[tokio::test]
    async fn the_http_channel_never_follows_a_redirect() {
        let app = Router::new()
            .route("/hop", get(|| async { Redirect::temporary("/landing") }))
            .route("/landing", get(|| async { (StatusCode::OK, "secret") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let channel = HttpChannel::plain_http_for_tests();
        let reply = channel.get(&addr.to_string(), "/hop").await.unwrap();
        assert_eq!(reply.status, 307, "the redirect is reported, not followed");
    }
}

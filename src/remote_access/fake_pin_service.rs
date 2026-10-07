//! A stand-in for the pin service, for tests: it checks the same things the
//! real one does about a request's signature, grant and recovery code, keeps
//! the pins in memory, and publishes the CAA records the real one would to a
//! Pebble challenge test server.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde_json::{Value, json};

use super::pebble_support::PebbleHarness;

/// The grant the fake relay issues and this service accepts.
pub(crate) const TEST_GRANT: &str = "test.grant.jws";

#[derive(Clone)]
struct StoredPin {
    account_uri: String,
    slug: String,
    /// The account's public key, which a `kid` signature is checked against.
    jwk: Option<Value>,
}

#[derive(Default)]
struct ServiceState {
    pins: Vec<StoredPin>,
    recovery_hash: Option<String>,
    seen_jtis: Vec<String>,
    requests: Vec<String>,
}

struct Shared {
    state: Mutex<ServiceState>,
    url: String,
    dns: Option<Arc<PebbleHarness>>,
}

/// A running fake pin service.
pub(crate) struct FakePinService {
    shared: Arc<Shared>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakePinService {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakePinService {
    /// Start on a free loopback port. With `dns`, every change publishes the
    /// CAA records for `{user}.{base}` and `{user}.workbench.{base}` there.
    pub(crate) async fn start(dns: Option<Arc<PebbleHarness>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake pin service");
        let addr: SocketAddr = listener.local_addr().expect("pin service address");
        let base = format!("http://{addr}");
        let shared = Arc::new(Shared {
            state: Mutex::new(ServiceState::default()),
            url: base.clone(),
            dns,
        });
        let app = Router::new()
            .route("/v1/pins/{user}", get(list))
            .route("/v1/enroll", post(enroll))
            .route("/v1/reset", post(reset))
            .route("/v1/pins/add", post(add))
            .route("/v1/pins/remove", post(remove))
            .with_state(Arc::clone(&shared));
        let task = crate::util::spawn_in_span(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::debug!(error = %e, "fake pin service ended");
            }
        });
        Self { shared, base, task }
    }

    /// The service's base URL.
    pub(crate) fn url(&self) -> &str {
        &self.base
    }

    /// Pin `account_uri` as if another instance had enrolled.
    pub(crate) fn preload(&self, account_uri: &str, slug: &str) {
        lock(&self.shared).pins.push(StoredPin {
            account_uri: account_uri.to_string(),
            slug: slug.to_string(),
            jwk: None,
        });
    }

    /// Pin `account_uri` with its public key, so it can sign `add` and `remove`.
    pub(crate) fn preload_with_key(&self, account_uri: &str, slug: &str, jwk: Value) {
        lock(&self.shared).pins.push(StoredPin {
            account_uri: account_uri.to_string(),
            slug: slug.to_string(),
            jwk: Some(jwk),
        });
    }

    /// The pinned accounts, as `(account_uri, slug)`.
    pub(crate) fn pins(&self) -> Vec<(String, String)> {
        lock(&self.shared)
            .pins
            .iter()
            .map(|pin| (pin.account_uri.clone(), pin.slug.clone()))
            .collect()
    }

    /// The `op` of every accepted mutation, in order.
    pub(crate) fn operations(&self) -> Vec<String> {
        lock(&self.shared).requests.clone()
    }
}

fn lock(shared: &Shared) -> MutexGuard<'_, ServiceState> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

type Reply = (StatusCode, Json<Value>);

fn fail(status: StatusCode, message: &str) -> Reply {
    (status, Json(json!({ "error": message })))
}

fn pin_list(shared: &Shared) -> Value {
    let pins: Vec<Value> = lock(shared)
        .pins
        .iter()
        .map(|pin| json!({ "account_uri": pin.account_uri, "slug": pin.slug }))
        .collect();
    json!({ "pins": pins })
}

async fn list(State(shared): State<Arc<Shared>>, Path(_user): Path<String>) -> Reply {
    (StatusCode::OK, Json(pin_list(&shared)))
}

/// Check the JWS the way the pin service does: ES256 over the protected
/// header and payload, the header's `url`, and a fresh `jti`.
fn verify(shared: &Shared, path: &str, body: &Value) -> Result<(Value, Value), Reply> {
    let part = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "malformed"))
    };
    let (protected_b64, payload_b64, signature_b64) =
        (part("protected")?, part("payload")?, part("signature")?);
    let decode = |text: &str| {
        URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_unparsed| fail(StatusCode::BAD_REQUEST, "malformed"))
    };
    let protected: Value = serde_json::from_slice(&decode(protected_b64)?)
        .map_err(|_unparsed| fail(StatusCode::BAD_REQUEST, "malformed"))?;
    let payload: Value = serde_json::from_slice(&decode(payload_b64)?)
        .map_err(|_unparsed| fail(StatusCode::BAD_REQUEST, "malformed"))?;
    if protected.get("alg").and_then(Value::as_str) != Some("ES256") {
        return Err(fail(StatusCode::BAD_REQUEST, "alg"));
    }
    if protected.get("url").and_then(Value::as_str) != Some(&format!("{}{path}", shared.url)) {
        return Err(fail(StatusCode::UNAUTHORIZED, "url mismatch"));
    }
    let jwk = match (
        protected.get("jwk"),
        protected.get("kid").and_then(Value::as_str),
    ) {
        (Some(jwk), None) => jwk.clone(),
        (None, Some(kid)) => lock(shared)
            .pins
            .iter()
            .find(|pin| pin.account_uri == kid)
            .ok_or_else(|| fail(StatusCode::FORBIDDEN, "signer is not pinned"))?
            .jwk
            .clone()
            .ok_or_else(|| fail(StatusCode::FORBIDDEN, "signer has no known key"))?,
        _ => return Err(fail(StatusCode::BAD_REQUEST, "exactly one of jwk and kid")),
    };
    let jwk = &jwk;
    let coordinate = |name: &str| {
        jwk.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "jwk"))
            .and_then(decode)
    };
    let mut public = vec![4_u8];
    public.extend(coordinate("x")?);
    public.extend(coordinate("y")?);
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
        .verify(
            format!("{protected_b64}.{payload_b64}").as_bytes(),
            &decode(signature_b64)?,
        )
        .map_err(|_unparsed| fail(StatusCode::UNAUTHORIZED, "bad signature"))?;
    let iat = payload.get("iat").and_then(Value::as_i64).unwrap_or(0);
    if (chrono::Utc::now().timestamp() - iat).abs() > 300 {
        return Err(fail(StatusCode::UNAUTHORIZED, "stale iat"));
    }
    let jti = payload
        .get("jti")
        .and_then(Value::as_str)
        .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "jti"))?
        .to_string();
    let mut state = lock(shared);
    if state.seen_jtis.contains(&jti) {
        return Err(fail(StatusCode::UNAUTHORIZED, "replayed jti"));
    }
    state.seen_jtis.push(jti);
    Ok((protected, payload))
}

fn field<'a>(payload: &'a Value, name: &str) -> &'a str {
    payload
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

async fn publish_caa(shared: &Shared, user: &str) {
    let Some(dns) = &shared.dns else { return };
    let accounts: Vec<String> = lock(shared)
        .pins
        .iter()
        .map(|pin| pin.account_uri.clone())
        .collect();
    let values: Vec<String> = accounts
        .iter()
        .map(|uri| {
            format!("pebble.letsencrypt.org; accounturi={uri}; validationmethods=tls-alpn-01")
        })
        .collect();
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    for host in [
        format!("{user}.relay.test"),
        format!("{user}.workbench.relay.test"),
    ] {
        if let Err(e) = dns.set_caa(&host, &refs).await {
            tracing::error!(error = %format!("{e:#}"), host = %host, "fake pin service couldn't publish CAA");
        }
    }
}

async fn enroll(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Reply {
    let (protected, payload) = match verify(&shared, "/v1/enroll", &body) {
        Ok(ok) => ok,
        Err(reply) => return reply,
    };
    if field(&payload, "op") != "enroll" || field(&payload, "grant") != TEST_GRANT {
        return fail(StatusCode::UNAUTHORIZED, "bad grant");
    }
    if !lock(&shared).pins.is_empty() {
        return fail(StatusCode::CONFLICT, "user already has pins");
    }
    if field(&payload, "recovery_code_hash").len() != 64 {
        return fail(StatusCode::BAD_REQUEST, "recovery_code_hash");
    }
    {
        let mut state = lock(&shared);
        state.pins.push(StoredPin {
            account_uri: field(&payload, "account_uri").to_string(),
            slug: field(&payload, "slug").to_string(),
            jwk: protected.get("jwk").cloned(),
        });
        state.recovery_hash = Some(field(&payload, "recovery_code_hash").to_string());
        state.requests.push("enroll".to_string());
    }
    publish_caa(&shared, field(&payload, "user")).await;
    (StatusCode::OK, Json(pin_list(&shared)))
}

async fn reset(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Reply {
    let (protected, payload) = match verify(&shared, "/v1/reset", &body) {
        Ok(ok) => ok,
        Err(reply) => return reply,
    };
    if field(&payload, "op") != "reset" || field(&payload, "grant") != TEST_GRANT {
        return fail(StatusCode::UNAUTHORIZED, "bad grant");
    }
    let presented = hex::encode(
        ring::digest::digest(
            &ring::digest::SHA256,
            field(&payload, "recovery_code").as_bytes(),
        )
        .as_ref(),
    );
    {
        let mut state = lock(&shared);
        if state.recovery_hash.as_deref() != Some(presented.as_str()) {
            return fail(StatusCode::FORBIDDEN, "wrong recovery code");
        }
        state.pins = vec![StoredPin {
            account_uri: field(&payload, "account_uri").to_string(),
            slug: field(&payload, "slug").to_string(),
            jwk: protected.get("jwk").cloned(),
        }];
        state.recovery_hash = Some(field(&payload, "recovery_code_hash").to_string());
        state.requests.push("reset".to_string());
    }
    publish_caa(&shared, field(&payload, "user")).await;
    (StatusCode::OK, Json(pin_list(&shared)))
}

async fn add(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Reply {
    let (_, payload) = match verify(&shared, "/v1/pins/add", &body) {
        Ok(ok) => ok,
        Err(reply) => return reply,
    };
    if field(&payload, "op") != "add" {
        return fail(StatusCode::BAD_REQUEST, "op");
    }
    {
        let mut state = lock(&shared);
        let uri = field(&payload, "account_uri");
        if !state.pins.iter().any(|pin| pin.account_uri == uri) {
            state.pins.push(StoredPin {
                account_uri: uri.to_string(),
                slug: field(&payload, "slug").to_string(),
                jwk: payload.get("jwk").cloned(),
            });
        }
        state.requests.push("add".to_string());
    }
    publish_caa(&shared, field(&payload, "user")).await;
    (StatusCode::OK, Json(pin_list(&shared)))
}

async fn remove(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Reply {
    let (_, payload) = match verify(&shared, "/v1/pins/remove", &body) {
        Ok(ok) => ok,
        Err(reply) => return reply,
    };
    if field(&payload, "op") != "remove" {
        return fail(StatusCode::BAD_REQUEST, "op");
    }
    {
        let mut state = lock(&shared);
        let uri = field(&payload, "account_uri");
        if state.pins.len() < 2 || !state.pins.iter().any(|pin| pin.account_uri == uri) {
            return fail(StatusCode::CONFLICT, "cannot remove that pin");
        }
        state.pins.retain(|pin| pin.account_uri != uri);
        state.requests.push("remove".to_string());
    }
    publish_caa(&shared, field(&payload, "user")).await;
    (StatusCode::OK, Json(pin_list(&shared)))
}

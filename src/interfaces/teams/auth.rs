//! Validation of the bearer token the Bot Connector sends with every activity.
//!
//! The Teams endpoint is reachable from the public internet, so nothing in an
//! activity is trusted until its RS256 JWT verifies against Microsoft's
//! published signing keys. The checks follow the Bot Framework connector
//! authentication spec: issuer, audience (our app ID), validity window,
//! a signing key endorsed for the `msteams` channel, and a `serviceurl`
//! claim matching the activity (so a replayed token cannot redirect replies).

use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

const OPENID_METADATA_URL: &str =
    "https://login.botframework.com/v1/.well-known/openidconfiguration";
const EXPECTED_ISSUER: &str = "https://api.botframework.com";
const TEAMS_CHANNEL: &str = "msteams";
/// Tolerated clock difference between us and Microsoft's token issuer.
const CLOCK_SKEW_SECS: i64 = 300;
/// Signing keys rotate rarely; refresh daily regardless.
const KEY_REFRESH_INTERVAL: Duration = Duration::from_hours(24);
/// Floor between refetches triggered by an unknown key ID, so junk tokens
/// cannot make us hammer Microsoft's metadata endpoint.
const MIN_REFETCH_INTERVAL: Duration = Duration::from_mins(5);

/// Why an inbound request was rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum AuthError {
    #[error("missing or non-bearer authorization header")]
    MissingBearer,
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("unsupported signing algorithm {0}")]
    UnsupportedAlgorithm(String),
    #[error("no signing key with id {0}")]
    UnknownKey(String),
    #[error("signing key is not endorsed for the msteams channel")]
    NotEndorsed,
    #[error("signature does not verify")]
    BadSignature,
    #[error("unexpected issuer {0}")]
    WrongIssuer(String),
    #[error("token audience does not match this bot's app id")]
    WrongAudience,
    #[error("token expired or not yet valid")]
    OutsideValidity,
    #[error("serviceurl claim does not match the activity")]
    ServiceUrlMismatch,
    #[error("could not fetch signing keys: {0}")]
    KeyFetch(String),
}

/// One RSA signing key from Microsoft's JWKS document.
#[derive(Debug, Clone)]
pub(super) struct SigningKey {
    kid: String,
    modulus: Vec<u8>,
    exponent: Vec<u8>,
    endorsements: Vec<String>,
}

#[derive(Deserialize)]
struct OpenIdMetadata {
    jwks_uri: String,
}

#[derive(Deserialize)]
struct JwksDocument {
    keys: Vec<JwkEntry>,
}

#[derive(Deserialize)]
struct JwkEntry {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    #[serde(default)]
    endorsements: Vec<String>,
}

#[derive(Deserialize)]
struct TokenHeader {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
struct TokenClaims {
    iss: String,
    aud: Audience,
    exp: i64,
    nbf: Option<i64>,
    #[serde(alias = "serviceUrl")]
    serviceurl: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, app_id: &str) -> bool {
        match self {
            Self::One(aud) => aud == app_id,
            Self::Many(auds) => auds.iter().any(|a| a == app_id),
        }
    }
}

/// Verify a connector token against a known key set. Pure; `now` is unix seconds.
pub(super) fn verify_token(
    token: &str,
    keys: &[SigningKey],
    app_id: &str,
    service_url: &str,
    now: i64,
) -> Result<(), AuthError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(claims_b64), Some(sig_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(AuthError::Malformed(
            "expected three dot-separated parts".to_string(),
        ));
    };

    let header: TokenHeader = decode_json(header_b64, "header")?;
    if header.alg != "RS256" {
        return Err(AuthError::UnsupportedAlgorithm(header.alg));
    }
    let kid = header
        .kid
        .ok_or_else(|| AuthError::Malformed("header has no kid".to_string()))?;
    let key = keys
        .iter()
        .find(|k| k.kid == kid)
        .ok_or_else(|| AuthError::UnknownKey(kid.clone()))?;
    if !key.endorsements.iter().any(|e| e == TEAMS_CHANNEL) {
        return Err(AuthError::NotEndorsed);
    }

    let signature = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|e| AuthError::Malformed(format!("signature is not base64url: {e}")))?;
    let signed = token
        .get(..header_b64.len() + 1 + claims_b64.len())
        .ok_or_else(|| AuthError::Malformed("token is shorter than its parts".to_string()))?;
    ring::signature::RsaPublicKeyComponents {
        n: &key.modulus,
        e: &key.exponent,
    }
    .verify(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        signed.as_bytes(),
        &signature,
    )
    .map_err(|ring::error::Unspecified| AuthError::BadSignature)?;

    let claims: TokenClaims = decode_json(claims_b64, "claims")?;
    if claims.iss != EXPECTED_ISSUER {
        return Err(AuthError::WrongIssuer(claims.iss));
    }
    if !claims.aud.contains(app_id) {
        return Err(AuthError::WrongAudience);
    }
    let not_before = claims.nbf.unwrap_or(i64::MIN);
    if now > claims.exp + CLOCK_SKEW_SECS || now + CLOCK_SKEW_SECS < not_before {
        return Err(AuthError::OutsideValidity);
    }
    let claimed_url = claims.serviceurl.ok_or(AuthError::ServiceUrlMismatch)?;
    if claimed_url.trim_end_matches('/') != service_url.trim_end_matches('/') {
        return Err(AuthError::ServiceUrlMismatch);
    }
    Ok(())
}

fn decode_json<T: serde::de::DeserializeOwned>(
    segment: &str,
    what: &'static str,
) -> Result<T, AuthError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|e| AuthError::Malformed(format!("{what} is not base64url: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| AuthError::Malformed(format!("{what} is not valid JSON: {e}")))
}

struct KeyCache {
    keys: Vec<SigningKey>,
    fetched_at: Option<Instant>,
}

/// Validates inbound tokens, keeping Microsoft's signing keys cached.
pub(super) struct TokenValidator {
    http: reqwest::Client,
    app_id: String,
    cache: tokio::sync::Mutex<KeyCache>,
}

impl TokenValidator {
    pub(super) fn new(http: reqwest::Client, app_id: String) -> Self {
        Self {
            http,
            app_id,
            cache: tokio::sync::Mutex::new(KeyCache {
                keys: Vec::new(),
                fetched_at: None,
            }),
        }
    }

    /// Validate the `Authorization` header of a request carrying an activity
    /// whose `serviceUrl` is `service_url`.
    pub(super) async fn validate(
        &self,
        authorization: Option<&str>,
        service_url: &str,
    ) -> Result<(), AuthError> {
        let token = authorization
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or(AuthError::MissingBearer)?;
        let now = chrono::Utc::now().timestamp();

        let mut cache = self.cache.lock().await;
        let stale = cache
            .fetched_at
            .is_none_or(|at| at.elapsed() >= KEY_REFRESH_INTERVAL);
        if stale {
            self.refresh(&mut cache).await?;
        }
        match verify_token(token, &cache.keys, &self.app_id, service_url, now) {
            Err(AuthError::UnknownKey(kid))
                if cache
                    .fetched_at
                    .is_none_or(|at| at.elapsed() >= MIN_REFETCH_INTERVAL) =>
            {
                tracing::info!(kid = %kid, "teams token signed by an unknown key, refreshing signing keys");
                self.refresh(&mut cache).await?;
                verify_token(token, &cache.keys, &self.app_id, service_url, now)
            }
            other => other,
        }
    }

    async fn refresh(&self, cache: &mut KeyCache) -> Result<(), AuthError> {
        let fetch_error = |e: reqwest::Error| AuthError::KeyFetch(e.to_string());
        let metadata: OpenIdMetadata = self
            .http
            .get(OPENID_METADATA_URL)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(fetch_error)?
            .json()
            .await
            .map_err(fetch_error)?;
        let jwks: JwksDocument = self
            .http
            .get(&metadata.jwks_uri)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(fetch_error)?
            .json()
            .await
            .map_err(fetch_error)?;

        cache.keys = jwks.keys.into_iter().filter_map(signing_key).collect();
        cache.fetched_at = Some(Instant::now());
        tracing::debug!(keys = cache.keys.len(), "teams signing keys refreshed");
        Ok(())
    }
}

fn signing_key(entry: JwkEntry) -> Option<SigningKey> {
    if entry.kty != "RSA" {
        return None;
    }
    let modulus = URL_SAFE_NO_PAD.decode(entry.n?).ok()?;
    let exponent = URL_SAFE_NO_PAD.decode(entry.e?).ok()?;
    Some(SigningKey {
        kid: entry.kid,
        modulus,
        exponent,
        endorsements: entry.endorsements,
    })
}

/// Signing fixtures shared by the auth and endpoint tests.
#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    fn test_keypair() -> ring::signature::RsaKeyPair {
        ring::signature::RsaKeyPair::from_der(include_bytes!("testdata/test_signing_key.rsa.der"))
            .unwrap()
    }

    /// The public half of the test key, published under `kid = "key-1"`.
    pub(in crate::interfaces::teams) fn test_key(endorsements: &[&str]) -> SigningKey {
        let components: ring::signature::RsaPublicKeyComponents<Vec<u8>> =
            test_keypair().public().into();
        SigningKey {
            kid: "key-1".to_string(),
            modulus: components.n,
            exponent: components.e,
            endorsements: endorsements.iter().map(ToString::to_string).collect(),
        }
    }

    /// Sign a JWT with the test key.
    pub(in crate::interfaces::teams) fn sign(
        header: &serde_json::Value,
        claims: &serde_json::Value,
    ) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let keypair = test_keypair();
        let mut sig = vec![0; keypair.public().modulus_len()];
        keypair
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                signed.as_bytes(),
                &mut sig,
            )
            .unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(sig))
    }

    /// A currently valid connector token for `app_id` and `service_url`.
    pub(in crate::interfaces::teams) fn valid_token(app_id: &str, service_url: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        sign(
            &serde_json::json!({ "alg": "RS256", "kid": "key-1" }),
            &serde_json::json!({
                "iss": EXPECTED_ISSUER,
                "aud": app_id,
                "exp": now + 3600,
                "nbf": now - 60,
                "serviceurl": service_url,
            }),
        )
    }

    impl TokenValidator {
        /// A validator that already holds the test key, so it never fetches.
        pub(in crate::interfaces::teams) fn with_test_key(app_id: &str) -> Self {
            Self {
                http: reqwest::Client::new(),
                app_id: app_id.to_string(),
                cache: tokio::sync::Mutex::new(KeyCache {
                    keys: vec![test_key(&["msteams"])],
                    fetched_at: Some(Instant::now()),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{sign, test_key};
    use super::*;

    const APP_ID: &str = "11111111-2222-3333-4444-555555555555";
    const SERVICE_URL: &str = "https://smba.trafficmanager.net/amer/";
    const NOW: i64 = 1_790_000_000;

    fn header() -> serde_json::Value {
        serde_json::json!({ "alg": "RS256", "kid": "key-1", "typ": "JWT" })
    }

    fn claims() -> serde_json::Value {
        serde_json::json!({
            "iss": EXPECTED_ISSUER,
            "aud": APP_ID,
            "exp": NOW + 3600,
            "nbf": NOW - 60,
            "serviceurl": SERVICE_URL,
        })
    }

    fn with_claim(key: &str, value: serde_json::Value) -> serde_json::Value {
        let mut claims = claims();
        claims
            .as_object_mut()
            .unwrap()
            .insert(key.to_string(), value);
        claims
    }

    fn verify(token: &str) -> Result<(), AuthError> {
        verify_token(token, &[test_key(&["msteams"])], APP_ID, SERVICE_URL, NOW)
    }

    #[test]
    fn accepts_a_valid_teams_token() {
        assert_eq!(verify(&sign(&header(), &claims())), Ok(()));
    }

    #[test]
    fn service_url_compare_ignores_trailing_slash() {
        let token = sign(&header(), &claims());
        let result = verify_token(
            &token,
            &[test_key(&["msteams"])],
            APP_ID,
            "https://smba.trafficmanager.net/amer",
            NOW,
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn rejects_tampered_claims() {
        let token = sign(&header(), &claims());
        let (header_b64, rest) = token.split_once('.').unwrap();
        let (_, signature_b64) = rest.split_once('.').unwrap();
        let forged = with_claim("aud", serde_json::json!("attacker-app"));
        let forged_b64 = URL_SAFE_NO_PAD.encode(forged.to_string());
        let tampered = format!("{header_b64}.{forged_b64}.{signature_b64}");
        assert_eq!(verify(&tampered), Err(AuthError::BadSignature));
    }

    #[test]
    fn rejects_wrong_audience_issuer_and_expiry() {
        let wrong_aud = with_claim("aud", serde_json::json!("some-other-bot"));
        assert_eq!(
            verify(&sign(&header(), &wrong_aud)),
            Err(AuthError::WrongAudience)
        );

        let wrong_iss = with_claim("iss", serde_json::json!("https://evil.example"));
        assert_eq!(
            verify(&sign(&header(), &wrong_iss)),
            Err(AuthError::WrongIssuer("https://evil.example".to_string()))
        );

        let expired = with_claim("exp", serde_json::json!(NOW - CLOCK_SKEW_SECS - 1));
        assert_eq!(
            verify(&sign(&header(), &expired)),
            Err(AuthError::OutsideValidity)
        );

        let early = with_claim("nbf", serde_json::json!(NOW + CLOCK_SKEW_SECS + 1));
        assert_eq!(
            verify(&sign(&header(), &early)),
            Err(AuthError::OutsideValidity)
        );
    }

    #[test]
    fn audience_may_be_a_list() {
        let listed = with_claim("aud", serde_json::json!(["other", APP_ID]));
        assert_eq!(verify(&sign(&header(), &listed)), Ok(()));
    }

    #[test]
    fn rejects_mismatched_or_missing_service_url() {
        let other_url = with_claim("serviceurl", serde_json::json!("https://attacker.example/"));
        assert_eq!(
            verify(&sign(&header(), &other_url)),
            Err(AuthError::ServiceUrlMismatch)
        );

        let mut missing = claims();
        missing.as_object_mut().unwrap().remove("serviceurl");
        assert_eq!(
            verify(&sign(&header(), &missing)),
            Err(AuthError::ServiceUrlMismatch)
        );
    }

    #[test]
    fn rejects_keys_not_endorsed_for_teams() {
        let token = sign(&header(), &claims());
        let result = verify_token(
            &token,
            &[test_key(&["skype", "webchat"])],
            APP_ID,
            SERVICE_URL,
            NOW,
        );
        assert_eq!(result, Err(AuthError::NotEndorsed));
    }

    #[test]
    fn rejects_other_algorithms_and_unknown_keys() {
        let none_alg = serde_json::json!({ "alg": "none", "kid": "key-1" });
        assert_eq!(
            verify(&sign(&none_alg, &claims())),
            Err(AuthError::UnsupportedAlgorithm("none".to_string()))
        );

        let other_kid = serde_json::json!({ "alg": "RS256", "kid": "key-2" });
        assert_eq!(
            verify(&sign(&other_kid, &claims())),
            Err(AuthError::UnknownKey("key-2".to_string()))
        );
    }

    #[test]
    fn rejects_malformed_tokens() {
        assert!(matches!(verify("not-a-jwt"), Err(AuthError::Malformed(_))));
        assert!(matches!(verify("a.b.c.d"), Err(AuthError::Malformed(_))));
    }
}

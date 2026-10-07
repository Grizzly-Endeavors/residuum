//! Client for the pin service: which ACME accounts may obtain certificates
//! for this user's host names.
//!
//! Mutations are flattened JWS objects signed with the instance's ACME
//! account key (ES256); the exact format is in the relay repository's
//! `pins/README.md`. The service's base URL comes from local configuration
//! only, never from anything the relay sends, and redirects are never
//! followed, so a request is signed for and sent to exactly one URL.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::{Value, json};

use super::jws::{AccountSigner, sign_jws};

/// Characters of a recovery code.
pub(crate) const RECOVERY_CODE_LEN: usize = 20;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// One pinned account, as the service lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Pin {
    pub(crate) account_uri: String,
    pub(crate) slug: String,
}

#[derive(Deserialize)]
struct PinList {
    #[serde(default)]
    pins: Vec<Pin>,
}

/// Why a pin service call didn't succeed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PinError {
    /// The service answered with an error status.
    #[error("the pin service refused the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    /// The service couldn't be reached, or answered nonsense.
    #[error("couldn't reach the pin service: {0}")]
    Unavailable(String),
}

/// What an enrollment or reset presents.
pub(crate) struct Enrollment<'a> {
    pub(crate) user: &'a str,
    pub(crate) slug: &'a str,
    pub(crate) grant: &'a str,
    /// The new recovery code; only its hash is sent.
    pub(crate) recovery_code: &'a str,
}

/// The pin service.
pub(crate) struct PinClient {
    http: reqwest::Client,
    base: String,
}

impl PinClient {
    /// A client for the service at `base_url`.
    ///
    /// # Errors
    /// Returns an error if the HTTP client can't be built.
    pub(crate) fn new(base_url: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build the pin service client: {e}"))?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// `GET /v1/pins/{user}`: the pinned accounts. Empty for an unknown user.
    pub(crate) async fn list(&self, user: &str) -> Result<Vec<Pin>, PinError> {
        let url = format!("{}/v1/pins/{user}", self.base);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| PinError::Unavailable(e.to_string()))?;
        parse_pins(response).await
    }

    /// `POST /v1/enroll`: pin `account` as the user's first account.
    pub(crate) async fn enroll(
        &self,
        account: &dyn AccountSigner,
        enrollment: &Enrollment<'_>,
    ) -> Result<Vec<Pin>, PinError> {
        let payload = json!({
            "op": "enroll",
            "user": enrollment.user,
            "slug": enrollment.slug,
            "account_uri": account.uri(),
            "recovery_code_hash": recovery_code_hash(enrollment.recovery_code),
            "grant": enrollment.grant,
        });
        self.post_signed(account, KeyRef::Jwk, "/v1/enroll", payload)
            .await
    }

    /// `POST /v1/reset`: replace every pin with `account`, proving ownership
    /// with the old recovery code, and store the hash of a new one.
    pub(crate) async fn reset(
        &self,
        account: &dyn AccountSigner,
        enrollment: &Enrollment<'_>,
        old_recovery_code: &str,
    ) -> Result<Vec<Pin>, PinError> {
        let payload = json!({
            "op": "reset",
            "user": enrollment.user,
            "slug": enrollment.slug,
            "account_uri": account.uri(),
            "recovery_code": old_recovery_code,
            "recovery_code_hash": recovery_code_hash(enrollment.recovery_code),
            "grant": enrollment.grant,
        });
        self.post_signed(account, KeyRef::Jwk, "/v1/reset", payload)
            .await
    }

    /// `POST /v1/pins/add`: pin `new_account` for `slug`, authorized by
    /// `signer`, which must already be pinned.
    pub(crate) async fn add(
        &self,
        signer: &dyn AccountSigner,
        user: &str,
        new_account: &NewPin<'_>,
    ) -> Result<Vec<Pin>, PinError> {
        let payload = json!({
            "op": "add",
            "user": user,
            "slug": new_account.slug,
            "account_uri": new_account.account_uri,
            "jwk": new_account.jwk,
        });
        self.post_signed(signer, KeyRef::Kid, "/v1/pins/add", payload)
            .await
    }

    /// `POST /v1/pins/remove`: stop allowing `account_uri` to get certificates,
    /// authorized by `signer`, which must be pinned.
    pub(crate) async fn remove(
        &self,
        signer: &dyn AccountSigner,
        user: &str,
        account_uri: &str,
    ) -> Result<Vec<Pin>, PinError> {
        let payload = json!({
            "op": "remove",
            "user": user,
            "account_uri": account_uri,
        });
        self.post_signed(signer, KeyRef::Kid, "/v1/pins/remove", payload)
            .await
    }

    async fn post_signed(
        &self,
        account: &dyn AccountSigner,
        key_ref: KeyRef,
        path: &str,
        mut payload: Value,
    ) -> Result<Vec<Pin>, PinError> {
        let url = format!("{}{path}", self.base);
        if let Some(object) = payload.as_object_mut() {
            object.insert("iat".into(), json!(chrono::Utc::now().timestamp()));
            object.insert(
                "jti".into(),
                json!(random_token().map_err(|e| PinError::Unavailable(e.to_string()))?),
            );
        }
        let header = match key_ref {
            KeyRef::Jwk => json!({ "alg": "ES256", "url": url, "jwk": account.jwk() }),
            KeyRef::Kid => json!({ "alg": "ES256", "url": url, "kid": account.uri() }),
        };
        let body = sign_jws(account, &header, &payload)
            .map_err(|e| PinError::Unavailable(format!("couldn't sign the request: {e:#}")))?;
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PinError::Unavailable(e.to_string()))?;
        parse_pins(response).await
    }
}

/// How a signed request names the key that signed it.
#[derive(Clone, Copy)]
enum KeyRef {
    /// The public key travels in the request (enroll and reset: no account is pinned yet).
    Jwk,
    /// The signer's pinned account URL (add and remove).
    Kid,
}

/// An account to pin next to the signer's own.
pub(crate) struct NewPin<'a> {
    pub(crate) slug: &'a str,
    pub(crate) account_uri: &'a str,
    pub(crate) jwk: &'a Value,
}

async fn parse_pins(response: reqwest::Response) -> Result<Vec<Pin>, PinError> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| PinError::Unavailable(format!("couldn't read the answer: {e}")))?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        return Err(PinError::Rejected {
            status: status.as_u16(),
            message,
        });
    }
    serde_json::from_str::<PinList>(&text)
        .map(|list| list.pins)
        .map_err(|e| PinError::Unavailable(format!("the answer wasn't a pin list: {e}")))
}

fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_unspecified| anyhow::anyhow!("the operating system's random source failed"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// A new recovery code: [`RECOVERY_CODE_LEN`] characters of `A-Z2-7`, about
/// 100 bits.
///
/// # Errors
/// Returns an error if the operating system's random source fails.
pub(crate) fn generate_recovery_code() -> anyhow::Result<String> {
    let rng = SystemRandom::new();
    let mut code = String::with_capacity(RECOVERY_CODE_LEN);
    // Rejection sampling keeps every character equally likely.
    while code.len() < RECOVERY_CODE_LEN {
        let mut bytes = [0_u8; 32];
        rng.fill(&mut bytes).map_err(|_unspecified| {
            anyhow::anyhow!("the operating system's random source failed")
        })?;
        for byte in bytes {
            if code.len() == RECOVERY_CODE_LEN {
                break;
            }
            if byte < 224
                && let Some(&ch) = BASE32_ALPHABET.get(usize::from(byte % 32))
            {
                code.push(char::from(ch));
            }
        }
    }
    Ok(code)
}

/// Whether `code` has the shape of a recovery code.
pub(crate) fn is_recovery_code(code: &str) -> bool {
    code.len() == RECOVERY_CODE_LEN && code.bytes().all(|b| BASE32_ALPHABET.contains(&b))
}

/// Lowercase hex SHA-256 of the code's ASCII, as the pin service stores it.
pub(crate) fn recovery_code_hash(code: &str) -> String {
    hex::encode(digest(&SHA256, code.as_bytes()).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_codes_have_the_documented_shape() {
        let code = generate_recovery_code().unwrap();
        assert!(is_recovery_code(&code), "{code}");
        assert_ne!(code, generate_recovery_code().unwrap());
        assert!(!is_recovery_code("abcdefghijklmnopqrst"));
        assert!(!is_recovery_code("ABCDEFGHIJKLMNOPQRS"));
        assert!(!is_recovery_code("ABCDEFGHIJKLMNOPQRS1"));
    }

    #[test]
    fn the_recovery_hash_is_lowercase_hex_sha256() {
        assert_eq!(
            recovery_code_hash("ABC"),
            "b5d4045c3f466fa91fe2cc6abe79232a1a57cdf104f7a26e716e0a1e2789df78"
        );
    }
}

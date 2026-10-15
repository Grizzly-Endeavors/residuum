//! Flattened JWS signed and checked with ES256 account keys: what the pin
//! service and sibling joins exchange, and the key thumbprint the join's
//! confirmation code is computed from.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde_json::{Value, json};

/// The ES256 key behind an ACME account: it signs requests to the pin service
/// and to sibling instances. The account URI is its identifier at the pin
/// service.
pub(crate) trait AccountSigner: Send + Sync {
    /// The account URL.
    fn uri(&self) -> &str;
    /// The public key as a JWK (`kty`, `crv`, `x`, `y`).
    fn jwk(&self) -> Value;
    /// ES256 signature (raw `r || s`) over `message`.
    fn sign_es256(&self, message: &[u8]) -> anyhow::Result<[u8; 64]>;
}

/// Why a JWS or JWK was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum JwsError {
    /// The structure isn't a flattened JWS with the expected parts.
    #[error("the signed request is malformed")]
    Malformed,
    /// The key isn't a P-256 public key.
    #[error("the signing key is not a P-256 public key")]
    BadKey,
    /// The signature doesn't match the signed bytes and key.
    #[error("the signature does not verify")]
    BadSignature,
}

/// A flattened JWS (`protected`, `payload`, `signature`) over `payload` with
/// `header` as the protected header.
///
/// # Errors
/// Returns an error if the header or payload can't be serialized or signing
/// fails.
pub(crate) fn sign_jws(
    signer: &dyn AccountSigner,
    header: &Value,
    payload: &Value,
) -> anyhow::Result<Value> {
    let protected = URL_SAFE_NO_PAD.encode(serde_json::to_vec(header)?);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload)?);
    let signature = signer.sign_es256(format!("{protected}.{payload}").as_bytes())?;
    Ok(json!({
        "protected": protected,
        "payload": payload,
        "signature": URL_SAFE_NO_PAD.encode(signature),
    }))
}

/// A JWS whose signature checked out against the key in its own header.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VerifiedJws {
    pub(crate) protected: Value,
    pub(crate) payload: Value,
    /// The signer's public key from the protected header's `jwk`.
    pub(crate) jwk: Value,
}

/// Check a flattened ES256 JWS against the `jwk` in its protected header.
///
/// # Errors
/// Returns why the request is malformed or the signature is wrong.
pub(crate) fn verify_jws(jws: &Value) -> Result<VerifiedJws, JwsError> {
    let part = |name: &str| {
        jws.get(name)
            .and_then(Value::as_str)
            .ok_or(JwsError::Malformed)
    };
    let (protected_b64, payload_b64, signature_b64) =
        (part("protected")?, part("payload")?, part("signature")?);
    let decode = |text: &str| {
        URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_undecodable| JwsError::Malformed)
    };
    let protected: Value =
        serde_json::from_slice(&decode(protected_b64)?).map_err(|_unparsed| JwsError::Malformed)?;
    let payload: Value =
        serde_json::from_slice(&decode(payload_b64)?).map_err(|_unparsed| JwsError::Malformed)?;
    if protected.get("alg").and_then(Value::as_str) != Some("ES256") {
        return Err(JwsError::Malformed);
    }
    let jwk = protected.get("jwk").cloned().ok_or(JwsError::Malformed)?;
    let point = uncompressed_point(&jwk)?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
        .verify(
            format!("{protected_b64}.{payload_b64}").as_bytes(),
            &decode(signature_b64)?,
        )
        .map_err(|_unverified| JwsError::BadSignature)?;
    Ok(VerifiedJws {
        protected,
        payload,
        jwk,
    })
}

/// The SEC1 uncompressed point (`0x04 || x || y`) for a P-256 JWK.
fn uncompressed_point(jwk: &Value) -> Result<Vec<u8>, JwsError> {
    let coordinate = |name: &str| -> Result<Vec<u8>, JwsError> {
        let bytes = jwk
            .get(name)
            .and_then(Value::as_str)
            .and_then(|text| URL_SAFE_NO_PAD.decode(text).ok())
            .ok_or(JwsError::BadKey)?;
        if bytes.len() == 32 {
            Ok(bytes)
        } else {
            Err(JwsError::BadKey)
        }
    };
    if jwk.get("kty").and_then(Value::as_str) != Some("EC")
        || jwk.get("crv").and_then(Value::as_str) != Some("P-256")
    {
        return Err(JwsError::BadKey);
    }
    let mut point = vec![4_u8];
    point.extend(coordinate("x")?);
    point.extend(coordinate("y")?);
    Ok(point)
}

/// The RFC 7638 SHA-256 thumbprint of a P-256 JWK: a digest of the key's
/// required members in lexicographic order, so any rendering of the same key
/// gives the same bytes.
///
/// # Errors
/// Returns [`JwsError::BadKey`] if `jwk` isn't a P-256 public key.
pub(crate) fn thumbprint(jwk: &Value) -> Result<[u8; 32], JwsError> {
    // Validates the key's shape and coordinate lengths.
    uncompressed_point(jwk)?;
    let member = |name: &str| {
        jwk.get(name)
            .and_then(Value::as_str)
            .ok_or(JwsError::BadKey)
    };
    let canonical = format!(
        r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
        member("x")?,
        member("y")?
    );
    let hash = digest(&SHA256, canonical.as_bytes());
    <[u8; 32]>::try_from(hash.as_ref()).map_err(|_wrong_length| JwsError::BadKey)
}

#[cfg(test)]
pub(crate) mod test_key {
    //! An account key for tests that have no ACME server to register with.

    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};

    use super::{AccountSigner, URL_SAFE_NO_PAD, Value, json};
    use base64::Engine as _;

    pub(crate) struct TestAccount {
        uri: String,
        pair: EcdsaKeyPair,
        rng: SystemRandom,
    }

    impl TestAccount {
        pub(crate) fn new(uri: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("generate a test key");
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .expect("load the test key");
            Self {
                uri: uri.to_string(),
                pair,
                rng,
            }
        }
    }

    impl AccountSigner for TestAccount {
        fn uri(&self) -> &str {
            &self.uri
        }

        fn jwk(&self) -> Value {
            let (x, y) = self
                .pair
                .public_key()
                .as_ref()
                .get(1..)
                .and_then(|coordinates| coordinates.split_at_checked(32))
                .expect("an uncompressed P-256 point");
            json!({
                "kty": "EC",
                "crv": "P-256",
                "x": URL_SAFE_NO_PAD.encode(x),
                "y": URL_SAFE_NO_PAD.encode(y),
            })
        }

        fn sign_es256(&self, message: &[u8]) -> anyhow::Result<[u8; 64]> {
            let signature = self
                .pair
                .sign(&self.rng, message)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(<[u8; 64]>::try_from(signature.as_ref())?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_key::TestAccount;
    use super::*;

    fn signed(account: &TestAccount, payload: &Value) -> Value {
        let header = json!({ "alg": "ES256", "url": "https://x.test/p", "jwk": account.jwk() });
        sign_jws(account, &header, payload).unwrap()
    }

    fn with_payload_of(jws: &Value, other: &Value) -> Value {
        let mut swapped = jws.clone();
        swapped
            .as_object_mut()
            .unwrap()
            .insert("payload".into(), other.get("payload").cloned().unwrap());
        swapped
    }

    #[test]
    fn a_signed_request_verifies_against_its_own_key() {
        let account = TestAccount::new("https://acme.test/acct/1");
        let jws = signed(&account, &json!({ "hello": "world" }));
        let verified = verify_jws(&jws).unwrap();
        assert_eq!(verified.payload.get("hello"), Some(&json!("world")));
        assert_eq!(verified.jwk, account.jwk());
    }

    #[test]
    fn a_changed_payload_or_another_key_fails() {
        let account = TestAccount::new("https://acme.test/acct/1");
        let other = TestAccount::new("https://acme.test/acct/2");
        let original = signed(&account, &json!({ "n": 1 }));
        let forged = with_payload_of(&original, &signed(&account, &json!({ "n": 2 })));
        assert_eq!(verify_jws(&forged), Err(JwsError::BadSignature));

        // The header names another key than the one that signed.
        let header = json!({ "alg": "ES256", "jwk": other.jwk() });
        let swapped = sign_jws(&account, &header, &json!({})).unwrap();
        assert_eq!(verify_jws(&swapped), Err(JwsError::BadSignature));
    }

    #[test]
    fn malformed_requests_and_keys_are_refused() {
        assert_eq!(verify_jws(&json!({})), Err(JwsError::Malformed));
        let account = TestAccount::new("https://acme.test/acct/1");
        let wrong_alg = json!({ "alg": "HS256", "jwk": account.jwk() });
        let jws = sign_jws(&account, &wrong_alg, &json!({})).unwrap();
        assert_eq!(verify_jws(&jws), Err(JwsError::Malformed));
        let wrong_key = json!({ "alg": "ES256", "jwk": { "kty": "RSA" } });
        let keyed_wrong = sign_jws(&account, &wrong_key, &json!({})).unwrap();
        assert_eq!(verify_jws(&keyed_wrong), Err(JwsError::BadKey));
    }

    #[test]
    fn thumbprints_follow_rfc_7638_and_ignore_extra_members() {
        let account = TestAccount::new("https://acme.test/acct/1");
        let plain = thumbprint(&account.jwk()).unwrap();
        let mut annotated = account.jwk();
        annotated
            .as_object_mut()
            .unwrap()
            .insert("kid".into(), json!("anything"));
        assert_eq!(thumbprint(&annotated).unwrap(), plain);
        let other = TestAccount::new("https://acme.test/acct/2");
        assert_ne!(thumbprint(&other.jwk()).unwrap(), plain);
        assert_eq!(thumbprint(&json!({ "kty": "EC" })), Err(JwsError::BadKey));
    }
}

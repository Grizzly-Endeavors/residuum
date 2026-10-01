//! The hub's VAPID key pair (RFC 8292).
//!
//! The key pair identifies the hub to push services: each subscription is
//! bound to the public key it was created with (the `applicationServerKey`),
//! and each push request carries a short-lived JWT signed with the private
//! key. Replacing the key would make every existing subscription useless, so
//! it is created once, on first use, and never regenerated automatically.

use std::path::Path;

use anyhow::{Context as _, anyhow};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};

use super::encrypt::base64url_encode;
use super::error::PushError;

/// How long a signed JWT is valid. RFC 8292 caps it at 24 hours.
const JWT_LIFETIME_SECS: i64 = 12 * 3600;

/// The VAPID signing key, loaded.
pub(super) struct VapidKey {
    pair: EcdsaKeyPair,
}

impl VapidKey {
    /// Load the key at `path`, or generate one there (mode 0600) when the
    /// file doesn't exist.
    ///
    /// A file that exists but can't be read as a key is an error and is left
    /// where it is: replacing it would sign every device out of
    /// notifications.
    ///
    /// # Errors
    /// Returns [`PushError::Failed`] if the file can't be read or parsed, or
    /// a new key can't be generated or saved.
    pub(super) async fn load_or_create(path: &Path) -> Result<Self, PushError> {
        match tokio::fs::read(path).await {
            Ok(pkcs8) => Self::from_pkcs8(&pkcs8).map_err(|e| {
                tracing::error!(error = %e, path = %path.display(), "the push signing key can't be used");
                PushError::Failed(format!(
                    "The notification signing key at {} can't be used ({e}). Residuum left it \
                     alone, because replacing it would end notifications on every device. To \
                     start over, move that file away and turn notifications on again on each \
                     device.",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::create(path).await,
            Err(e) => {
                tracing::error!(error = %e, path = %path.display(), "failed to read the push signing key");
                Err(PushError::Failed(format!(
                    "Residuum couldn't read its notification signing key at {}: {e}.",
                    path.display()
                )))
            }
        }
    }

    async fn create(path: &Path) -> Result<Self, PushError> {
        let failed = |e: &dyn std::fmt::Display| {
            tracing::error!(error = %e, path = %path.display(), "failed to create the push signing key");
            PushError::Failed(format!(
                "Residuum couldn't create its notification signing key at {}: {e}.",
                path.display()
            ))
        };
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_unspecified| failed(&"the system couldn't supply random bytes"))?;
        crate::util::fs::atomic_write_owner_only(path, pkcs8.as_ref())
            .await
            .map_err(|e| failed(&format!("{e:#}")))?;
        tracing::info!(path = %path.display(), "generated the push signing key");
        Self::from_pkcs8(pkcs8.as_ref()).map_err(|e| failed(&e))
    }

    fn from_pkcs8(pkcs8: &[u8]) -> anyhow::Result<Self> {
        let pair = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|e| anyhow!("it isn't a P-256 private key: {e}"))?;
        Ok(Self { pair })
    }

    /// The public key in uncompressed form, which a browser subscribes with.
    pub(super) fn public_key(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    /// The public key as base64url, the form `applicationServerKey` takes.
    pub(super) fn public_key_base64url(&self) -> String {
        base64url_encode(self.public_key())
    }

    /// The `Authorization` header for a push to a service at `audience` (the
    /// origin of the subscription's endpoint): a JWT signed with this key,
    /// naming `subject` as the contact, valid for 12 hours from `now_unix`.
    ///
    /// # Errors
    /// Returns an error if signing fails.
    pub(super) fn authorization(
        &self,
        audience: &str,
        subject: &str,
        now_unix: i64,
    ) -> anyhow::Result<String> {
        let jwt = self.sign_jwt(audience, subject, now_unix + JWT_LIFETIME_SECS)?;
        Ok(format!("vapid t={jwt}, k={}", self.public_key_base64url()))
    }

    fn sign_jwt(&self, audience: &str, subject: &str, expires_at: i64) -> anyhow::Result<String> {
        let header = base64url_encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = base64url_encode(
            serde_json::json!({ "aud": audience, "exp": expires_at, "sub": subject })
                .to_string()
                .as_bytes(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .map_err(|_unspecified| anyhow!("the signing key rejected the message"))
            .context("failed to sign the VAPID token")?;
        Ok(format!(
            "{signing_input}.{}",
            base64url_encode(signature.as_ref())
        ))
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;

    fn decode(part: &str) -> Vec<u8> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(part)
            .unwrap()
    }

    #[tokio::test]
    async fn the_key_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-vapid.key");

        let first = VapidKey::load_or_create(&path).await.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let second = VapidKey::load_or_create(&path).await.unwrap();

        assert_eq!(first.public_key(), second.public_key());
        assert_eq!(first.public_key().len(), 65);
        assert_eq!(first.public_key().first(), Some(&0x04));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the file is not rewritten"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_key_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-vapid.key");
        VapidKey::load_or_create(&path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn an_unreadable_key_file_is_left_alone_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push-vapid.key");
        std::fs::write(&path, b"not a key").unwrap();

        let err = VapidKey::load_or_create(&path).await.err().unwrap();
        assert!(matches!(err, PushError::Failed(_)));
        assert!(err.to_string().contains("push-vapid.key"), "{err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"not a key",
            "a key that can't be read is never replaced"
        );
    }

    #[tokio::test]
    async fn the_public_key_is_base64url_of_the_uncompressed_point() {
        let dir = tempfile::tempdir().unwrap();
        let key = VapidKey::load_or_create(&dir.path().join("k"))
            .await
            .unwrap();
        assert_eq!(decode(&key.public_key_base64url()), key.public_key());
    }

    #[tokio::test]
    async fn the_authorization_header_carries_a_jwt_the_public_key_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let key = VapidKey::load_or_create(&dir.path().join("k"))
            .await
            .unwrap();

        let header = key
            .authorization("https://push.example.com", "mailto:bear@example.com", 1_000)
            .unwrap();
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (jwt, public) = rest.split_once(", k=").unwrap();
        assert_eq!(public, key.public_key_base64url());

        let mut parts = jwt.split('.');
        let (header_part, claims_part, signature_part) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        assert!(parts.next().is_none(), "a JWT has three parts");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&decode(header_part)).unwrap(),
            serde_json::json!({ "typ": "JWT", "alg": "ES256" })
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&decode(claims_part)).unwrap(),
            serde_json::json!({
                "aud": "https://push.example.com",
                "exp": 1_000 + 12 * 3600,
                "sub": "mailto:bear@example.com",
            })
        );

        // ES256 signatures are the 64-byte r||s form, verified against the
        // public key a browser subscribes with.
        let signature = decode(signature_part);
        assert_eq!(signature.len(), 64);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, key.public_key())
            .verify(
                format!("{header_part}.{claims_part}").as_bytes(),
                &signature,
            )
            .unwrap();
    }

    #[tokio::test]
    async fn a_token_does_not_verify_with_another_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = VapidKey::load_or_create(&dir.path().join("a"))
            .await
            .unwrap();
        let other = VapidKey::load_or_create(&dir.path().join("b"))
            .await
            .unwrap();

        let jwt = key
            .sign_jwt("https://push.example.com", "mailto:a@b.c", 1)
            .unwrap();
        let (signing_input, signature) = jwt.rsplit_once('.').unwrap();
        assert!(
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, other.public_key())
                .verify(signing_input.as_bytes(), &decode(signature))
                .is_err()
        );
    }
}

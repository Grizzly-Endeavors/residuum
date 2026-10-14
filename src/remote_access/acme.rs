//! ACME account, certificate orders (TLS-ALPN-01), renewal information and the on-disk state.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use instant_acme::{
    Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, Key, NewOrder,
    OrderStatus, RetryPolicy,
};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{debug, info, warn};

use super::jws::AccountSigner;
use super::tls::{CertBundle, CertResolver, leaf_validity};
use super::types::normalize_host;
use crate::util::fs::atomic_write_owner_only;

/// How long one order may take from creation to an issued certificate.
const ORDER_POLL_TIMEOUT: Duration = Duration::from_secs(180);

/// Where the ACME server lives and where this instance keeps its state.
#[derive(Debug, Clone)]
pub(crate) struct AcmeSettings {
    /// ACME directory URL of the CA.
    pub directory_url: String,
    /// Root certificate that signs the CA's own TLS certificate, for private CAs and
    /// test CAs. When set it replaces the built-in web roots for talking to the CA.
    pub root_ca_pem: Option<PathBuf>,
    /// Directory for the account credentials and the certificate; files are mode 0600.
    pub state_dir: PathBuf,
}

/// Short stable file-name fragment for a directory URL, so that switching CA
/// never mixes accounts or certificates.
fn directory_fingerprint(directory_url: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, directory_url.as_bytes());
    let bytes = digest.as_ref();
    hex::encode(bytes.get(..8).unwrap_or(bytes))
}

/// The account's P-256 signing key: the one ACME requests are signed with, also
/// used to sign requests to the pin service.
struct AccountKey {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl AccountKey {
    fn from_pkcs8(pkcs8: &[u8]) -> anyhow::Result<Self> {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
            .map_err(|e| anyhow::anyhow!("account key is not a P-256 PKCS#8 key: {e}"))?;
        Ok(Self { pair, rng })
    }

    fn jwk(&self) -> serde_json::Value {
        // Uncompressed SEC1 point: 0x04, then 32 bytes each of x and y.
        let point = self.pair.public_key().as_ref();
        let x = point.get(1..33).unwrap_or_default();
        let y = point.get(33..65).unwrap_or_default();
        serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": BASE64_URL_SAFE_NO_PAD.encode(x),
            "y": BASE64_URL_SAFE_NO_PAD.encode(y),
        })
    }

    fn sign_es256(&self, message: &[u8]) -> anyhow::Result<[u8; 64]> {
        let signature = self
            .pair
            .sign(&self.rng, message)
            .map_err(|e| anyhow::anyhow!("failed to sign with the account key: {e}"))?;
        <[u8; 64]>::try_from(signature.as_ref())
            .map_err(|_wrong_length| anyhow::anyhow!("account signature is not 64 bytes"))
    }
}

/// An ACME account and its signing key.
pub(crate) struct AcmeAccount {
    account: Account,
    key: AccountKey,
}

impl fmt::Debug for AcmeAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcmeAccount")
            .field("uri", &self.account.id())
            .finish_non_exhaustive()
    }
}

impl AcmeAccount {
    /// Restore the account stored for `settings.directory_url`, or register a
    /// new one (agreeing to the terms of service, no contact) and store it.
    ///
    /// # Errors
    /// Returns an error if stored credentials are unreadable (they are never
    /// replaced silently, because CAA records may pin the account), the CA
    /// is unreachable, or registration or storing fails.
    pub(crate) async fn load_or_create(settings: &AcmeSettings) -> anyhow::Result<Self> {
        // The ACME HTTP client picks up the process-wide rustls provider, and this build
        // links both ring and aws-lc-rs so rustls cannot choose one itself. Another
        // provider installed first is fine, which is why the result is ignored.
        drop(rustls::crypto::ring::default_provider().install_default());
        let path = settings.state_dir.join(format!(
            "acme-account-{}.json",
            directory_fingerprint(&settings.directory_url)
        ));
        let builder = || match &settings.root_ca_pem {
            Some(root) => Account::builder_with_root(root).with_context(|| {
                format!(
                    "failed to load the ACME root certificate {}",
                    root.display()
                )
            }),
            None => Account::builder().context("failed to set up the ACME HTTP client"),
        };

        let (account, key) = match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let credentials: instant_acme::AccountCredentials = serde_json::from_slice(&bytes)
                    .with_context(|| {
                        format!(
                            "failed to parse the ACME account credentials at {}",
                            path.display()
                        )
                    })?;
                let key = AccountKey::from_pkcs8(credentials.private_key().secret_pkcs8_der())?;
                let account = builder()?
                    .from_credentials(credentials)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to restore the ACME account from {} at {}",
                            path.display(),
                            settings.directory_url
                        )
                    })?;
                (account, key)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let (generated, pkcs8) =
                    Key::generate_pkcs8().context("failed to generate the ACME account key")?;
                let (account, credentials) = builder()?
                    .create_from_key(
                        (generated, PrivateKeyDer::Pkcs8(pkcs8)),
                        settings.directory_url.clone(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to register an ACME account at {}",
                            settings.directory_url
                        )
                    })?;
                tokio::fs::create_dir_all(&settings.state_dir)
                    .await
                    .with_context(|| {
                        format!("failed to create {}", settings.state_dir.display())
                    })?;
                let json = serde_json::to_vec_pretty(&credentials)
                    .context("failed to serialize the ACME account credentials")?;
                atomic_write_owner_only(&path, json)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to store the ACME account credentials at {}",
                            path.display()
                        )
                    })?;
                let key = AccountKey::from_pkcs8(credentials.private_key().secret_pkcs8_der())?;
                info!(account = account.id(), directory = %settings.directory_url, "registered ACME account");
                (account, key)
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "failed to read the ACME account credentials at {}",
                        path.display()
                    )
                });
            }
        };

        Ok(Self { account, key })
    }
}

impl AccountSigner for AcmeAccount {
    fn uri(&self) -> &str {
        self.account.id()
    }

    fn jwk(&self) -> serde_json::Value {
        self.key.jwk()
    }

    fn sign_es256(&self, message: &[u8]) -> anyhow::Result<[u8; 64]> {
        self.key.sign_es256(message)
    }
}

/// One order for a certificate covering `names`, answered with TLS-ALPN-01.
pub(crate) struct CertificateOrder<'a> {
    /// Account placing the order.
    pub account: &'a AcmeAccount,
    /// Resolver whose challenge slots answer the CA's validation handshakes.
    pub resolver: &'a Arc<CertResolver>,
    /// DNS names to certify.
    pub names: &'a [String],
}

impl CertificateOrder<'_> {
    /// Place the order, answer the challenges, finalize and fetch the certificate.
    ///
    /// The caller must already hold the relay-side challenge claim for the names
    /// (so the CA's validation connections reach this instance) and releases it
    /// afterwards. Challenge slots on the resolver are always cleared again.
    ///
    /// # Errors
    /// Returns an error if the CA offers no TLS-ALPN-01 challenge, rejects an
    /// authorization, or the order does not complete in time.
    pub(crate) async fn run(&self) -> anyhow::Result<CertBundle> {
        let names: Vec<String> = self.names.iter().map(|n| normalize_host(n)).collect();
        anyhow::ensure!(!names.is_empty(), "certificate order has no names");

        let mut armed = Vec::with_capacity(names.len());
        let outcome = self.order(&names, &mut armed).await;
        for name in &armed {
            self.resolver.clear_challenge(name);
        }
        let (chain_pem, key_pem) = outcome
            .with_context(|| format!("certificate order for {} failed", names.join(", ")))?;

        let (not_before, not_after) =
            leaf_validity(&chain_pem).context("the CA issued a certificate that cannot be read")?;
        info!(names = ?names, %not_after, "issued certificate");
        Ok(CertBundle {
            chain_pem,
            key_pem,
            not_before,
            not_after,
            names,
        })
    }

    async fn order(
        &self,
        names: &[String],
        armed: &mut Vec<String>,
    ) -> anyhow::Result<(String, String)> {
        let identifiers: Vec<Identifier> =
            names.iter().map(|n| Identifier::Dns(n.clone())).collect();
        let mut order = self
            .account
            .account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .context("failed to create the order")?;

        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result.context("failed to fetch an authorization")?;
            let name = authz.identifier().to_string();
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other @ (AuthorizationStatus::Invalid
                | AuthorizationStatus::Revoked
                | AuthorizationStatus::Expired
                | AuthorizationStatus::Deactivated) => {
                    anyhow::bail!("authorization for {name} is {other:?}")
                }
            }
            let mut challenge = authz
                .challenge(ChallengeType::TlsAlpn01)
                .with_context(|| format!("the CA offered no tls-alpn-01 challenge for {name}"))?;
            let digest: [u8; 32] = challenge
                .key_authorization()
                .digest()
                .as_ref()
                .try_into()
                .context("key authorization digest is not 32 bytes")?;
            self.resolver.set_challenge(&name, digest)?;
            armed.push(name.clone());
            challenge
                .set_ready()
                .await
                .with_context(|| format!("failed to start validation of {name}"))?;
        }

        let policy = RetryPolicy::new()
            .initial_delay(Duration::from_millis(500))
            .backoff(1.5)
            .timeout(ORDER_POLL_TIMEOUT);
        let status = order
            .poll_ready(&policy)
            .await
            .context("failed while waiting for the CA to validate the names")?;
        if status != OrderStatus::Ready {
            let reasons = rejected_challenges(&mut order).await;
            anyhow::bail!("order ended {status:?}{reasons}");
        }

        let key_pem = order
            .finalize()
            .await
            .context("failed to finalize the order")?;
        let chain_pem = order
            .poll_certificate(&policy)
            .await
            .context("failed while waiting for the certificate")?;
        Ok((chain_pem, key_pem))
    }
}

/// What the CA said about each failed challenge, as a ready-to-append suffix (empty when unknown).
async fn rejected_challenges(order: &mut instant_acme::Order) -> String {
    let mut reasons = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(result) = authorizations.next().await {
        let Ok(authz) = result else { break };
        for challenge in &authz.challenges {
            if let Some(problem) = &challenge.error {
                reasons.push(format!("{}: {problem}", authz.identifier()));
            }
        }
    }
    if reasons.is_empty() {
        String::new()
    } else {
        format!(" ({})", reasons.join("; "))
    }
}

/// When the CA suggests renewing `bundle` (ARI, RFC 9773): the start of the
/// suggested window. `None` when the CA does not support ARI or the lookup
/// fails; the caller then renews by [`CertBundle::renew_at`].
pub(crate) async fn renewal_window(
    account: &AcmeAccount,
    bundle: &CertBundle,
) -> Option<DateTime<Utc>> {
    let leaf = match CertificateDer::pem_slice_iter(bundle.chain_pem.as_bytes()).next() {
        Some(Ok(leaf)) => leaf,
        Some(Err(error)) => {
            debug!(%error, "cannot read the leaf certificate for the renewal information lookup");
            return None;
        }
        None => {
            debug!("certificate chain is empty, skipping the renewal information lookup");
            return None;
        }
    };
    let identifier = match CertificateIdentifier::try_from(&leaf) {
        Ok(identifier) => identifier,
        Err(error) => {
            debug!(%error, "cannot derive the ARI certificate identifier");
            return None;
        }
    };
    match account.account.renewal_info(&identifier).await {
        Ok((info, _poll_after)) => {
            let start = info.suggested_window.start;
            let at = DateTime::from_timestamp(start.unix_timestamp(), start.nanosecond());
            if at.is_none() {
                debug!(%start, "CA suggested a renewal time that is out of range");
            }
            at
        }
        Err(error) => {
            debug!(%error, "CA gave no renewal information");
            None
        }
    }
}

/// The issued certificate, kept on disk so a restart does not order again.
pub(crate) struct CertStore {
    path: PathBuf,
}

impl CertStore {
    /// Store for the certificate issued by `directory_url`, inside `state_dir`.
    pub(crate) fn new(state_dir: &Path, directory_url: &str) -> Self {
        Self {
            path: state_dir.join(format!(
                "certificate-{}.json",
                directory_fingerprint(directory_url)
            )),
        }
    }

    /// The stored bundle, or `None` when nothing usable is stored. A file that
    /// exists but cannot be read is logged at warn and treated as absent.
    pub(crate) fn load(&self) -> Option<CertBundle> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                warn!(%error, path = %self.path.display(), "failed to read the stored certificate");
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(bundle) => Some(bundle),
            Err(error) => {
                warn!(%error, path = %self.path.display(), "stored certificate is corrupt, ignoring it");
                None
            }
        }
    }

    /// Replace the stored bundle (mode 0600).
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or the file written.
    pub(crate) async fn save(&self, bundle: &CertBundle) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let json =
            serde_json::to_vec_pretty(bundle).context("failed to serialize the certificate")?;
        atomic_write_owner_only(&self.path, json)
            .await
            .with_context(|| format!("failed to store the certificate at {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    fn test_key() -> AccountKey {
        let (_, pkcs8) = Key::generate_pkcs8().unwrap();
        AccountKey::from_pkcs8(pkcs8.secret_pkcs8_der()).unwrap()
    }

    #[test]
    fn jwk_has_the_p256_public_coordinates() {
        let key = test_key();
        let jwk = key.jwk();
        assert_eq!(
            jwk.get("kty").and_then(serde_json::Value::as_str),
            Some("EC")
        );
        assert_eq!(
            jwk.get("crv").and_then(serde_json::Value::as_str),
            Some("P-256")
        );
        assert_eq!(jwk_coordinate(&jwk, "x").len(), 32);
        assert_eq!(jwk_coordinate(&jwk, "y").len(), 32);
        assert_eq!(jwk.as_object().unwrap().len(), 4);
    }

    #[test]
    fn signature_verifies_against_the_jwk() {
        let key = test_key();
        let jwk = key.jwk();
        let point = uncompressed_point(&jwk);

        let message = b"pin service request";
        let signature = key.sign_es256(message).unwrap();
        let public = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point);
        public.verify(message, &signature).unwrap();
        assert!(public.verify(b"another message", &signature).is_err());
    }

    #[test]
    fn account_key_rejects_garbage() {
        assert!(AccountKey::from_pkcs8(b"not a key").is_err());
    }

    #[test]
    fn directory_fingerprints_differ_per_directory() {
        let a = directory_fingerprint("https://ca.example/dir");
        assert_eq!(a, directory_fingerprint("https://ca.example/dir"));
        assert_ne!(a, directory_fingerprint("https://ca.example/staging"));
    }

    fn sample_bundle() -> CertBundle {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        CertBundle {
            chain_pem: "CHAIN".to_owned(),
            key_pem: "KEY".to_owned(),
            not_before: now,
            not_after: now + chrono::Duration::days(90),
            names: vec!["a.example.test".to_owned()],
        }
    }

    #[tokio::test]
    async fn cert_store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(&dir.path().join("nested"), "https://ca.example/dir");
        assert!(store.load().is_none());

        let bundle = sample_bundle();
        store.save(&bundle).await.unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.chain_pem, bundle.chain_pem);
        assert_eq!(loaded.key_pem, bundle.key_pem);
        assert_eq!(loaded.not_before, bundle.not_before);
        assert_eq!(loaded.not_after, bundle.not_after);
        assert_eq!(loaded.names, bundle.names);
    }

    #[tokio::test]
    async fn cert_store_is_keyed_by_directory() {
        let dir = tempfile::tempdir().unwrap();
        CertStore::new(dir.path(), "https://one.example/dir")
            .save(&sample_bundle())
            .await
            .unwrap();
        assert!(
            CertStore::new(dir.path(), "https://two.example/dir")
                .load()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cert_store_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path(), "https://ca.example/dir");
        store.save(&sample_bundle()).await.unwrap();
        let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn corrupt_stored_certificate_loads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path(), "https://ca.example/dir");
        std::fs::write(&store.path, b"{not json").unwrap();
        assert!(store.load().is_none());
    }

    // Pebble integration tests: `cargo test --quiet pebble -- --ignored` (needs Docker).

    use crate::remote_access::challenge::san_dns_names;
    use crate::remote_access::pebble_support::PebbleHarness;

    const NAMES: [&str; 3] = [
        "alice.lab.test",
        "alice.workbench.lab.test",
        "laptop.alice.lab.test",
    ];

    fn names() -> Vec<String> {
        NAMES.iter().map(|n| (*n).to_owned()).collect()
    }

    async fn register_names(pebble: &PebbleHarness) {
        for name in NAMES {
            pebble.add_a(name).await.unwrap();
        }
    }

    fn leaf_dns_names(chain_pem: &str) -> Vec<String> {
        let leaf = CertificateDer::from_pem_slice(chain_pem.as_bytes()).unwrap();
        san_dns_names(leaf.as_ref())
    }

    fn jwk_coordinate(jwk: &serde_json::Value, axis: &str) -> Vec<u8> {
        let encoded = jwk.get(axis).and_then(serde_json::Value::as_str).unwrap();
        BASE64_URL_SAFE_NO_PAD.decode(encoded).unwrap()
    }

    fn uncompressed_point(jwk: &serde_json::Value) -> Vec<u8> {
        let mut point = vec![0x04];
        point.extend(jwk_coordinate(jwk, "x"));
        point.extend(jwk_coordinate(jwk, "y"));
        point
    }

    #[tokio::test]
    #[ignore = "needs docker: runs Pebble"]
    async fn pebble_orders_three_names_then_renews() {
        let pebble = PebbleHarness::start().await.unwrap();
        register_names(&pebble).await;
        let resolver = CertResolver::new();
        assert!(pebble.root_ca_pem_path().is_file());
        assert_ne!(pebble.tls_port(), 0);
        let _listener = pebble.serve_tls_alpn(&resolver).unwrap();
        let state = pebble.scratch_dir().join("state");
        let account = AcmeAccount::load_or_create(&pebble.acme_settings(&state))
            .await
            .unwrap();
        let names = names();

        let order = CertificateOrder {
            account: &account,
            resolver: &resolver,
            names: &names,
        };
        let first = order.run().await.unwrap();
        let mut expected = names.clone();
        expected.sort();
        assert_eq!(leaf_dns_names(&first.chain_pem), expected);
        assert_eq!(first.names, names);
        let lifetime = first.not_after - first.not_before;
        assert!(
            lifetime > chrono::Duration::days(89) && lifetime < chrono::Duration::days(91),
            "lifetime {lifetime}"
        );
        assert_eq!(first.not_after - first.renew_at(), lifetime / 3);

        // The issued bundle is usable by the resolver and survives the store.
        resolver.set_certificate(&first).unwrap();
        let store = CertStore::new(&state, &pebble.directory_url());
        store.save(&first).await.unwrap();
        assert_eq!(store.load().unwrap().chain_pem, first.chain_pem);

        // ARI is optional for a CA: either answer is acceptable, neither is an error.
        let window = renewal_window(&account, &first).await;
        info!(?window, "renewal window");
        if let Some(start) = window {
            assert!(
                start >= first.not_before && start <= first.not_after,
                "{start}"
            );
        }

        let second = order.run().await.unwrap();
        assert_eq!(leaf_dns_names(&second.chain_pem), expected);
        assert_ne!(
            second.chain_pem, first.chain_pem,
            "renewal issues a new certificate"
        );
        assert_ne!(second.key_pem, first.key_pem, "renewal uses a new key");
    }

    #[tokio::test]
    #[ignore = "needs docker: runs Pebble"]
    async fn pebble_account_is_restored_from_the_state_dir() {
        let pebble = PebbleHarness::start().await.unwrap();
        let state = pebble.scratch_dir().join("state");
        let settings = pebble.acme_settings(&state);

        let created = AcmeAccount::load_or_create(&settings).await.unwrap();
        let restored = AcmeAccount::load_or_create(&settings).await.unwrap();
        assert_eq!(created.uri(), restored.uri());
        assert_eq!(created.jwk(), restored.jwk());

        let message = b"proof of possession";
        let signature = restored.sign_es256(message).unwrap();
        let jwk = created.jwk();
        let point = uncompressed_point(&jwk);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &point)
            .verify(message, &signature)
            .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let stored = std::fs::read_dir(&state).unwrap().next().unwrap().unwrap();
            assert_eq!(
                stored.metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    #[ignore = "needs docker: runs Pebble"]
    async fn pebble_failed_validation_names_the_reason_and_clears_challenges() {
        let pebble = PebbleHarness::start().await.unwrap();
        register_names(&pebble).await;
        let resolver = CertResolver::new();
        // No listener answers on the validation port, so the CA cannot validate.
        let state = pebble.scratch_dir().join("state");
        let account = AcmeAccount::load_or_create(&pebble.acme_settings(&state))
            .await
            .unwrap();
        let names = names();

        let error = CertificateOrder {
            account: &account,
            resolver: &resolver,
            names: &names,
        }
        .run()
        .await
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("alice.lab.test"), "{message}");
        for name in NAMES {
            assert!(
                resolver.select_certificate(Some(name), true).is_none(),
                "challenge for {name} was left armed"
            );
        }
    }
}

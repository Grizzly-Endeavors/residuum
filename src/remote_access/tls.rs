//! Certificate bundles and the TLS certificate resolver for remote access.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};

use super::challenge::{ACME_TLS_ALPN, KEY_AUTHORIZATION_DIGEST_LEN, acme_identifier_certificate};
use super::types::normalize_host;

/// An issued certificate chain with its private key and the names it covers.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CertBundle {
    /// Leaf first, then intermediates, PEM encoded.
    pub chain_pem: String,
    /// PKCS#8 private key of the leaf, PEM encoded.
    pub key_pem: String,
    /// Start of the leaf's validity.
    pub not_before: DateTime<Utc>,
    /// End of the leaf's validity.
    pub not_after: DateTime<Utc>,
    /// DNS names the certificate was ordered for.
    pub names: Vec<String>,
}

impl fmt::Debug for CertBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertBundle")
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

impl CertBundle {
    /// When to start renewing: with one third of the lifetime left.
    pub(crate) fn renew_at(&self) -> DateTime<Utc> {
        self.not_after - (self.not_after - self.not_before) / 3
    }
}

/// Validity window of the first (leaf) certificate in a PEM chain.
pub(crate) fn leaf_validity(chain_pem: &str) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let leaf = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
        .next()
        .context("certificate chain is empty")?
        .context("failed to parse the leaf certificate PEM")?;
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|e| anyhow::anyhow!("failed to parse the leaf certificate: {e}"))?;
    let validity = parsed.validity();
    let to_chrono = |seconds: i64| {
        DateTime::from_timestamp(seconds, 0)
            .with_context(|| format!("certificate time {seconds} is out of range"))
    };
    Ok((
        to_chrono(validity.not_before.timestamp())?,
        to_chrono(validity.not_after.timestamp())?,
    ))
}

struct ServingCertificate {
    certified: Arc<CertifiedKey>,
    /// Normalized names the certificate may be served for.
    names: Vec<String>,
}

#[derive(Default)]
struct ResolverState {
    certificate: Option<ServingCertificate>,
    /// Normalized name to its TLS-ALPN-01 answer.
    challenges: HashMap<String, Arc<CertifiedKey>>,
}

/// Picks the certificate for each TLS handshake: the instance certificate for
/// its own names, or the TLS-ALPN-01 answer when a CA asks for it.
pub(crate) struct CertResolver {
    state: RwLock<ResolverState>,
}

impl fmt::Debug for CertResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertResolver")
            .field("has_certificate", &self.has_certificate())
            .finish_non_exhaustive()
    }
}

impl CertResolver {
    /// A resolver with no certificate yet; handshakes fail until one is set.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(ResolverState::default()),
        })
    }

    /// Serve `bundle` from now on, for exactly the names it lists.
    ///
    /// # Errors
    /// Returns an error if the chain or key cannot be parsed or do not belong together.
    pub(crate) fn set_certificate(&self, bundle: &CertBundle) -> anyhow::Result<()> {
        let chain = CertificateDer::pem_slice_iter(bundle.chain_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse the certificate chain PEM")?;
        anyhow::ensure!(
            !chain.is_empty(),
            "certificate chain contains no certificates"
        );
        let key = PrivateKeyDer::from_pem_slice(bundle.key_pem.as_bytes())
            .context("failed to parse the private key PEM")?;
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
            .context("private key is not usable for TLS")?;
        let certified = CertifiedKey::new(chain, signing_key);
        certified
            .keys_match()
            .context("private key does not match the certificate")?;

        let names = bundle.names.iter().map(|n| normalize_host(n)).collect();
        self.write().certificate = Some(ServingCertificate {
            certified: Arc::new(certified),
            names,
        });
        Ok(())
    }

    /// Whether an instance certificate has been set.
    pub(crate) fn has_certificate(&self) -> bool {
        self.read().certificate.is_some()
    }

    /// Answer TLS-ALPN-01 handshakes for `name` with a certificate carrying `key_authorization_digest`.
    ///
    /// # Errors
    /// Returns an error if the challenge certificate cannot be generated.
    pub(crate) fn set_challenge(
        &self,
        name: &str,
        key_authorization_digest: [u8; KEY_AUTHORIZATION_DIGEST_LEN],
    ) -> anyhow::Result<()> {
        let certified = acme_identifier_certificate(name, key_authorization_digest)?;
        self.write()
            .challenges
            .insert(normalize_host(name), Arc::new(certified));
        Ok(())
    }

    /// Stop answering TLS-ALPN-01 handshakes for `name`.
    pub(crate) fn clear_challenge(&self, name: &str) {
        self.write().challenges.remove(&normalize_host(name));
    }

    /// Server configuration that resolves certificates through this resolver:
    /// ALPN `h2`, `http/1.1` and `acme-tls/1`, TLS 1.2 and 1.3, no client auth.
    ///
    /// # Errors
    /// Returns an error if the TLS protocol versions are not supported by the crypto provider.
    pub(crate) fn server_config(self: &Arc<Self>) -> anyhow::Result<Arc<rustls::ServerConfig>> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .context("failed to configure TLS protocol versions")?
            .with_no_client_auth()
            .with_cert_resolver(Arc::clone(self) as Arc<dyn ResolvesServerCert>);
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
        Ok(Arc::new(config))
    }

    pub(super) fn select_certificate(
        &self,
        server_name: Option<&str>,
        offers_acme_alpn: bool,
    ) -> Option<Arc<CertifiedKey>> {
        let name = normalize_host(server_name?);
        let state = self.read();
        if offers_acme_alpn && let Some(answer) = state.challenges.get(&name) {
            return Some(Arc::clone(answer));
        }
        let serving = state.certificate.as_ref()?;
        serving
            .names
            .contains(&name)
            .then(|| Arc::clone(&serving.certified))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, ResolverState> {
        // The state is replaced whole, so a poisoned lock still holds consistent data.
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, ResolverState> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let offers_acme_alpn = client_hello
            .alpn()
            .is_some_and(|mut protocols| protocols.any(|p| p == ACME_TLS_ALPN));
        self.select_certificate(client_hello.server_name(), offers_acme_alpn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn bundle_with_lifetime(days: i64) -> CertBundle {
        let not_before = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        CertBundle {
            chain_pem: String::new(),
            key_pem: String::new(),
            not_before,
            not_after: not_before + Duration::days(days),
            names: vec![],
        }
    }

    #[test]
    fn renew_at_leaves_a_third_of_a_90_day_lifetime() {
        let bundle = bundle_with_lifetime(90);
        assert_eq!(bundle.not_after - bundle.renew_at(), Duration::days(30));
    }

    #[test]
    fn renew_at_leaves_a_third_of_a_45_day_lifetime() {
        let bundle = bundle_with_lifetime(45);
        assert_eq!(bundle.not_after - bundle.renew_at(), Duration::days(15));
    }

    #[test]
    fn debug_never_prints_the_key() {
        let mut bundle = bundle_with_lifetime(1);
        bundle.key_pem = "SECRET-KEY-MATERIAL".to_owned();
        assert!(!format!("{bundle:?}").contains("SECRET"));
    }

    /// A self-signed certificate for `names`, standing in for an issued one.
    pub(crate) fn test_bundle(names: &[&str]) -> CertBundle {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(
            names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
        )
        .unwrap();
        let cert = params.self_signed(&key).unwrap();
        let chain_pem = cert.pem();
        let (not_before, not_after) = leaf_validity(&chain_pem).unwrap();
        CertBundle {
            chain_pem,
            key_pem: key.serialize_pem(),
            not_before,
            not_after,
            names: names.iter().map(|n| (*n).to_owned()).collect(),
        }
    }

    #[test]
    fn leaf_validity_reads_the_certificate_dates() {
        let bundle = test_bundle(&["a.example.test"]);
        assert!(bundle.not_before < Utc::now());
        assert!(bundle.not_after > Utc::now());
    }

    #[test]
    fn resolver_without_certificate_serves_nothing() {
        let resolver = CertResolver::new();
        assert!(!resolver.has_certificate());
        assert!(
            resolver
                .select_certificate(Some("a.example.test"), false)
                .is_none()
        );
        assert!(
            resolver
                .select_certificate(Some("a.example.test"), true)
                .is_none()
        );
    }

    #[test]
    fn resolver_serves_only_the_bundle_names() {
        let resolver = CertResolver::new();
        resolver
            .set_certificate(&test_bundle(&["a.example.test", "b.example.test"]))
            .unwrap();
        assert!(resolver.has_certificate());
        assert!(
            resolver
                .select_certificate(Some("a.example.test"), false)
                .is_some()
        );
        assert!(
            resolver
                .select_certificate(Some("B.Example.Test."), false)
                .is_some()
        );
        assert!(
            resolver
                .select_certificate(Some("evil.example.test"), false)
                .is_none()
        );
        assert!(resolver.select_certificate(None, false).is_none());
    }

    #[test]
    fn set_certificate_rejects_mismatched_key() {
        let resolver = CertResolver::new();
        let mut bundle = test_bundle(&["a.example.test"]);
        bundle.key_pem = rcgen::KeyPair::generate().unwrap().serialize_pem();
        let err = resolver.set_certificate(&bundle).unwrap_err();
        assert!(format!("{err:#}").contains("does not match"), "{err:#}");
        assert!(!resolver.has_certificate());
    }

    #[test]
    fn set_certificate_rejects_garbage() {
        let resolver = CertResolver::new();
        let mut bundle = test_bundle(&["a.example.test"]);
        bundle.chain_pem = "not a pem".to_owned();
        assert!(resolver.set_certificate(&bundle).is_err());
    }

    #[test]
    fn challenge_answers_only_acme_alpn_for_its_name() {
        let resolver = CertResolver::new();
        resolver
            .set_certificate(&test_bundle(&["a.example.test"]))
            .unwrap();
        let normal = resolver
            .select_certificate(Some("a.example.test"), false)
            .unwrap();
        resolver.set_challenge("a.example.test", [7; 32]).unwrap();

        let challenge = resolver
            .select_certificate(Some("a.example.test"), true)
            .unwrap();
        assert!(!Arc::ptr_eq(&normal, &challenge));
        let still_normal = resolver
            .select_certificate(Some("a.example.test"), false)
            .unwrap();
        assert!(Arc::ptr_eq(&normal, &still_normal));

        // A challenge on another name does not leak into this one.
        resolver.set_challenge("b.example.test", [8; 32]).unwrap();
        let other = resolver
            .select_certificate(Some("b.example.test"), true)
            .unwrap();
        assert!(!Arc::ptr_eq(&other, &challenge));
        assert!(
            resolver
                .select_certificate(Some("b.example.test"), false)
                .is_none()
        );
    }

    #[test]
    fn challenge_works_before_any_certificate_exists() {
        let resolver = CertResolver::new();
        resolver.set_challenge("a.example.test", [9; 32]).unwrap();
        assert!(
            resolver
                .select_certificate(Some("a.example.test"), true)
                .is_some()
        );
        assert!(
            resolver
                .select_certificate(Some("a.example.test"), false)
                .is_none()
        );
    }

    #[test]
    fn cleared_challenge_falls_back_to_the_instance_certificate() {
        let resolver = CertResolver::new();
        resolver
            .set_certificate(&test_bundle(&["a.example.test"]))
            .unwrap();
        let normal = resolver
            .select_certificate(Some("a.example.test"), false)
            .unwrap();
        resolver.set_challenge("a.example.test", [7; 32]).unwrap();
        resolver.clear_challenge("a.example.test");
        let after = resolver
            .select_certificate(Some("a.example.test"), true)
            .unwrap();
        assert!(Arc::ptr_eq(&normal, &after));
    }

    /// Verifier that accepts any certificate: these tests inspect what the server sent.
    #[derive(Debug)]
    struct AcceptAny;

    impl rustls::client::danger::ServerCertVerifier for AcceptAny {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    /// Handshake against `resolver` over an in-memory pipe and return the
    /// leaf certificate the server presented plus the negotiated ALPN protocol.
    async fn handshake(
        resolver: &Arc<CertResolver>,
        sni: &str,
        alpn: &[&[u8]],
    ) -> anyhow::Result<(Vec<u8>, Option<Vec<u8>>)> {
        let acceptor = TlsAcceptor::from(resolver.server_config()?);
        let mut client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny))
        .with_no_client_auth();
        client_config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let connector = TlsConnector::from(Arc::new(client_config));

        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            let mut tls = acceptor.accept(server_io).await?;
            tls.write_all(b"x").await?;
            tls.shutdown().await?;
            anyhow::Ok(())
        });
        let name = ServerName::try_from(sni.to_owned())?;
        let mut tls = connector.connect(name, client_io).await?;
        let mut byte = [0_u8; 1];
        let _ = tls.read(&mut byte).await?;
        let (_, session) = tls.get_ref();
        let leaf = session
            .peer_certificates()
            .and_then(|c| c.first())
            .context("server sent no certificate")?
            .as_ref()
            .to_vec();
        let negotiated = session.alpn_protocol().map(<[u8]>::to_vec);
        server.await??;
        Ok((leaf, negotiated))
    }

    #[tokio::test]
    async fn handshake_picks_challenge_certificate_only_for_acme_alpn() {
        let resolver = CertResolver::new();
        let bundle = test_bundle(&["a.example.test"]);
        resolver.set_certificate(&bundle).unwrap();
        resolver.set_challenge("a.example.test", [5; 32]).unwrap();

        let normal_leaf = CertificateDer::from_pem_slice(bundle.chain_pem.as_bytes()).unwrap();

        let (leaf, alpn) = handshake(&resolver, "a.example.test", &[b"h2", b"http/1.1"])
            .await
            .unwrap();
        assert_eq!(leaf, normal_leaf.as_ref());
        assert_eq!(alpn.as_deref(), Some(&b"h2"[..]));

        let (challenge_leaf, challenge_alpn) =
            handshake(&resolver, "a.example.test", &[b"acme-tls/1"])
                .await
                .unwrap();
        assert_ne!(challenge_leaf, normal_leaf.as_ref());
        assert_eq!(challenge_alpn.as_deref(), Some(&b"acme-tls/1"[..]));
    }

    #[tokio::test]
    async fn handshake_for_unknown_name_or_missing_certificate_fails() {
        let resolver = CertResolver::new();
        assert!(
            handshake(&resolver, "a.example.test", &[b"h2"])
                .await
                .is_err()
        );

        resolver
            .set_certificate(&test_bundle(&["a.example.test"]))
            .unwrap();
        assert!(
            handshake(&resolver, "evil.example.test", &[b"h2"])
                .await
                .is_err()
        );
        assert!(
            handshake(&resolver, "a.example.test", &[b"h2"])
                .await
                .is_ok()
        );
    }
}

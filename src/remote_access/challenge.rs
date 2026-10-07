//! TLS-ALPN-01 challenge certificates (RFC 8737).

use anyhow::Context as _;
use chrono::{Datelike as _, Duration, Utc};
use rcgen::{CertificateParams, CustomExtension, KeyPair, date_time_ymd};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

/// Length in bytes of the SHA-256 key authorization digest an ACME server expects in the certificate.
pub(crate) const KEY_AUTHORIZATION_DIGEST_LEN: usize = 32;

/// The ALPN protocol name a CA offers when it validates a TLS-ALPN-01 challenge.
pub(crate) const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

/// Build the self-signed certificate that answers a TLS-ALPN-01 challenge for
/// `name`: its only SAN is `name` and it carries the critical `acmeIdentifier`
/// extension holding `key_authorization_digest`.
///
/// Every call generates a fresh key. The certificate is valid for a few days
/// (rcgen takes whole days) because the CA validates it within seconds and
/// nothing else ever trusts it.
pub(crate) fn acme_identifier_certificate(
    name: &str,
    key_authorization_digest: [u8; KEY_AUTHORIZATION_DIGEST_LEN],
) -> anyhow::Result<CertifiedKey> {
    let mut params = CertificateParams::new(vec![name.to_owned()])
        .with_context(|| format!("invalid challenge certificate name {name}"))?;
    params
        .custom_extensions
        .push(CustomExtension::new_acme_identifier(
            &key_authorization_digest,
        ));
    set_validity(&mut params)?;

    let key_pair = KeyPair::generate().context("failed to generate challenge certificate key")?;
    let certificate = params
        .self_signed(&key_pair)
        .with_context(|| format!("failed to sign challenge certificate for {name}"))?;

    let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .context("challenge certificate key is not usable for TLS")?;
    Ok(CertifiedKey::new(
        vec![CertificateDer::from(certificate.der().to_vec())],
        signing_key,
    ))
}

/// rcgen only builds dates at day granularity, so the window is yesterday to the day after tomorrow.
fn set_validity(params: &mut CertificateParams) -> anyhow::Result<()> {
    let now = Utc::now();
    let day = |at: chrono::DateTime<Utc>| -> anyhow::Result<_> {
        let month = u8::try_from(at.month()).context("month out of range")?;
        let day = u8::try_from(at.day()).context("day out of range")?;
        Ok(date_time_ymd(at.year(), month, day))
    };
    params.not_before = day(now - Duration::days(1))?;
    params.not_after = day(now + Duration::days(2))?;
    Ok(())
}

/// DNS names in the certificate's subject alternative names, sorted.
#[cfg(test)]
pub(crate) fn san_dns_names(der: &[u8]) -> Vec<String> {
    use x509_parser::prelude::{FromDer as _, GeneralName, X509Certificate};
    let (_, parsed) = X509Certificate::from_der(der).unwrap();
    let mut names: Vec<String> = parsed
        .subject_alternative_name()
        .unwrap()
        .map(|extension| {
            extension
                .value
                .general_names
                .iter()
                .filter_map(|name| {
                    if let GeneralName::DNSName(dns) = name {
                        Some((*dns).to_owned())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::prelude::{FromDer as _, X509Certificate};

    const ACME_IDENTIFIER_OID: &str = "1.3.6.1.5.5.7.1.31";

    #[test]
    fn challenge_certificate_carries_critical_acme_identifier_with_digest() {
        let digest = [0xab_u8; 32];
        let certified = acme_identifier_certificate("host.example.test", digest).unwrap();
        let der = certified.cert.first().unwrap();
        let (_, parsed) = X509Certificate::from_der(der.as_ref()).unwrap();

        let ext = parsed
            .iter_extensions()
            .find(|e| e.oid.to_id_string() == ACME_IDENTIFIER_OID)
            .expect("acmeIdentifier extension present");
        assert!(
            ext.critical,
            "RFC 8737 requires the extension to be critical"
        );
        // extnValue is a DER OCTET STRING wrapping the 32-byte digest.
        assert_eq!(ext.value, [&[0x04, 0x20][..], &digest[..]].concat());
    }

    fn leaf_der(certified: &CertifiedKey) -> &[u8] {
        certified.cert.first().unwrap().as_ref()
    }

    #[test]
    fn challenge_certificate_names_only_the_challenged_host() {
        let certified = acme_identifier_certificate("host.example.test", [1; 32]).unwrap();
        assert_eq!(
            san_dns_names(leaf_der(&certified)),
            vec!["host.example.test".to_owned()]
        );
    }

    #[test]
    fn challenge_certificate_is_short_lived() {
        let certified = acme_identifier_certificate("host.example.test", [2; 32]).unwrap();
        let (_, parsed) = X509Certificate::from_der(leaf_der(&certified)).unwrap();
        let lifetime =
            parsed.validity().not_after.timestamp() - parsed.validity().not_before.timestamp();
        assert!(
            lifetime <= 4 * 24 * 3600,
            "lifetime {lifetime}s is too long"
        );
        assert!(parsed.validity().is_valid(), "valid right now");
    }

    #[test]
    fn every_call_uses_a_fresh_key() {
        let a = acme_identifier_certificate("host.example.test", [3; 32]).unwrap();
        let b = acme_identifier_certificate("host.example.test", [3; 32]).unwrap();
        assert_ne!(leaf_der(&a), leaf_der(&b));
    }
}

//! Random secrets, their hashes, and the codes people type.
//!
//! Every credential, token and request id is generated from the operating
//! system's random source and only ever stored as a SHA-256 hash, so the
//! pairing file holds nothing that can be replayed.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

use super::error::PairingError;

/// RFC 4648 base32, the alphabet of the codes people read and type.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Length of a recovery code, in base32 characters (80 bits).
pub(super) const RECOVERY_CODE_LEN: usize = 16;

/// Length of the code a pairing request shows, in base32 characters.
pub(super) const PAIRING_CODE_LEN: usize = 6;

/// `N` random bytes from the operating system.
fn random_bytes<const N: usize>() -> Result<[u8; N], PairingError> {
    let mut bytes = [0_u8; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_unspecified| {
            tracing::error!("the operating system's random source failed");
            PairingError::Random
        })?;
    Ok(bytes)
}

/// A 256-bit secret as URL-safe base64: device credentials and pairing tokens.
pub(super) fn secret_256() -> Result<String, PairingError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<32>()?))
}

/// A 128-bit secret as URL-safe base64: the id only a waiting browser knows.
pub(super) fn secret_128() -> Result<String, PairingError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<16>()?))
}

/// A short public identifier (64 bits as hex) for listing and revoking.
pub(super) fn public_id() -> Result<String, PairingError> {
    Ok(hex::encode(random_bytes::<8>()?))
}

/// Lowercase hex SHA-256 of `secret`.
pub(super) fn hash(secret: &str) -> String {
    hex::encode(digest(&SHA256, secret.as_bytes()).as_ref())
}

/// Whether two hashes are equal, in time that doesn't depend on where they differ.
pub(super) fn hashes_equal(a: &str, b: &str) -> bool {
    crate::util::secret_compare::secrets_match(a, b)
}

/// `len` random base32 characters.
pub(super) fn base32_code(len: usize) -> Result<String, PairingError> {
    let mut code = String::with_capacity(len);
    // Rejection sampling keeps every character equally likely: a byte maps to
    // one of 32 characters only when it is below the largest multiple of 32.
    while code.len() < len {
        for byte in random_bytes::<32>()? {
            if code.len() == len {
                break;
            }
            if byte < 224 {
                let index = usize::from(byte % 32);
                if let Some(&ch) = BASE32_ALPHABET.get(index) {
                    code.push(char::from(ch));
                }
            }
        }
    }
    Ok(code)
}

/// The form a typed recovery code is compared in: upper case, with the
/// spaces and dashes people add for reading removed.
pub(super) fn normalize_recovery_code(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A recovery code grouped for reading: `ABCD-EFGH-IJKL-MNOP`.
pub(super) fn group_recovery_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_url_safe_and_distinct() {
        let a = secret_256().unwrap();
        let b = secret_256().unwrap();
        assert_ne!(a, b, "two secrets must differ");
        assert_eq!(a.len(), 43, "256 bits is 43 base64 characters");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "secret must be URL-safe: {a}"
        );
        assert_eq!(secret_128().unwrap().len(), 22);
    }

    #[test]
    fn base32_codes_use_only_the_alphabet() {
        let code = base32_code(RECOVERY_CODE_LEN).unwrap();
        assert_eq!(code.len(), RECOVERY_CODE_LEN);
        assert!(
            code.bytes().all(|b| BASE32_ALPHABET.contains(&b)),
            "unexpected character in {code}"
        );
    }

    #[test]
    fn typed_recovery_codes_normalize_case_and_separators() {
        assert_eq!(
            normalize_recovery_code(" abcd-efgh ijkl-mnop "),
            "ABCDEFGHIJKLMNOP"
        );
    }

    #[test]
    fn recovery_codes_group_in_fours() {
        assert_eq!(
            group_recovery_code("ABCDEFGHIJKLMNOP"),
            "ABCD-EFGH-IJKL-MNOP"
        );
    }

    #[test]
    fn hash_is_stable_and_hex() {
        assert_eq!(
            hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(hashes_equal(&hash("x"), &hash("x")));
        assert!(!hashes_equal(&hash("x"), &hash("y")));
    }
}

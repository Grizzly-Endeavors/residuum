use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

use super::error::PairingError;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub(super) const RECOVERY_CODE_LEN: usize = 16;

pub(super) const PAIRING_CODE_LEN: usize = 6;

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

pub(super) fn secret_256() -> Result<String, PairingError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<32>()?))
}

pub(super) fn secret_128() -> Result<String, PairingError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<16>()?))
}

pub(super) fn public_id() -> Result<String, PairingError> {
    Ok(hex::encode(random_bytes::<8>()?))
}

pub(super) fn hash(secret: &str) -> String {
    hex::encode(digest(&SHA256, secret.as_bytes()).as_ref())
}

pub(super) fn hashes_equal(a: &str, b: &str) -> bool {
    residuum_core::util::secret_compare::secrets_match(a, b)
}

pub(super) fn base32_code(len: usize) -> Result<String, PairingError> {
    let mut code = String::with_capacity(len);
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

pub(super) fn normalize_recovery_code(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

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

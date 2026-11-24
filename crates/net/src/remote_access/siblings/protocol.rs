use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::keys::is_key_shaped;
use crate::remote_access::identity::is_valid_slug;
use crate::remote_access::jws::{JwsError, thumbprint};

pub(crate) const PATH_PREFIX: &str = "/_sibling/join";
pub(crate) const NONCE_PATH: &str = "/_sibling/join/nonce";
/// `POST`: the signed join request. `GET {PATH}/{join id}` polls the answer.
pub(crate) const REQUEST_PATH: &str = "/_sibling/join";

pub(crate) const LIFETIME_SECS: i64 = 600;

const MAX_DISPLAY_NAME: usize = 64;
const MAX_ACCOUNT_URI: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NonceReply {
    pub(crate) nonce: String,
    pub(crate) slug: String,
    pub(crate) account_uri: String,
    pub(crate) account_jwk: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JoinPayload {
    pub(crate) nonce: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) account_uri: String,
    pub(crate) key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubmitReply {
    pub(crate) join_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum PollReply {
    Pending,
    Approved {
        key: String,
        account_uri: String,
        account_jwk: Value,
    },
    Denied,
}

pub(crate) fn confirmation_code(
    host_jwk: &Value,
    requester_jwk: &Value,
    nonce: &str,
) -> Result<String, JwsError> {
    let mut input = Vec::with_capacity(64 + nonce.len());
    input.extend_from_slice(&thumbprint(host_jwk)?);
    input.extend_from_slice(&thumbprint(requester_jwk)?);
    input.extend_from_slice(nonce.as_bytes());
    let hash = digest(&SHA256, &input);
    let leading = hash
        .as_ref()
        .first_chunk::<4>()
        .map_or(0, |bytes| u32::from_be_bytes(*bytes));
    Ok(format!("{:06}", leading % 1_000_000))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PayloadError {
    #[error("the instance name isn't valid")]
    Slug,
    #[error("an instance can't join itself")]
    SelfJoin,
    #[error("the display name is empty, too long, or has control characters")]
    DisplayName,
    #[error("the certificate account address isn't valid")]
    AccountUri,
    #[error("the key isn't shaped like a sibling key")]
    Key,
}

impl JoinPayload {
    pub(crate) fn validate(&self, host_slug: &str) -> Result<(), PayloadError> {
        if !is_valid_slug(&self.slug) {
            return Err(PayloadError::Slug);
        }
        if self.slug == host_slug {
            return Err(PayloadError::SelfJoin);
        }
        let name = self.display_name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_DISPLAY_NAME
            || name.chars().any(char::is_control)
        {
            return Err(PayloadError::DisplayName);
        }
        if !is_account_uri(&self.account_uri) {
            return Err(PayloadError::AccountUri);
        }
        if !is_key_shaped(&self.key) {
            return Err(PayloadError::Key);
        }
        Ok(())
    }
}

pub(crate) fn is_account_uri(uri: &str) -> bool {
    uri.len() <= MAX_ACCOUNT_URI
        && uri.starts_with("https://")
        && uri.chars().all(|c| c.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_access::jws::AccountSigner;
    use crate::remote_access::jws::test_key::TestAccount;

    fn payload(slug: &str) -> JoinPayload {
        JoinPayload {
            nonce: "n".into(),
            slug: slug.into(),
            display_name: "Desk".into(),
            account_uri: "https://acme.test/acct/2".into(),
            key: super::super::keys::issue_key(),
        }
    }

    #[test]
    fn the_code_is_six_digits_and_depends_on_every_input() {
        let host = TestAccount::new("https://acme.test/acct/1");
        let requester = TestAccount::new("https://acme.test/acct/2");
        let other = TestAccount::new("https://acme.test/acct/3");
        let code = confirmation_code(&host.jwk(), &requester.jwk(), "nonce-1").unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()), "{code}");
        assert_eq!(
            confirmation_code(&host.jwk(), &requester.jwk(), "nonce-1").unwrap(),
            code,
            "both sides compute the same code"
        );
        assert_ne!(
            confirmation_code(&host.jwk(), &other.jwk(), "nonce-1").unwrap(),
            code
        );
        assert_ne!(
            confirmation_code(&host.jwk(), &requester.jwk(), "nonce-2").unwrap(),
            code
        );
        assert_ne!(
            confirmation_code(&other.jwk(), &requester.jwk(), "nonce-1").unwrap(),
            code
        );
    }

    #[test]
    fn the_code_has_a_fixed_known_answer() {
        let host = serde_json::json!({
            "kty": "EC", "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        });
        let requester = serde_json::json!({
            "kty": "EC", "crv": "P-256",
            "x": "MKBCTNIcKUSDii11ySs3526iDZ8AiTo7Tu6KPAqv7D4",
            "y": "4Etl6SRW2YiLUrN5vfvVHuhp7x8PxltmWWlbbM4IFyM",
        });
        let code = confirmation_code(&host, &requester, "abc").unwrap();
        assert_eq!(code, "894698");
    }

    #[test]
    fn payload_fields_are_checked() {
        assert_eq!(payload("desktop").validate("laptop"), Ok(()));
        assert_eq!(
            payload("laptop").validate("laptop"),
            Err(PayloadError::SelfJoin)
        );
        assert_eq!(
            payload("Bad Slug").validate("laptop"),
            Err(PayloadError::Slug)
        );
        let mut bad_name = payload("desktop");
        for name in ["  ".to_string(), "a\u{7}b".to_string(), "x".repeat(65)] {
            bad_name.display_name = name;
            assert_eq!(bad_name.validate("laptop"), Err(PayloadError::DisplayName));
        }
        let mut bad_uri = payload("desktop");
        bad_uri.account_uri = "http://acme.test/acct/2".into();
        assert_eq!(bad_uri.validate("laptop"), Err(PayloadError::AccountUri));
        let mut bad_key = payload("desktop");
        bad_key.key = "short".into();
        assert_eq!(bad_key.validate("laptop"), Err(PayloadError::Key));
    }
}

use anyhow::{Context as _, anyhow, bail};
use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use ring::aead;
use ring::agreement;
use ring::hmac;
use ring::rand::{SecureRandom as _, SystemRandom};

use super::types::WebPushSubscriptionKeys;

pub(super) const PUBLIC_KEY_LEN: usize = 65;

const AUTH_SECRET_LEN: usize = 16;

const SALT_LEN: usize = 16;

const TAG_LEN: usize = 16;

/// The `rs` field of the header: the most bytes one record holds. Push
/// services accept at least 4096 bytes of body, so one record of this size is
/// the most a message can carry.
const RECORD_SIZE: u32 = 4096;

/// Salt, record size, key id length, and the hub's public key.
const HEADER_LEN: usize = SALT_LEN + 4 + 1 + PUBLIC_KEY_LEN;

pub(super) const MAX_PLAINTEXT_LEN: usize = RECORD_SIZE as usize - HEADER_LEN - 1 - TAG_LEN;

/// Browsers give base64url without padding; padded input is accepted too.
const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub(super) fn base64url_encode(bytes: &[u8]) -> String {
    BASE64URL.encode(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Recipient {
    public_key: [u8; PUBLIC_KEY_LEN],
    auth_secret: [u8; AUTH_SECRET_LEN],
}

impl Recipient {
    pub(super) fn parse(keys: &WebPushSubscriptionKeys) -> Result<Self, String> {
        let public_key = BASE64URL
            .decode(&keys.p256dh)
            .ok()
            .and_then(|bytes| <[u8; PUBLIC_KEY_LEN]>::try_from(bytes).ok())
            .filter(|key| key.first() == Some(&0x04))
            .ok_or_else(|| {
                "the subscription's p256dh key isn't a base64url P-256 public key (65 bytes starting with 0x04)"
                    .to_string()
            })?;
        let auth_secret = BASE64URL
            .decode(&keys.auth)
            .ok()
            .and_then(|bytes| <[u8; AUTH_SECRET_LEN]>::try_from(bytes).ok())
            .ok_or_else(|| {
                "the subscription's auth secret isn't 16 base64url-encoded bytes".to_string()
            })?;
        Ok(Self {
            public_key,
            auth_secret,
        })
    }
}

pub(super) fn encrypt(recipient: &Recipient, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    if plaintext.len() > MAX_PLAINTEXT_LEN {
        bail!(
            "the notification is {} bytes, more than the {MAX_PLAINTEXT_LEN} one push message holds",
            plaintext.len()
        );
    }
    let rng = SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_unspecified| anyhow!("couldn't generate an encryption key"))?;
    let as_public = <[u8; PUBLIC_KEY_LEN]>::try_from(
        private
            .compute_public_key()
            .map_err(|_unspecified| anyhow!("couldn't compute the encryption public key"))?
            .as_ref(),
    )
    .context("the encryption public key has an unexpected length")?;
    let mut salt = [0_u8; SALT_LEN];
    rng.fill(&mut salt)
        .map_err(|_unspecified| anyhow!("couldn't generate a random salt"))?;

    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &recipient.public_key);
    agreement::agree_ephemeral(private, &peer, |ecdh_secret| {
        seal(ecdh_secret, recipient, &as_public, &salt, plaintext)
    })
    .map_err(|_unspecified| {
        anyhow!("the subscription's p256dh key isn't a valid P-256 public key")
    })?
}

fn seal(
    ecdh_secret: &[u8],
    recipient: &Recipient,
    as_public: &[u8; PUBLIC_KEY_LEN],
    salt: &[u8; SALT_LEN],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let keys = derive_keys(ecdh_secret, recipient, as_public, salt);

    let mut record = Vec::with_capacity(plaintext.len() + 1 + TAG_LEN);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, &keys.content_key)
            .map_err(|_unspecified| anyhow!("couldn't prepare the content encryption key"))?,
    );
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(keys.nonce),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_unspecified| anyhow!("couldn't encrypt the notification"))?;

    let key_id_len = u8::try_from(as_public.len()).context("the public key id is too long")?;
    let mut body = Vec::with_capacity(HEADER_LEN + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(key_id_len);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

struct ContentKeys {
    content_key: [u8; 16],
    nonce: [u8; 12],
}

fn derive_keys(
    ecdh_secret: &[u8],
    recipient: &Recipient,
    as_public: &[u8; PUBLIC_KEY_LEN],
    salt: &[u8; SALT_LEN],
) -> ContentKeys {
    let prk_key = hmac_sha256(&recipient.auth_secret, ecdh_secret);
    let mut key_info = Vec::with_capacity(14 + 2 * PUBLIC_KEY_LEN + 1);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(&recipient.public_key);
    key_info.extend_from_slice(as_public);
    key_info.push(1);
    let ikm = hmac_sha256(&prk_key, &key_info);

    let prk = hmac_sha256(salt, &ikm);
    let content_key = hmac_sha256(&prk, b"Content-Encoding: aes128gcm\0\x01");
    let nonce = hmac_sha256(&prk, b"Content-Encoding: nonce\0\x01");
    ContentKeys {
        content_key: prefix(&content_key),
        nonce: prefix(&nonce),
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    prefix(hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data).as_ref())
}

fn prefix<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut out = [0_u8; N];
    for (dst, src) in out.iter_mut().zip(bytes) {
        *dst = *src;
    }
    out
}

#[cfg(any(test, feature = "test-support"))]
pub(super) mod browser {
    use ring::aead;
    use ring::agreement;
    use ring::rand::{SecureRandom as _, SystemRandom};

    use super::{
        AUTH_SECRET_LEN, PUBLIC_KEY_LEN, RECORD_SIZE, Recipient, SALT_LEN, base64url_encode,
        derive_keys,
    };
    use crate::push::types::WebPushSubscriptionKeys;

    pub struct Browser {
        private: agreement::EphemeralPrivateKey,
        public_key: [u8; PUBLIC_KEY_LEN],
        auth_secret: [u8; AUTH_SECRET_LEN],
    }

    impl Default for Browser {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Browser {
        #[expect(
            clippy::unwrap_used,
            reason = "test-support helper: a key pair that cannot be made fails the test that asked for it"
        )]
        #[must_use]
        pub fn new() -> Self {
            let rng = SystemRandom::new();
            let private =
                agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
            let public_key =
                <[u8; PUBLIC_KEY_LEN]>::try_from(private.compute_public_key().unwrap().as_ref())
                    .unwrap();
            let mut auth_secret = [0_u8; AUTH_SECRET_LEN];
            rng.fill(&mut auth_secret).unwrap();
            Self {
                private,
                public_key,
                auth_secret,
            }
        }

        #[must_use]
        pub fn keys(&self) -> WebPushSubscriptionKeys {
            WebPushSubscriptionKeys {
                p256dh: base64url_encode(&self.public_key),
                auth: base64url_encode(&self.auth_secret),
            }
        }

        pub(super) fn recipient(&self) -> Recipient {
            Recipient {
                public_key: self.public_key,
                auth_secret: self.auth_secret,
            }
        }

        #[expect(
            clippy::unwrap_used,
            reason = "test-support helper: a message that does not open fails the test that sent it"
        )]
        #[must_use]
        pub fn decrypt(self, body: &[u8]) -> Vec<u8> {
            let recipient = self.recipient();
            let (salt, rest) = body.split_first_chunk::<SALT_LEN>().unwrap();
            let (record_size, rest) = rest.split_first_chunk::<4>().unwrap();
            assert_eq!(
                u32::from_be_bytes(*record_size),
                RECORD_SIZE,
                "the header names the record size the hub seals with"
            );
            let (key_id_len, rest) = rest.split_first().unwrap();
            assert_eq!(
                usize::from(*key_id_len),
                PUBLIC_KEY_LEN,
                "the key id is the hub's public key"
            );
            let (as_public, ciphertext) = rest.split_first_chunk::<PUBLIC_KEY_LEN>().unwrap();

            let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, as_public);
            let keys = agreement::agree_ephemeral(self.private, &peer, |secret| {
                derive_keys(secret, &recipient, as_public, salt)
            })
            .unwrap();

            let key = aead::LessSafeKey::new(
                aead::UnboundKey::new(&aead::AES_128_GCM, &keys.content_key).unwrap(),
            );
            let mut record = ciphertext.to_vec();
            let opened = key
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(keys.nonce),
                    aead::Aad::empty(),
                    &mut record,
                )
                .unwrap();
            let (delimiter, plaintext) = opened.split_last().unwrap();
            assert_eq!(
                *delimiter, 0x02,
                "the record ends with the last-record delimiter"
            );
            plaintext.to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(text: &str) -> Vec<u8> {
        BASE64URL.decode(text.replace([' ', '\n'], "")).unwrap()
    }

    fn array<const N: usize>(text: &str) -> [u8; N] {
        <[u8; N]>::try_from(b64(text)).unwrap()
    }

    struct Rfc8291 {
        plaintext: Vec<u8>,
        as_public: [u8; PUBLIC_KEY_LEN],
        ua_public: [u8; PUBLIC_KEY_LEN],
        auth_secret: [u8; AUTH_SECRET_LEN],
        salt: [u8; SALT_LEN],
        ecdh_secret: Vec<u8>,
    }

    impl Rfc8291 {
        fn recipient(&self) -> Recipient {
            Recipient {
                public_key: self.ua_public,
                auth_secret: self.auth_secret,
            }
        }
    }

    fn rfc8291() -> Rfc8291 {
        Rfc8291 {
            plaintext: b"When I grow up, I want to be a watermelon".to_vec(),
            as_public: array(
                "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
            ),
            ua_public: array(
                "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
            ),
            auth_secret: array("BTBZMqHH6r4Tts7J_aSIgg"),
            salt: array("DGv6ra1nlYgDCS1FRnbzlw"),
            ecdh_secret: b64("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"),
        }
    }

    #[test]
    fn derived_keys_match_the_rfc_8291_appendix() {
        let vector = rfc8291();
        let keys = derive_keys(
            &vector.ecdh_secret,
            &vector.recipient(),
            &vector.as_public,
            &vector.salt,
        );
        assert_eq!(keys.content_key.to_vec(), b64("oIhVW04MRdy2XN9CiKLxTg"));
        assert_eq!(keys.nonce.to_vec(), b64("4h_95klXJ5E_qnoN"));
    }

    #[test]
    fn sealing_reproduces_the_rfc_8291_message_byte_for_byte() {
        let vector = rfc8291();
        let body = seal(
            &vector.ecdh_secret,
            &vector.recipient(),
            &vector.as_public,
            &vector.salt,
            &vector.plaintext,
        )
        .unwrap();
        let expected = b64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        assert_eq!(expected.len(), 86 + 41 + 1 + 16);
        assert_eq!(body, expected);
    }

    #[test]
    fn the_largest_plaintext_fills_one_record_of_4096_bytes() {
        assert_eq!(MAX_PLAINTEXT_LEN, 3993);
        let vector = rfc8291();
        let body = seal(
            &vector.ecdh_secret,
            &vector.recipient(),
            &vector.as_public,
            &vector.salt,
            &vec![b'x'; MAX_PLAINTEXT_LEN],
        )
        .unwrap();
        assert_eq!(body.len(), 4096);
    }

    #[test]
    fn a_message_encrypted_to_a_browser_decrypts_in_the_browser() {
        let browser = browser::Browser::new();
        let body = encrypt(&browser.recipient(), b"{\"hello\":\"world\"}").unwrap();
        assert_eq!(browser.decrypt(&body), b"{\"hello\":\"world\"}");
    }

    #[test]
    fn every_message_uses_a_fresh_key_and_salt() {
        let device = browser::Browser::new().recipient();
        let first = encrypt(&device, b"same").unwrap();
        let second = encrypt(&device, b"same").unwrap();
        assert_ne!(first, second);
        let (salt_a, _) = first.split_first_chunk::<SALT_LEN>().unwrap();
        let (salt_b, _) = second.split_first_chunk::<SALT_LEN>().unwrap();
        assert_ne!(salt_a, salt_b);
    }

    #[test]
    fn a_message_too_long_for_one_record_is_refused() {
        let device = browser::Browser::new().recipient();
        let err = encrypt(&device, &vec![b'x'; MAX_PLAINTEXT_LEN + 1]).unwrap_err();
        assert!(err.to_string().contains("3993"), "{err}");
    }

    #[test]
    fn a_public_key_that_is_not_on_the_curve_is_refused() {
        let mut device = browser::Browser::new().recipient();
        device.public_key = [0xFF; PUBLIC_KEY_LEN];
        if let Some(format_byte) = device.public_key.first_mut() {
            *format_byte = 0x04;
        }
        let err = encrypt(&device, b"x").unwrap_err();
        assert!(err.to_string().contains("p256dh"), "{err}");
    }

    #[test]
    fn subscription_keys_are_checked_for_shape() {
        let good = WebPushSubscriptionKeys {
            p256dh: base64url_encode(&rfc8291().ua_public),
            auth: base64url_encode(&rfc8291().auth_secret),
        };
        assert!(Recipient::parse(&good).is_ok());

        let padded = WebPushSubscriptionKeys {
            auth: format!("{}==", good.auth),
            ..good.clone()
        };
        assert!(Recipient::parse(&padded).is_ok(), "padding is tolerated");

        let short_key = WebPushSubscriptionKeys {
            p256dh: base64url_encode(&[4_u8; 10]),
            ..good.clone()
        };
        assert!(Recipient::parse(&short_key).unwrap_err().contains("p256dh"));

        let compressed = WebPushSubscriptionKeys {
            p256dh: base64url_encode(&[2_u8; PUBLIC_KEY_LEN]),
            ..good.clone()
        };
        assert!(
            Recipient::parse(&compressed)
                .unwrap_err()
                .contains("p256dh")
        );

        let short_auth = WebPushSubscriptionKeys {
            auth: base64url_encode(&[1_u8; 8]),
            ..good.clone()
        };
        assert!(Recipient::parse(&short_auth).unwrap_err().contains("auth"));

        let not_base64 = WebPushSubscriptionKeys {
            p256dh: "!!!".to_string(),
            ..good
        };
        assert!(Recipient::parse(&not_base64).is_err());
    }
}

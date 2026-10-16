//! Timing-safe comparison of presented vs. expected secret material.

use ring::digest::{SHA256, digest};

/// Compare a presented secret with the expected one without leaking, through
/// response timing, how much of it was right: only fixed-length digests are
/// compared, so an early mismatch reveals nothing about the secret itself.
///
/// Used for webhook bearer tokens and A2A caller keys.
#[must_use]
pub fn secrets_match(provided: &str, expected: &str) -> bool {
    digest(&SHA256, provided.as_bytes()).as_ref() == digest(&SHA256, expected.as_bytes()).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_on_exact_equality() {
        assert!(secrets_match("s3cret", "s3cret"));
        assert!(!secrets_match("s3cre", "s3cret"));
        assert!(!secrets_match("", "s3cret"));
        assert!(!secrets_match("S3CRET", "s3cret"));
    }
}

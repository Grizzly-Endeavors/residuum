use ring::digest::{SHA256, digest};

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

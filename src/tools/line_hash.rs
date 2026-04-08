//! Line content hashing for staleness detection.
//!
//! Produces a 2-character hex hash for each line of text. Used by `ReadTool`
//! to tag output lines and by `EditTool` to validate that lines haven't changed.

/// Compute FNV-1a hash of a byte slice.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Compute a 2-character hex hash for a line of text.
///
/// Returns the lower byte of the FNV-1a hash formatted as 2 hex digits
/// (e.g. `"f1"`, `"a3"`, `"0e"`).
#[must_use]
pub fn line_hash(content: &str) -> String {
    let lower_byte = (fnv1a(content.as_bytes()) & 0xFF) as u8;
    format!("{lower_byte:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_input() {
        let hash1 = line_hash("fn main() {");
        let hash2 = line_hash("fn main() {");
        assert_eq!(hash1, hash2, "same input should produce same hash");
    }

    /// Compute the expected FNV-1a lower-byte hash directly, mirroring the production algorithm.
    fn expected_hash(s: &str) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in s.as_bytes() {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{:02x}", (hash & 0xFF) as u8)
    }

    #[test]
    fn different_inputs_differ() {
        let h1 = line_hash("fn main() {");
        let h2 = line_hash("fn foo() {");
        let h3 = line_hash("let x = 42;");
        assert_eq!(
            h1,
            expected_hash("fn main() {"),
            "hash must match reference for 'fn main() {{'"
        );
        assert_eq!(
            h2,
            expected_hash("fn foo() {"),
            "hash must match reference for 'fn foo() {{'"
        );
        assert_eq!(
            h3,
            expected_hash("let x = 42;"),
            "hash must match reference for 'let x = 42;'"
        );
        assert_ne!(
            h1, h2,
            "\"fn main() {{\" and \"fn foo() {{\" should have different hashes"
        );
        assert_ne!(
            h1, h3,
            "\"fn main() {{\" and \"let x = 42;\" should have different hashes"
        );
        assert_ne!(
            h2, h3,
            "\"fn foo() {{\" and \"let x = 42;\" should have different hashes"
        );
    }

    #[test]
    fn empty_string_consistent() {
        let h1 = line_hash("");
        let h2 = line_hash("");
        assert_eq!(h1, h2, "empty string hash should be consistent");
        assert_eq!(h1.len(), 2, "hash should always be 2 characters");
    }

    #[test]
    fn hash_is_two_hex_chars() {
        let hash = line_hash("hello world");
        assert_eq!(hash.len(), 2, "hash should be exactly 2 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be valid hex"
        );
    }
}

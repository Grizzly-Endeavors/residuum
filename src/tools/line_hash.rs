//! Line content hashing for staleness detection.
//!
//! Produces a 4-character hex hash for each line of text. Used by `ReadTool`
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

/// Compute a 4-character hex hash for a line of text.
///
/// Returns the lower 16 bits of the FNV-1a hash formatted as 4 hex digits
/// (e.g. `"f1a3"`, `"00e2"`). Widened from a single byte (256 buckets) to
/// 16 bits (65,536 buckets) because `EditTool` relies on this as its sole
/// staleness check — a narrower hash collides often enough on common short
/// lines (`}`, blank lines, `);`) to silently defeat that check.
#[must_use]
pub fn line_hash(content: &str) -> String {
    let lower_16_bits = (fnv1a(content.as_bytes()) & 0xFFFF) as u16;
    format!("{lower_16_bits:04x}")
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

    #[test]
    fn different_inputs_differ() {
        let h1 = line_hash("fn main() {");
        let h2 = line_hash("fn foo() {");
        let h3 = line_hash("let x = 42;");
        assert_eq!(h1, "1d48", "hash must match reference for 'fn main() {{'");
        assert_eq!(h2, "b1d3", "hash must match reference for 'fn foo() {{'");
        assert_eq!(h3, "d8aa", "hash must match reference for 'let x = 42;'");
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
        assert_eq!(h1.len(), 4, "hash should always be 4 characters");
    }

    #[test]
    fn hash_is_four_hex_chars() {
        let hash = line_hash("hello world");
        assert_eq!(hash.len(), 4, "hash should be exactly 4 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should be valid hex"
        );
    }
}

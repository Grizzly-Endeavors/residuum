//! RFC 3986 percent-encoding.

use std::fmt::Write as _;

/// `bytes` with every byte outside the RFC 3986 unreserved set
/// (`A-Z a-z 0-9 - . _ ~`) written as `%XX`, so the result is safe as one
/// URL path segment or query value.
#[must_use]
pub fn percent_encode_unreserved(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_bytes_pass_through() {
        assert_eq!(percent_encode_unreserved(b"AZaz09-._~"), "AZaz09-._~");
    }

    #[test]
    fn every_other_byte_is_escaped_in_upper_case_hex() {
        assert_eq!(
            percent_encode_unreserved("a/b c+é".as_bytes()),
            "a%2Fb%20c%2B%C3%A9"
        );
        assert_eq!(percent_encode_unreserved(&[0x00, 0xFF]), "%00%FF");
    }
}

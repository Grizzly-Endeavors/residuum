//! Replaces agent-key values in text with `[agent-key:<name>]` markers.

use std::borrow::Cow;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};

/// Substring redactor over every agent-key value and its common encodings.
///
/// Covers the raw value plus its standard and URL-safe base64 forms (padded
/// and unpadded) and its percent-encoded form, since those are the shapes a
/// credential most often takes in command output (HTTP headers, URLs, JSON).
/// Needles are matched longest first so a longer encoding is never
/// partially clobbered by a shorter one.
#[derive(Default, Clone)]
pub struct Redactor {
    /// `(needle, replacement)`, sorted by needle length, longest first.
    patterns: Vec<(String, String)>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("patterns", &self.patterns.len())
            .finish()
    }
}

impl Redactor {
    /// Build a redactor over `(name, value)` pairs.
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut patterns: Vec<(String, String)> = Vec::new();
        for (name, value) in entries {
            if value.is_empty() {
                continue;
            }
            let marker = marker_for(name);
            for needle in encodings(value) {
                if !patterns.iter().any(|(existing, _)| *existing == needle) {
                    patterns.push((needle, marker.clone()));
                }
            }
        }
        patterns.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
        Self { patterns }
    }

    /// A copy of this redactor that also covers one more `(name, value)`.
    #[must_use]
    pub fn with_entry(&self, name: &str, value: &str) -> Self {
        let mut combined = self.clone();
        let extra = Self::from_entries([(name, value)]);
        for pattern in extra.patterns {
            if !combined.patterns.iter().any(|(n, _)| *n == pattern.0) {
                combined.patterns.push(pattern);
            }
        }
        combined
            .patterns
            .sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));
        combined
    }

    /// Whether there is nothing to redact.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// `text` with every known value replaced by its marker. Borrows when
    /// nothing matched.
    #[must_use]
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out: Cow<'a, str> = Cow::Borrowed(text);
        for (needle, marker) in &self.patterns {
            if out.contains(needle.as_str()) {
                out = Cow::Owned(out.replace(needle.as_str(), marker));
            }
        }
        out
    }

    /// Redact a string in place, returning whether anything changed.
    pub fn redact_in_place(&self, text: &mut String) -> bool {
        if let Cow::Owned(redacted) = self.redact(text) {
            *text = redacted;
            true
        } else {
            false
        }
    }
}

/// The marker a key's value is replaced with.
#[must_use]
pub fn marker_for(name: &str) -> String {
    format!("[agent-key:{name}]")
}

/// The raw value and each distinct encoding of it worth matching.
fn encodings(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut out = vec![value.to_string()];
    for encoded in [
        STANDARD.encode(bytes),
        STANDARD_NO_PAD.encode(bytes),
        URL_SAFE.encode(bytes),
        URL_SAFE_NO_PAD.encode(bytes),
        percent_encode(bytes),
    ] {
        if !out.contains(&encoded) {
            out.push(encoded);
        }
    }
    out
}

/// RFC 3986 percent-encoding: every byte outside the unreserved set becomes
/// `%XX`.
fn percent_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_raw_value() {
        let r = Redactor::from_entries([("gh", "ghp_secretvalue123")]);
        assert_eq!(
            r.redact("token=ghp_secretvalue123 ok"),
            "token=[agent-key:gh] ok",
            "raw value should be replaced with the key's marker"
        );
    }

    #[test]
    fn redacts_base64_forms() {
        let value = "ab?cd>ef~gh";
        let r = Redactor::from_entries([("k", value)]);
        for encoded in [
            STANDARD.encode(value),
            URL_SAFE_NO_PAD.encode(value),
            STANDARD_NO_PAD.encode(value),
        ] {
            let text = format!("Authorization: Basic {encoded}");
            assert_eq!(
                r.redact(&text),
                "Authorization: Basic [agent-key:k]",
                "encoded form {encoded} should be redacted"
            );
        }
    }

    #[test]
    fn redacts_percent_encoded_form() {
        let r = Redactor::from_entries([("k", "p@ss word/123")]);
        assert_eq!(
            r.redact("https://x?pw=p%40ss%20word%2F123"),
            "https://x?pw=[agent-key:k]",
            "percent-encoded form should be redacted"
        );
    }

    #[test]
    fn untouched_text_is_borrowed() {
        let r = Redactor::from_entries([("k", "value-12345678")]);
        assert!(
            matches!(r.redact("nothing here"), Cow::Borrowed(_)),
            "no match should not allocate"
        );
    }

    #[test]
    fn longer_value_wins_over_contained_shorter_value() {
        let r = Redactor::from_entries([("short", "abcdefgh"), ("long", "abcdefgh-ijklmnop")]);
        assert_eq!(
            r.redact("x abcdefgh-ijklmnop y abcdefgh"),
            "x [agent-key:long] y [agent-key:short]",
            "the longer value should be matched before the shorter one it contains"
        );
    }

    #[test]
    fn with_entry_adds_a_value() {
        let r = Redactor::default().with_entry("new", "fresh-token-value");
        assert!(!r.is_empty(), "with_entry should add patterns");
        assert_eq!(
            r.redact("fresh-token-value"),
            "[agent-key:new]",
            "added value should be redacted"
        );
    }

    #[test]
    fn redact_in_place_reports_change() {
        let r = Redactor::from_entries([("k", "value-12345678")]);
        let mut s = "a value-12345678 b".to_string();
        assert!(r.redact_in_place(&mut s), "should report a change");
        assert_eq!(s, "a [agent-key:k] b", "should rewrite in place");
        let mut clean = "clean".to_string();
        assert!(!r.redact_in_place(&mut clean), "should report no change");
    }
}

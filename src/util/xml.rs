//! Minimal XML-escaping for author-controlled text embedded in pseudo-XML system prompts.

/// Escape `&`, `<`, and `>` so untrusted text (e.g. names/descriptions from
/// preset or skill frontmatter) cannot break out of the surrounding pseudo-XML
/// block when interpolated into a system prompt.
///
/// `&` is replaced first so it doesn't double-escape the entities produced by
/// the `<`/`>` replacements.
#[must_use]
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn escapes_special_chars() {
        let input = "Handles <tags> & \"quotes\"";
        let output = xml_escape(input);
        assert!(output.contains("&lt;tags&gt;"), "< and > should be escaped");
        assert!(output.contains("&amp;"), "& should be escaped");
        assert!(
            !output.contains("<tags>"),
            "raw < should not appear in output"
        );
    }

    #[test]
    fn leaves_plain_text_untouched() {
        assert_eq!(xml_escape("plain description"), "plain description");
    }
}

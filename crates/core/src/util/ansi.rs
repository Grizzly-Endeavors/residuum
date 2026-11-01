//! Removal of ANSI terminal escape sequences from a byte stream read in pieces.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const NEWLINE: u8 = b'\n';

/// Strips escape sequences (CSI, OSC and the two-byte and charset forms) from
/// output read in chunks.
///
/// The state carries across [`AnsiStripper::strip_into`] calls, so a sequence
/// split between two reads is removed whole. An OSC string that never ends is
/// closed by the next newline rather than swallowing the rest of the output.
#[derive(Debug, Default)]
pub(crate) struct AnsiStripper {
    state: State,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Text,
    /// After `ESC`, before the byte that says what kind of sequence it opens.
    Escape,
    /// Intermediate bytes of a sequence without a `[` or `]` introducer, such as
    /// the charset designation `ESC ( B`.
    EscapeIntermediate,
    /// Inside a CSI sequence (`ESC [`), before its final byte.
    Csi,
    /// Inside an OSC string (`ESC ]`), before its terminator.
    Osc,
    /// Inside an OSC string, after an `ESC` that may start the `ESC \` terminator.
    OscEscape,
}

impl AnsiStripper {
    /// Append `input` to `out` with any escape sequence removed.
    pub(crate) fn strip_into(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &byte in input {
            let (next, emit) = match self.state {
                State::Text if byte == ESC => (State::Escape, false),
                State::Text => (State::Text, true),
                State::Escape => match byte {
                    b'[' => (State::Csi, false),
                    b']' => (State::Osc, false),
                    0x20..=0x2F => (State::EscapeIntermediate, false),
                    // A two-byte sequence such as `ESC 7` or `ESC M`.
                    0x30..=0x7E => (State::Text, false),
                    ESC => (State::Escape, false),
                    _ => (State::Text, true),
                },
                State::EscapeIntermediate => match byte {
                    0x20..=0x2F => (State::EscapeIntermediate, false),
                    0x30..=0x7E => (State::Text, false),
                    ESC => (State::Escape, false),
                    _ => (State::Text, true),
                },
                State::Csi => match byte {
                    0x40..=0x7E => (State::Text, false),
                    0x20..=0x3F => (State::Csi, false),
                    ESC => (State::Escape, false),
                    _ => (State::Text, true),
                },
                State::Osc => match byte {
                    BEL => (State::Text, false),
                    ESC => (State::OscEscape, false),
                    NEWLINE => (State::Text, true),
                    _ => (State::Osc, false),
                },
                State::OscEscape => match byte {
                    b'\\' => (State::Text, false),
                    ESC => (State::OscEscape, false),
                    NEWLINE => (State::Text, true),
                    _ => (State::Osc, false),
                },
            };
            if emit {
                out.push(byte);
            }
            self.state = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> String {
        let mut stripper = AnsiStripper::default();
        let mut out = Vec::new();
        for chunk in chunks {
            stripper.strip_into(chunk, &mut out);
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let text = "hello\nw\u{f6}rld \u{2713}\tok";
        assert_eq!(strip(&[text.as_bytes()]), text);
    }

    #[test]
    fn colour_codes_are_removed() {
        assert_eq!(strip(&[b"\x1b[1;31merror\x1b[0m: bad"]), "error: bad");
    }

    #[test]
    fn private_mode_csi_is_removed() {
        assert_eq!(strip(&[b"a\x1b[?25lb\x1b[?25hc"]), "abc");
    }

    #[test]
    fn osc_terminated_by_bel_is_removed() {
        assert_eq!(strip(&[b"\x1b]0;window title\x07ok"]), "ok");
    }

    #[test]
    fn osc_terminated_by_string_terminator_is_removed() {
        assert_eq!(
            strip(&[b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"]),
            "link"
        );
    }

    #[test]
    fn two_byte_and_charset_sequences_are_removed() {
        assert_eq!(strip(&[b"\x1b7saved\x1b8"]), "saved");
        assert_eq!(strip(&[b"\x1b(Bhi"]), "hi");
    }

    #[test]
    fn sequence_split_across_reads_is_removed_whole() {
        assert_eq!(strip(&[b"x\x1b[3", b"1mred\x1b", b"[0my"]), "xredy");
    }

    #[test]
    fn unterminated_osc_ends_at_the_newline() {
        assert_eq!(strip(&[b"\x1b]0;title\nnext"]), "\nnext");
    }

    #[test]
    fn escape_followed_by_escape_starts_a_new_sequence() {
        assert_eq!(strip(&[b"\x1b\x1b[1mX"]), "X");
    }

    #[test]
    fn csi_aborted_by_a_newline_keeps_the_newline() {
        assert_eq!(strip(&[b"\x1b[31\nnext"]), "\nnext");
    }
}

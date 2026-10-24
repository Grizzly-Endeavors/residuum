//! Markdown → Slack mrkdwn conversion for outbound messages.
//!
//! Agent replies arrive as markdown; Slack renders its own `mrkdwn` dialect
//! (`*bold*`, `_italic_`). [`markdown_to_mrkdwn`] is a deliberately small,
//! line-oriented state machine over that gap — not a markdown parser:
//! anything it does not recognize passes through untouched, so unknown
//! constructs degrade to plain text rather than being mangled.
//!
//! Rules (design doc, Outbound §format):
//!
//! * fenced ```` ``` ```` blocks pass through **untouched**, ```` ** ```` and
//!   all — Slack already renders fenced blocks as preformatted code.
//! * `**bold**` → `*bold*`, `*italic*` → `_italic_` (one single pass, so the
//!   asterisks a converted `*bold*` introduces can never be re-read as the
//!   opening of an italic pair).
//! * Inline `` `code` `` spans pass through untouched, contents included.
//! * Line-leading `#`…`######` headers → `*Header text*` (hashes stripped,
//!   remainder trimmed).
//! * Markdown tables (a run of lines starting with `|`) are wrapped in a
//!   ```` ``` ```` fence — Slack has no table rendering, so raw pipes beat
//!   a puddle of wrapped text.
//! * Everything else — including emphasis whose content starts or ends with
//!   whitespace (`5 * 3 * 2` stays arithmetic, not italics) and unmatched
//!   single asterisks — passes through as-is.

/// Convert a markdown message to Slack mrkdwn. See the module docs for the
/// rule set; unrecognized constructs pass through unchanged.
#[must_use]
pub(crate) fn markdown_to_mrkdwn(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    // Buffered run of table rows, emitted fence-wrapped when the run ends.
    let mut table: Vec<&str> = Vec::new();

    for line in md.split('\n') {
        if in_fence {
            push_line(&mut out, line);
            if is_fence_delimiter(line) {
                in_fence = false;
            }
        } else if is_fence_delimiter(line) {
            flush_table(&mut out, &mut table);
            in_fence = true;
            push_line(&mut out, line);
        } else if is_table_row(line) {
            // Held back: the whole run is wrapped in one fence when it ends.
            table.push(line);
        } else {
            flush_table(&mut out, &mut table);
            push_line(&mut out, &convert_line(line));
        }
    }
    flush_table(&mut out, &mut table);

    // Every emitted line carries one '\n'; drop only the final one so a
    // trailing newline in the input survives conversion.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Append `line` plus its newline to `out`.
fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

/// A line that opens or closes a fenced block.
fn is_fence_delimiter(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// A line of a markdown table: pipe-first, like every row of one.
fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Emit the buffered table rows wrapped in a fence; nothing when empty.
fn flush_table(out: &mut String, table: &mut Vec<&str>) {
    if table.is_empty() {
        return;
    }
    out.push_str("```\n");
    for row in table.drain(..) {
        push_line(out, row);
    }
    out.push_str("```\n");
}

/// Convert one non-fenced, non-table line: headers, else inline emphasis.
fn convert_line(line: &str) -> String {
    match header_text(line) {
        Some(converted) => converted,
        None => convert_inline(line),
    }
}

/// `## Header` → `*Header*`, or `None` when the line is not a header.
fn header_text(line: &str) -> Option<String> {
    let rest = line.trim_start();
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = rest.split_at(hashes).1;
    // CommonMark: a header's hashes are followed by a space (or the line
    // end). `#hashtag` is not a header and must pass through untouched.
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim();
    if text.is_empty() {
        return None;
    }
    Some(format!("*{}*", convert_inline(text)))
}

/// Convert emphasis in one line in a single pass: `` `code` `` spans copy
/// verbatim, `**bold**` → `*bold*`, `*italic*` → `_italic_`, and everything
/// unmatched or whitespace-edged passes through as-is.
///
/// One cursor advances over the line, and each recognized construct consumes
/// exactly its own span — so the asterisks a converted `*bold*` emits can
/// never be re-read as the opener of an italic pair.
fn convert_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(first) = rest.chars().next() {
        rest = if first == '`' {
            copy_code_span(&mut out, rest)
        } else if first == '*' && rest.starts_with("**") {
            convert_emphasis(&mut out, rest, "**", "**", '*', '*')
        } else if first == '*' {
            convert_emphasis(&mut out, rest, "*", "*", '_', '_')
        } else {
            out.push(first);
            rest.split_at(first.len_utf8()).1
        };
    }
    out
}

/// Consume one inline code span at the start of `rest` (which begins with a
/// backtick), copying it verbatim — backticks and contents both, so
/// `**x**` inside inline code stays `**x**`. A lone backtick emits itself
/// and leaves the rest of the line to ordinary conversion.
fn copy_code_span<'a>(out: &mut String, rest: &'a str) -> &'a str {
    let after_tick = rest.split_at('`'.len_utf8()).1;
    if let Some(close) = after_tick.find('`') {
        let (content, tail) = after_tick.split_at(close);
        out.push('`');
        out.push_str(content);
        out.push('`');
        tail.split_at('`'.len_utf8()).1
    } else {
        out.push('`');
        after_tick
    }
}

/// Consume one emphasis construct at the start of `rest`: an `open`
/// delimiter, legal content, and a matching `close` delimiter emit
/// `out_open` + content + `out_close`. Anything else — no close, or
/// whitespace-edged content — emits `open` verbatim and consumes only it,
/// leaving the scanner to try again from just past that delimiter.
fn convert_emphasis<'a>(
    out: &mut String,
    rest: &'a str,
    open: &str,
    close: &str,
    out_open: char,
    out_close: char,
) -> &'a str {
    let after_open = rest.split_at(open.len()).1;
    if let Some(close_at) = after_open.find(close) {
        let (content, tail) = after_open.split_at(close_at);
        if is_emphasis_content(content) {
            out.push(out_open);
            out.push_str(content);
            out.push(out_close);
            return tail.split_at(close.len()).1;
        }
    }
    out.push_str(open);
    after_open
}

/// Emphasis content must be non-empty and not whitespace-edged — the
/// `CommonMark` flanking rule, which keeps `5 * 3 * 2` arithmetic and
/// `** spaced **` unconverted instead of italicizing the gap between them.
fn is_emphasis_content(content: &str) -> bool {
    !content.is_empty()
        && !content.starts_with(char::is_whitespace)
        && !content.ends_with(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core construct pairs, straight from the plan (Task 6, step 1).
    #[test]
    fn converter_maps_core_constructs() {
        assert_eq!(markdown_to_mrkdwn("**bold**"), "*bold*");
        assert_eq!(markdown_to_mrkdwn("*italic*"), "_italic_");
        assert_eq!(markdown_to_mrkdwn("## Header"), "*Header*");
        assert_eq!(
            markdown_to_mrkdwn("| a | b |\n|---|---|\n| 1 | 2 |"),
            "```\n| a | b |\n|---|---|\n| 1 | 2 |\n```"
        );
        // fenced blocks pass through untouched; nested ** inside code untouched
        assert_eq!(markdown_to_mrkdwn("```\n**raw**\n```"), "```\n**raw**\n```");
    }

    #[test]
    fn fenced_blocks_pass_through_completely_untouched() {
        // Language-tagged opening fence, markdown oddities, an unbalanced
        // fence — everything between the delimiters is verbatim.
        let md = "```rust\nlet x = **not bold**;\n* nor italic\n| a | not a table |\n```";
        assert_eq!(markdown_to_mrkdwn(md), md);
    }

    #[test]
    fn prose_around_a_fence_is_still_converted() {
        assert_eq!(
            markdown_to_mrkdwn("**before**\n```\n**raw**\n```\n*after*"),
            "*before*\n```\n**raw**\n```\n_after_"
        );
    }

    #[test]
    fn unmatched_single_asterisks_pass_through() {
        assert_eq!(markdown_to_mrkdwn("5 * 3 = 15"), "5 * 3 = 15");
        assert_eq!(markdown_to_mrkdwn("a lone * stays"), "a lone * stays");
        assert_eq!(markdown_to_mrkdwn("**unclosed bold"), "**unclosed bold");
    }

    #[test]
    fn whitespace_edged_emphasis_is_left_alone() {
        // `*` with space on both edges of the content is CommonMark's "not
        // emphasis"; converting it would eat arithmetic and stray bullets.
        assert_eq!(markdown_to_mrkdwn("5 * 3 * 2 = 30"), "5 * 3 * 2 = 30");
        assert_eq!(markdown_to_mrkdwn("** not bold **"), "** not bold **");
    }

    #[test]
    fn headers_strip_hashes_and_trim() {
        assert_eq!(markdown_to_mrkdwn("### Deep header"), "*Deep header*");
        assert_eq!(markdown_to_mrkdwn("###### Six"), "*Six*");
        assert_eq!(markdown_to_mrkdwn("##  Padded  "), "*Padded*");
        // A hash not followed by a space is not a header.
        assert_eq!(markdown_to_mrkdwn("#hashtag"), "#hashtag");
        // Bare hashes with nothing after them are not headers either.
        assert_eq!(markdown_to_mrkdwn("##"), "##");
    }

    #[test]
    fn inline_code_passes_through_with_contents() {
        assert_eq!(
            markdown_to_mrkdwn("run `make **all**`"),
            "run `make **all**`"
        );
        // An unmatched backtick never swallows the rest of the line.
        assert_eq!(markdown_to_mrkdwn("tick ` **bold**"), "tick ` *bold*");
    }

    #[test]
    fn bold_and_italic_share_a_line() {
        assert_eq!(
            markdown_to_mrkdwn("**bold** and *italic* and `code`"),
            "*bold* and _italic_ and `code`"
        );
        // The single pass must not re-read converted bold as italic pairs.
        assert_eq!(markdown_to_mrkdwn("**a** *b* **c**"), "*a* _b_ *c*");
    }

    #[test]
    fn tables_are_fenced_and_prose_around_them_is_kept() {
        assert_eq!(
            markdown_to_mrkdwn("Result:\n| a | b |\n|---|---|\n| 1 | 2 |\nDone **today**."),
            "Result:\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n```\nDone *today*."
        );
        // Two runs separated by prose become two fences.
        assert_eq!(
            markdown_to_mrkdwn("| one |\nmid\n| two |"),
            "```\n| one |\n```\nmid\n```\n| two |\n```"
        );
    }

    #[test]
    fn plain_text_round_trips_exactly() {
        for plain in [
            "",
            "abc",
            "abc\n\ndef",
            "trailing newline\n",
            "  indented line",
        ] {
            assert_eq!(markdown_to_mrkdwn(plain), plain);
        }
    }
}

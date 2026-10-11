//! Tests of the overview's pure parts. Its behavior with agents, files and
//! clients is tested through the hub (`hub/http/tests/overview.rs` and
//! `hub/host/tests/overview.rs`).

use std::time::Duration;

use super::*;

#[test]
fn a_preview_drops_markdown_syntax_and_keeps_the_words() {
    for (markdown, preview) in [
        (
            "**Done**: see [the report](https://example.com/r)",
            "Done: see the report",
        ),
        (
            "# Plan\n\n- first\n- second\n\n1. third",
            "Plan first second third",
        ),
        ("Run `cargo test` now", "Run cargo test now"),
        ("![a chart](chart.png) shows growth", "a chart shows growth"),
        (
            "~~old~~ new and *emphasis* and _more_",
            "old new and emphasis and more",
        ),
        ("> quoted\n> text", "quoted text"),
        ("```rust\nlet x = 1;\n```\nafter", "let x = 1; after"),
        ("| a | b |\n|---|---|\n| 1 | 2 |", "a b 1 2"),
        ("- [x] shipped\n- [ ] pending", "shipped pending"),
        ("a <b>bold</b> word", "a bold word"),
        ("bold**ly**", "boldly"),
    ] {
        assert_eq!(plain_preview(markdown), preview, "{markdown:?}");
    }
}

#[test]
fn a_preview_is_one_line_with_single_spaces() {
    assert_eq!(
        plain_preview("  first line  \n\n\n second\tline\r\nthird  "),
        "first line second line third"
    );
}

#[test]
fn text_with_nothing_to_show_has_an_empty_preview() {
    for text in ["", "   \n\t", "---", "<!-- a comment -->", "<div></div>"] {
        assert_eq!(plain_preview(text), "", "{text:?}");
    }
}

#[test]
fn a_preview_over_the_limit_is_cut_with_an_ellipsis_and_stays_within_it() {
    let exact = "a".repeat(PREVIEW_CHARS);
    assert_eq!(
        plain_preview(&exact),
        exact,
        "a preview at the limit is kept"
    );

    let over = "b".repeat(PREVIEW_CHARS + 1);
    let cut = plain_preview(&over);
    assert_eq!(cut.chars().count(), PREVIEW_CHARS, "the ellipsis counts");
    assert!(cut.ends_with('…'), "{cut}");
    assert!(cut.starts_with(&"b".repeat(PREVIEW_CHARS - 1)), "{cut}");
}

#[test]
fn a_cut_preview_counts_characters_not_bytes_and_drops_a_trailing_space() {
    // The 199 characters kept end in the space between the two words.
    let text = format!("{} {}", "é".repeat(PREVIEW_CHARS - 2), "ü".repeat(50));
    let cut = plain_preview(&text);
    assert_eq!(cut, format!("{}…", "é".repeat(PREVIEW_CHARS - 2)));
}

#[test]
fn the_overview_waits_one_second_to_gather_an_agents_changes() {
    assert_eq!(
        COALESCE_WINDOW,
        Duration::from_secs(1),
        "the window is a second"
    );
}

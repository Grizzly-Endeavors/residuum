//! A message as one short line of plain text, for the places that show only
//! the start of it.

use pulldown_cmark::{Event, Options, Parser, TagEnd};

/// The most characters a preview holds, the ellipsis that marks a cut
/// included.
pub const PREVIEW_CHARS: usize = 200;

/// What a cut preview ends with.
const ELLIPSIS: char = '…';

/// `markdown` as plain text on one line: its syntax removed (links and images
/// keep their text, code keeps its characters, raw HTML goes), every run of
/// whitespace one space, and cut to [`PREVIEW_CHARS`] characters with a
/// trailing `…` when it is longer. Empty when the text has nothing to show.
#[must_use]
pub fn plain_preview(markdown: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut text = String::new();
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak | Event::HardBreak | Event::Rule => text.push(' '),
            // Inline styling ends inside a word ("**bold**ly"); a block, row
            // or cell ends between words.
            Event::End(end) if !is_inline(end) => text.push(' '),
            Event::End(_)
            | Event::Start(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_) => {}
        }
    }
    cut(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Whether `end` closes something inside a line of text.
fn is_inline(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

/// `line`, or its start and an ellipsis when it is over the limit.
fn cut(line: &str) -> String {
    if line.chars().count() <= PREVIEW_CHARS {
        return line.to_string();
    }
    let start: String = line.chars().take(PREVIEW_CHARS - 1).collect();
    format!("{}{ELLIPSIS}", start.trim_end())
}

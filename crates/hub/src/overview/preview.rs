use pulldown_cmark::{Event, Options, Parser, TagEnd};

pub const PREVIEW_CHARS: usize = 200;

const ELLIPSIS: char = '…';

#[must_use]
pub fn plain_preview(markdown: &str) -> String {
    plain_preview_within(markdown, PREVIEW_CHARS)
}

#[must_use]
pub fn plain_preview_within(markdown: &str, max_chars: usize) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut text = String::new();
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak | Event::HardBreak | Event::Rule => text.push(' '),
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
    cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        max_chars,
    )
}

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

fn cut(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let start: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}{ELLIPSIS}", start.trim_end())
}

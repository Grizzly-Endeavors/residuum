use super::{
    ReplayScope, StreamDelta, StreamSink, ThinkingBlock, ThinkingOrigin, blocks_produced_at,
};

const OPEN_TAG: &str = "<think>";
const CLOSE_TAG: &str = "</think>";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    Thinking(String),
}

#[derive(Default)]
pub(crate) struct ThinkSplitter {
    inside: bool,
    held: String,
    emitted_text: bool,
    trim_leading: bool,
}

impl ThinkSplitter {
    pub(crate) fn push(&mut self, piece: &str) -> Vec<Piece> {
        self.held.push_str(piece);
        let mut out = Vec::new();
        loop {
            let tag = if self.inside { CLOSE_TAG } else { OPEN_TAG };
            if let Some((before, after)) = self.held.split_once(tag) {
                let (before, after) = (before.to_string(), after.to_string());
                self.emit(before, &mut out);
                self.held = after;
                if self.inside {
                    self.trim_leading = !self.emitted_text;
                }
                self.inside = !self.inside;
                continue;
            }
            let held_back = partial_tag_len(&self.held, tag);
            let (settled, partial) = self.held.split_at(self.held.len() - held_back);
            let (settled, partial) = (settled.to_string(), partial.to_string());
            self.emit(settled, &mut out);
            self.held = partial;
            return out;
        }
    }

    pub(crate) fn finish(&mut self) -> Vec<Piece> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.held);
        self.emit(rest, &mut out);
        out
    }

    fn emit(&mut self, text: String, out: &mut Vec<Piece>) {
        if self.inside {
            if !text.is_empty() {
                out.push(Piece::Thinking(text));
            }
            return;
        }
        let text = if self.trim_leading {
            let trimmed = text.trim_start();
            if !trimmed.is_empty() {
                self.trim_leading = false;
            }
            trimmed.to_string()
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        if !text.trim().is_empty() {
            self.emitted_text = true;
        }
        out.push(Piece::Text(text));
    }
}

fn partial_tag_len(text: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&len| tag.get(..len).is_some_and(|prefix| text.ends_with(prefix)))
        .unwrap_or(0)
}

pub(crate) fn plain_reasoning(
    thinking: &[ThinkingBlock],
    reader: &ThinkingOrigin,
) -> Option<String> {
    let text: Vec<&str> = blocks_produced_at(thinking, reader, ReplayScope::SameProvider)
        .into_iter()
        .map(|block| block.text.as_str())
        .filter(|text| !text.is_empty())
        .collect();
    (!text.is_empty()).then(|| text.join("\n\n"))
}

pub(crate) struct ReplyAssembler<'a> {
    sink: Option<&'a dyn StreamSink>,
    splitter: ThinkSplitter,
    content: String,
    thinking: String,
}

impl<'a> ReplyAssembler<'a> {
    pub(crate) fn new(sink: Option<&'a dyn StreamSink>) -> Self {
        Self {
            sink,
            splitter: ThinkSplitter::default(),
            content: String::new(),
            thinking: String::new(),
        }
    }

    pub(crate) fn reasoning(&mut self, text: &str) {
        self.take(Piece::Thinking(text.to_string()));
    }

    pub(crate) fn content(&mut self, text: &str) {
        for piece in self.splitter.push(text) {
            self.take(piece);
        }
    }

    pub(crate) fn finish(mut self) -> (String, Vec<ThinkingBlock>) {
        for piece in self.splitter.finish() {
            self.take(piece);
        }
        let thinking = self.thinking.trim();
        let blocks = if thinking.is_empty() {
            Vec::new()
        } else {
            vec![ThinkingBlock::text(thinking)]
        };
        (self.content, blocks)
    }

    fn take(&mut self, piece: Piece) {
        match piece {
            Piece::Text(text) => {
                self.content.push_str(&text);
                if let Some(sink) = self.sink {
                    sink.push(StreamDelta::Text(text));
                }
            }
            Piece::Thinking(text) => {
                self.thinking.push_str(&text);
                if let Some(sink) = self.sink {
                    sink.push(StreamDelta::Thinking(text));
                }
            }
        }
    }
}

#[must_use]
pub fn strip_code_fences(s: &str) -> &str {
    s.strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .map_or(s, str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingSink;

    fn split(input: &str, size: usize) -> (String, String) {
        let mut splitter = ThinkSplitter::default();
        let chars: Vec<char> = input.chars().collect();
        let mut pieces = Vec::new();
        for chunk in chars.chunks(size) {
            pieces.extend(splitter.push(&chunk.iter().collect::<String>()));
        }
        pieces.extend(splitter.finish());
        let mut text = String::new();
        let mut thinking = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(t) => text.push_str(&t),
                Piece::Thinking(t) => thinking.push_str(&t),
            }
        }
        (text, thinking)
    }

    #[test]
    fn reasoning_between_tags_is_separated_from_the_text() {
        let (text, thinking) = split("<think>weigh it</think>The answer.", usize::MAX);
        assert_eq!(text, "The answer.", "tags and reasoning leave the text");
        assert_eq!(thinking, "weigh it", "reasoning is captured");
    }

    #[test]
    fn splitting_is_independent_of_where_the_pieces_break() {
        let input =
            "<think>\nStep 1\nStep 2\n</think>\n\nHello <think>again</think>world \u{1f600}";
        let expected = split(input, usize::MAX);
        assert_eq!(
            expected,
            (
                "Hello world \u{1f600}".to_string(),
                "\nStep 1\nStep 2\nagain".to_string()
            ),
            "whole-input result"
        );
        for size in 1..input.chars().count() {
            assert_eq!(
                split(input, size),
                expected,
                "pieces of {size} characters must give the same result"
            );
        }
    }

    #[test]
    fn text_without_tags_passes_through_unchanged() {
        let input = "  plain <b>text</b> with a < and a </think\n";
        for size in [1, 3, usize::MAX] {
            assert_eq!(
                split(input, size),
                (input.to_string(), String::new()),
                "nothing to separate at piece size {size}"
            );
        }
    }

    #[test]
    fn whitespace_after_a_leading_reasoning_block_is_dropped() {
        let (leading, _) = split("<think>x</think>\n\n  Answer", 2);
        assert_eq!(leading, "Answer", "the gap the block leaves is trimmed");
        let (after_text, _) = split("Intro<think>x</think>\n\nAnswer", 2);
        assert_eq!(
            after_text, "Intro\n\nAnswer",
            "whitespace after text that already started is kept"
        );
    }

    #[test]
    fn unclosed_reasoning_runs_to_the_end_as_reasoning() {
        let (text, thinking) = split("Start<think>never closes", 4);
        assert_eq!(text, "Start", "text before the tag");
        assert_eq!(thinking, "never closes", "the rest is reasoning");
    }

    #[test]
    fn a_tag_prefix_at_the_end_of_the_reply_is_released_as_text() {
        let (text, thinking) = split("1 <thi", 2);
        assert_eq!(text, "1 <thi", "an incomplete tag at the end is just text");
        assert_eq!(thinking, "", "no reasoning");
    }

    #[test]
    fn assembler_merges_separate_reasoning_with_inline_blocks_and_streams_both() {
        let sink = RecordingSink::default();
        let mut reply = ReplyAssembler::new(Some(&sink));
        reply.reasoning("first ");
        reply.reasoning("thoughts");
        reply.content("<thi");
        reply.content("nk> more </think>The ");
        reply.content("answer");
        let (content, thinking) = reply.finish();

        assert_eq!(content, "The answer", "content has no tags");
        assert_eq!(
            thinking,
            vec![ThinkingBlock::text("first thoughts more")],
            "reasoning is one trimmed block"
        );
        assert_eq!(sink.text(), "The answer", "text was streamed");
        assert_eq!(
            sink.thinking(),
            "first thoughts more ",
            "reasoning was streamed"
        );
    }

    #[test]
    fn assembler_without_a_sink_gives_the_same_result() {
        let sink = RecordingSink::default();
        let mut streamed = ReplyAssembler::new(Some(&sink));
        let mut whole = ReplyAssembler::new(None);
        for reply in [&mut streamed, &mut whole] {
            reply.reasoning("hmm");
            reply.content("<think>a</think>b");
        }
        assert_eq!(
            streamed.finish(),
            whole.finish(),
            "same output with or without a sink"
        );
    }

    #[test]
    fn assembler_with_no_reasoning_returns_no_blocks() {
        let mut reply = ReplyAssembler::new(None);
        reply.content("just text");
        reply.reasoning("   ");
        let (content, thinking) = reply.finish();
        assert_eq!(content, "just text", "text kept");
        assert!(thinking.is_empty(), "blank reasoning is not a block");
    }
}

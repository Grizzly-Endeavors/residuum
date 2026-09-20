/// Strip `<think>...</think>` blocks from model output.
///
/// Handles multiple blocks, nested content, and unclosed tags (strips to end).
/// Trims leading whitespace left behind after stripping.
pub(crate) fn strip_think_tags(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some((before, after)) = remaining.split_once("<think>") {
        result.push_str(before);
        if let Some((_, rest)) = after.split_once("</think>") {
            remaining = rest;
        } else {
            // Unclosed tag: strip everything from <think> to end
            remaining = "";
            break;
        }
    }

    result.push_str(remaining);

    // Trim leading whitespace left behind when a think block was at the start
    let trimmed = result.trim_start();
    if trimmed.len() == result.len() {
        result
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string() {
        assert_eq!(strip_think_tags(""), "");
    }

    #[test]
    fn no_think_tags() {
        let input = "Hello, world! This is a normal response.";
        assert_eq!(strip_think_tags(input), input);
    }

    #[test]
    fn single_think_block() {
        let input = "<think>Let me reason about this...</think>The answer is 42.";
        assert_eq!(strip_think_tags(input), "The answer is 42.");
    }

    #[test]
    fn multiple_think_blocks() {
        let input = "<think>first thought</think>Hello <think>second thought</think>world";
        assert_eq!(strip_think_tags(input), "Hello world");
    }

    #[test]
    fn unclosed_think_tag() {
        let input = "Start<think>this never closes";
        assert_eq!(strip_think_tags(input), "Start");
    }

    #[test]
    fn content_before_and_after() {
        let input = "Before <think>thinking</think> After";
        assert_eq!(strip_think_tags(input), "Before  After");
    }

    #[test]
    fn think_block_at_start_trims_whitespace() {
        let input = "<think>reasoning</think>\n\nThe answer is 42.";
        assert_eq!(strip_think_tags(input), "The answer is 42.");
    }

    #[test]
    fn think_block_with_newlines_inside() {
        let input = "<think>\nStep 1: ...\nStep 2: ...\n</think>Final answer.";
        assert_eq!(strip_think_tags(input), "Final answer.");
    }

    #[test]
    fn no_stripping_of_partial_tags() {
        let input = "This has <thin and </think> but no opening";
        assert_eq!(strip_think_tags(input), input);
    }
}

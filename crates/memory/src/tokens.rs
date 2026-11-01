//! Token estimation utilities for threshold-based triggers.
//!
//! Uses character-based heuristic (~4 chars/token) rather than a
//! tokenizer dependency. Sufficient for threshold comparisons, not billing.

use residuum_inference::Message;

/// Estimate the number of tokens in a string.
///
/// Uses a ~4 characters per token heuristic.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate the total tokens across a slice of messages.
///
/// Accounts for role labels, tool call metadata, the sender prefix, reasoning
/// text and images in addition to content.
#[must_use]
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_single_message).sum()
}

/// Tokens charged for one image.
///
/// `ImageData` stores the base64 bytes and media type but not the dimensions,
/// so the provider's `width × height / 750` estimate can't be computed. This
/// is that estimate for a typical resized image.
const TOKENS_PER_IMAGE: usize = 1_600;

/// Estimate tokens for a single message including metadata overhead.
///
/// Counts the message as the request carries it: the sender prefix as rendered,
/// the reasoning text replayed with it, and its images.
pub(crate) fn estimate_single_message(msg: &Message) -> usize {
    // Rendered content, including the `[From: …]` line for a known sender
    let mut chars = msg.attributed_content().len();

    // Role label overhead (~10 chars)
    chars += 10;

    // Reasoning text; signatures and encrypted blocks are opaque and left out
    chars += msg
        .thinking
        .iter()
        .map(|block| block.text.len())
        .sum::<usize>();

    // Tool call overhead
    if let Some(calls) = &msg.tool_calls {
        for call in calls {
            chars += call.name.len();
            chars += call.arguments.to_string().len();
            chars += call.id.len();
            // Structural overhead per call
            chars += 20;
        }
    }

    // Tool call ID overhead
    if let Some(id) = &msg.tool_call_id {
        chars += id.len();
    }

    chars.div_ceil(4) + msg.images.len() * TOKENS_PER_IMAGE
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_inference::{ImageData, MessageSender, ThinkingBlock, ToolCall};

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0, "empty string should be 0 tokens");
    }

    #[test]
    fn estimate_tokens_short() {
        // "hello" = 5 chars => ceil(5/4) = 2 tokens
        assert_eq!(
            estimate_tokens("hello"),
            2,
            "5-char string should be ~2 tokens"
        );
    }

    #[test]
    fn estimate_tokens_exact_multiple() {
        // "abcdefgh" = 8 chars => ceil(8/4) = 2 tokens
        assert_eq!(
            estimate_tokens("abcdefgh"),
            2,
            "8-char string should be ~2 tokens"
        );
    }

    #[test]
    fn estimate_tokens_longer_text() {
        let text = "a".repeat(400);
        let tokens = estimate_tokens(&text);
        // 400 chars => ceil(400/4) = 100 tokens
        assert!(
            (99..=101).contains(&tokens),
            "400-char string should be ~100 tokens, got {tokens}"
        );
    }

    #[test]
    fn estimate_message_tokens_empty_slice() {
        assert_eq!(
            estimate_message_tokens(&[]),
            0,
            "empty message slice should be 0"
        );
    }

    #[test]
    fn estimate_message_tokens_basic() {
        let messages = vec![Message::user("hello world")];
        let tokens = estimate_message_tokens(&messages);
        // 11 content + 10 role = 21 chars => ceil(21/4) = 6 tokens
        assert!(tokens > 0, "should estimate some tokens");
        assert!(
            tokens < 20,
            "single short message should not be many tokens"
        );
    }

    #[test]
    fn estimate_message_tokens_with_tool_calls() {
        let messages = vec![Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
                server: None,
            }]),
        )];
        let tokens = estimate_message_tokens(&messages);
        assert!(
            tokens > 5,
            "message with tool call should have meaningful token count"
        );
    }

    #[test]
    fn estimate_message_tokens_multiple_messages() {
        let messages = vec![
            Message::user("what files are in /tmp?"),
            Message::assistant(
                "I'll check for you.",
                Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "ls /tmp"}),
                    server: None,
                }]),
            ),
            Message::tool("file1.txt\nfile2.txt", "call_1"),
        ];
        let single_tokens = estimate_message_tokens(messages.get(..1).unwrap_or_default());
        let all_tokens = estimate_message_tokens(&messages);
        assert!(
            all_tokens > single_tokens,
            "more messages should mean more tokens"
        );
    }

    #[test]
    fn plain_text_message_estimate_is_unchanged() {
        // "hello world" = 11 chars + 10 role = 21 => ceil(21/4) = 6 tokens
        assert_eq!(estimate_single_message(&Message::user("hello world")), 6);
    }

    #[test]
    fn reasoning_text_counts_toward_the_size() {
        // "The answer is 42." = 17 chars + 10 role = 27 => 7 tokens
        let plain = Message::assistant("The answer is 42.", None);
        // 27 + 400 = 427 => ceil(427/4) = 107 tokens
        let reasoned = Message::assistant("The answer is 42.", None)
            .with_thinking(vec![ThinkingBlock::text("a".repeat(400))]);

        assert_eq!(estimate_single_message(&plain), 7);
        assert_eq!(estimate_single_message(&reasoned), 107);
    }

    #[test]
    fn opaque_reasoning_blocks_are_not_counted() {
        let plain = Message::assistant("The answer is 42.", None);
        let opaque =
            Message::assistant("The answer is 42.", None).with_thinking(vec![ThinkingBlock {
                text: String::new(),
                signature: Some("sig-opaque-token".to_string()),
                redacted: Some("encrypted-blob".to_string()),
                part: None,
                origin: None,
                binding: None,
            }]);

        assert_eq!(
            estimate_single_message(&opaque),
            estimate_single_message(&plain),
            "signatures and encrypted blocks are not text the model reads"
        );
    }

    #[test]
    fn sender_prefix_counts_as_rendered() {
        let sender = MessageSender {
            name: "Jane Doe".to_string(),
            id: "u-1".to_string(),
            interface: "teams".to_string(),
            location: None,
        };
        // "[From: Jane Doe via teams]\nhi" = 29 chars + 10 role = 39 => 10 tokens
        let msg = Message::user("hi").with_sender(Some(sender));

        assert_eq!(estimate_single_message(&msg), 10);
    }

    #[test]
    fn each_image_adds_a_fixed_estimate() {
        let image = ImageData {
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        };
        let plain_tokens = estimate_single_message(&Message::user(""));
        let one = Message::user_with_images("", vec![image.clone()]);
        let two = Message::user_with_images("", vec![image.clone(), image]);

        assert_eq!(
            estimate_single_message(&one),
            plain_tokens + TOKENS_PER_IMAGE
        );
        assert_eq!(
            estimate_single_message(&two),
            plain_tokens + 2 * TOKENS_PER_IMAGE
        );
    }
}

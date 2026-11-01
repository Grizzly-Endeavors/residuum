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

/// Estimate the total tokens across a slice of stored history messages.
///
/// Accounts for role labels, tool call metadata, the sender prefix, the
/// status line, reasoning text and images in addition to content.
#[must_use]
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_single_message).sum()
}

/// Tokens charged for one image.
///
/// `ImageData` stores the base64 bytes and media type but not the dimensions,
/// so the provider's `width × height / 750` estimate can't be computed. This
/// is that estimate for a typical resized image.
pub const TOKENS_PER_IMAGE: usize = 1_600;

/// Estimate tokens for a single message including metadata overhead.
///
/// Counts the message as the request carries it: the sender prefix as rendered,
/// its stored status line, the reasoning text replayed with it, and its images.
///
/// `msg` is a stored history message. Assembly replays a stored status line
/// as its own system message ahead of the message, so a stored message is
/// where the line is counted, once.
pub(crate) fn estimate_single_message(msg: &Message) -> usize {
    // Rendered content, including the `[From: …]` line for a known sender
    let mut chars = text_chars(msg, &msg.attributed_content());

    // The status line, sent as its own system message with its own role label
    if let Some(line) = &msg.status_line {
        chars += line.len() + 10;
    }

    // Reasoning text; signatures and encrypted blocks are opaque and left out
    chars += msg
        .thinking
        .iter()
        .map(|block| block.text.len())
        .sum::<usize>();

    chars.div_ceil(4) + msg.images.len() * TOKENS_PER_IMAGE
}

/// Estimate the tokens of one message of an assembled request: its content
/// as it stands (a sender prefix already rendered into it), its role label
/// and its tool call metadata.
///
/// What a stored message sends apart from that text is left to the caller:
/// its status line, which assembly sends as a message of its own, and its
/// images and reasoning, which reach the model only as the provider decides.
#[must_use]
pub fn estimate_request_message_tokens(msg: &Message) -> usize {
    text_chars(msg, &msg.content).div_ceil(4)
}

/// The characters of `content` sent as `msg`, with its role label and tool
/// call metadata.
fn text_chars(msg: &Message, content: &str) -> usize {
    // Role label overhead (~10 chars)
    let mut chars = content.len() + 10;

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

    chars
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
    fn a_stored_status_line_counts_once_with_its_role_label() {
        let line = "[Received: Sunday Nov 1st 2026 | 10:49]";
        let mut stamped = Message::user("hello world");
        stamped.status_line = Some(line.to_string());

        // "hello world" 11 + 10 role, plus the 39-char line + 10 role = 70 => 18 tokens
        assert_eq!(line.len(), 39);
        assert_eq!(estimate_single_message(&stamped), 18);
        assert_eq!(estimate_message_tokens(std::slice::from_ref(&stamped)), 18);
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
    fn a_request_message_counts_its_text_alone() {
        let sender = MessageSender {
            name: "Jane Doe".to_string(),
            id: "u-1".to_string(),
            interface: "teams".to_string(),
            location: None,
        };
        let mut stored = Message::user_with_images(
            "hi",
            vec![ImageData {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".to_string(),
            }],
        )
        .with_sender(Some(sender))
        .with_thinking(vec![ThinkingBlock::text("a".repeat(400))]);
        stored.status_line = Some("[Received: Sunday Nov 1st 2026 | 10:49]".to_string());
        // Assembly renders the sender prefix into the content and keeps the
        // stored fields alongside it.
        let mut sent = stored.clone();
        sent.content = stored.attributed_content().into_owned();

        // "[From: Jane Doe via teams]\nhi" = 29 chars + 10 role = 39 => 10 tokens
        assert_eq!(estimate_request_message_tokens(&sent), 10);
        assert_eq!(
            estimate_request_message_tokens(&Message::user("hello world")),
            estimate_single_message(&Message::user("hello world")),
            "a plain message is the same either way"
        );
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

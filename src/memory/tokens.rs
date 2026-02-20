//! Token estimation utilities for threshold-based triggers.
//!
//! Uses character-based heuristic (~4 chars/token) rather than a
//! tokenizer dependency. Sufficient for threshold comparisons, not billing.

use crate::models::Message;

/// Estimate the number of tokens in a string.
///
/// Uses a ~4 characters per token heuristic.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate the total tokens across a slice of messages.
///
/// Accounts for role labels and tool call metadata in addition to content.
#[must_use]
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_single_message).sum()
}

/// Estimate tokens for a single message including metadata overhead.
fn estimate_single_message(msg: &Message) -> usize {
    // Base content
    let mut chars = msg.content.len();

    // Role label overhead (~10 chars)
    chars += 10;

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

    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolCall;

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
}

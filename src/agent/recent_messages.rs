//! In-memory recent message history before observation.

use crate::models::Message;

/// In-memory buffer holding recent conversation messages before observation.
pub struct RecentMessages {
    messages: Vec<Message>,
}

impl Default for RecentMessages {
    fn default() -> Self {
        Self::new()
    }
}

impl RecentMessages {
    /// Create a new empty message buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Add a message to the recent history.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get all recent messages.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get messages starting from the given index.
    #[must_use]
    pub fn messages_since(&self, idx: usize) -> &[Message] {
        self.messages.get(idx..).unwrap_or_default()
    }

    /// Get the current message count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Append all messages to the end of the buffer.
    pub fn extend(&mut self, messages: Vec<Message>) {
        self.messages.extend(messages);
    }

    /// Remove all messages from the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Check if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Return the last `n` user/assistant text-only exchange pairs.
    ///
    /// Walks backward through messages to find pairs of user + assistant
    /// messages that have text content and no tool calls. Returns them
    /// flattened in chronological order.
    #[must_use]
    pub fn last_exchanges(&self, n: usize) -> Vec<Message> {
        if n == 0 {
            return Vec::new();
        }

        let mut pairs: Vec<(Message, Message)> = Vec::new();
        let mut i = self.messages.len();

        while i > 0 && pairs.len() < n {
            i -= 1;
            let Some(msg) = self.messages.get(i) else {
                break;
            };

            // Look for an assistant message with text and no tool calls
            if msg.role != crate::models::Role::Assistant {
                continue;
            }
            if msg.content.is_empty() || msg.tool_calls.is_some() {
                continue;
            }

            // Walk backward to find the preceding user message, skipping
            // tool results and assistant messages with tool calls (the tool
            // call/result chain between a user message and the final text reply).
            let mut j = i;
            while j > 0 {
                j -= 1;
                let Some(prev) = self.messages.get(j) else {
                    break;
                };
                if prev.role == crate::models::Role::User && !prev.content.is_empty() {
                    pairs.push((prev.clone(), msg.clone()));
                    i = j; // continue scanning before this user message
                    break;
                }
                // Skip tool results and assistant-with-tool-calls (mid-chain)
                if prev.role == crate::models::Role::Tool {
                    continue;
                }
                if prev.role == crate::models::Role::Assistant && prev.tool_calls.is_some() {
                    continue;
                }
                // Any other message type means no matching user message
                break;
            }
        }

        pairs.reverse();
        pairs.into_iter().flat_map(|(u, a)| [u, a]).collect()
    }

    /// Insert messages at the beginning of the buffer.
    pub fn prepend(&mut self, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }
        let mut new_messages = messages;
        new_messages.append(&mut self.messages);
        self.messages = new_messages;
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::models::Role;

    fn push_user_msg(recent: &mut RecentMessages, content: &str) {
        recent.push(Message {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    #[test]
    fn starts_empty() {
        let recent = RecentMessages::new();
        assert!(recent.messages().is_empty(), "new buffer should be empty");
        assert!(recent.is_empty(), "new buffer should report empty");
        assert_eq!(recent.len(), 0, "new buffer should have length 0");
    }

    #[test]
    fn push_and_get() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "hello");

        assert_eq!(recent.messages().len(), 1, "should have one message");
        assert_eq!(recent.len(), 1, "len should match");
        assert_eq!(
            recent.messages().first().map(|m| &m.content),
            Some(&"hello".to_string()),
            "content should match"
        );
    }

    #[test]
    fn messages_since_returns_tail() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "first");
        push_user_msg(&mut recent, "second");
        push_user_msg(&mut recent, "third");

        let tail = recent.messages_since(1);
        assert_eq!(tail.len(), 2, "should return last two messages");
        assert_eq!(
            tail.first().map(|m| m.content.as_str()),
            Some("second"),
            "first in tail should be 'second'"
        );
    }

    fn push_assistant_msg(recent: &mut RecentMessages, content: &str) {
        recent.push(Message {
            role: Role::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    fn push_tool_msg(recent: &mut RecentMessages, content: &str) {
        recent.push(Message {
            role: Role::Tool,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
        });
    }

    fn push_assistant_with_tools(recent: &mut RecentMessages) {
        recent.push(Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(vec![crate::models::ToolCall {
                id: "call_1".to_string(),
                name: "exec".to_string(),
                arguments: serde_json::json!({"command": "echo test"}),
            }]),
            tool_call_id: None,
        });
    }

    #[test]
    fn last_exchanges_empty_buffer() {
        let recent = RecentMessages::new();
        let exchanges = recent.last_exchanges(3);
        assert!(exchanges.is_empty(), "empty buffer should return empty");
    }

    #[test]
    fn last_exchanges_no_text_responses() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "hello");
        push_assistant_with_tools(&mut recent);
        push_tool_msg(&mut recent, "tool result");

        let exchanges = recent.last_exchanges(3);
        assert!(
            exchanges.is_empty(),
            "no text-only assistant responses should return empty"
        );
    }

    #[test]
    fn last_exchanges_single_exchange() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "hello");
        push_assistant_msg(&mut recent, "hi there");

        let exchanges = recent.last_exchanges(3);
        assert_eq!(
            exchanges.len(),
            2,
            "should return one exchange (2 messages)"
        );
        assert_eq!(exchanges[0].content, "hello");
        assert_eq!(exchanges[1].content, "hi there");
    }

    #[test]
    fn last_exchanges_three_from_longer_history() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "first");
        push_assistant_msg(&mut recent, "first reply");
        push_user_msg(&mut recent, "second");
        push_assistant_msg(&mut recent, "second reply");
        push_user_msg(&mut recent, "third");
        push_assistant_msg(&mut recent, "third reply");
        push_user_msg(&mut recent, "fourth");
        push_assistant_msg(&mut recent, "fourth reply");

        let exchanges = recent.last_exchanges(3);
        assert_eq!(exchanges.len(), 6, "should return 3 exchanges (6 messages)");
        assert_eq!(exchanges[0].content, "second");
        assert_eq!(exchanges[1].content, "second reply");
        assert_eq!(exchanges[4].content, "fourth");
        assert_eq!(exchanges[5].content, "fourth reply");
    }

    #[test]
    fn last_exchanges_skips_tool_messages() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "run a command");
        push_assistant_with_tools(&mut recent);
        push_tool_msg(&mut recent, "tool result");
        push_assistant_msg(&mut recent, "the result was: test");
        push_user_msg(&mut recent, "thanks");
        push_assistant_msg(&mut recent, "you're welcome");

        let exchanges = recent.last_exchanges(3);
        // Should find "thanks"/"you're welcome" and "run a command"/"the result was: test"
        assert_eq!(
            exchanges.len(),
            4,
            "should find 2 text exchanges (4 messages), skipping tool call"
        );
        assert_eq!(exchanges[2].content, "thanks");
        assert_eq!(exchanges[3].content, "you're welcome");
    }

    #[test]
    fn prepend_to_empty() {
        let mut recent = RecentMessages::new();
        recent.prepend(vec![
            Message::user("prepended"),
            Message::assistant("response", None),
        ]);
        assert_eq!(recent.len(), 2, "should have 2 messages");
        assert_eq!(
            recent.messages()[0].content,
            "prepended",
            "first should be prepended"
        );
    }

    #[test]
    fn prepend_then_push() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "original");
        recent.prepend(vec![Message::user("prepended")]);
        assert_eq!(recent.len(), 2, "should have 2 messages");
        assert_eq!(
            recent.messages()[0].content,
            "prepended",
            "prepended should be first"
        );
        assert_eq!(
            recent.messages()[1].content,
            "original",
            "original should be second"
        );
    }

    #[test]
    fn messages_since_beyond_length() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "only");

        let tail = recent.messages_since(100);
        assert!(tail.is_empty(), "beyond-length index should return empty");
    }

    #[test]
    fn messages_since_zero_returns_all() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "first");
        push_user_msg(&mut recent, "second");

        let all = recent.messages_since(0);
        assert_eq!(all.len(), 2, "index 0 should return all messages");
    }

    #[test]
    fn last_exchanges_zero_n_returns_empty() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "hello");
        push_assistant_msg(&mut recent, "hi");

        let exchanges = recent.last_exchanges(0);
        assert!(
            exchanges.is_empty(),
            "n=0 should return empty regardless of buffer contents"
        );
    }

    #[test]
    fn extend_appends_messages() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "first");
        recent.extend(vec![
            Message::user("second"),
            Message::assistant("third", None),
        ]);
        assert_eq!(recent.len(), 3, "extend should append all messages");
        assert_eq!(
            recent.messages()[2].content,
            "third",
            "last message should be from extend"
        );
    }

    #[test]
    fn prepend_empty_vec_is_noop() {
        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "existing");
        recent.prepend(vec![]);
        assert_eq!(
            recent.len(),
            1,
            "prepending empty vec should not change length"
        );
        assert_eq!(
            recent.messages()[0].content,
            "existing",
            "existing message should be unchanged"
        );
    }
}

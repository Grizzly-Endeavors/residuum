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

    /// Remove all messages from the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Check if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
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
}

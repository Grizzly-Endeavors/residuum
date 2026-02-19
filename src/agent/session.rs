//! Session message history storage.

use crate::models::Message;

/// In-memory session storing conversation history.
pub struct Session {
    messages: Vec<Message>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Create a new empty session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Add a message to the session history.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Get all messages in the session.
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

    /// Check if the session is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;

    fn push_user_msg(session: &mut Session, content: &str) {
        session.push(Message {
            role: Role::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    #[test]
    fn session_starts_empty() {
        let session = Session::new();
        assert!(session.messages().is_empty(), "new session should be empty");
        assert!(session.is_empty(), "new session should report empty");
        assert_eq!(session.len(), 0, "new session should have length 0");
    }

    #[test]
    fn session_push_and_get() {
        let mut session = Session::new();
        push_user_msg(&mut session, "hello");

        assert_eq!(session.messages().len(), 1, "should have one message");
        assert_eq!(session.len(), 1, "len should match");
        assert_eq!(
            session.messages().first().map(|m| &m.content),
            Some(&"hello".to_string()),
            "content should match"
        );
    }

    #[test]
    fn messages_since_returns_tail() {
        let mut session = Session::new();
        push_user_msg(&mut session, "first");
        push_user_msg(&mut session, "second");
        push_user_msg(&mut session, "third");

        let tail = session.messages_since(1);
        assert_eq!(tail.len(), 2, "should return last two messages");
        assert_eq!(
            tail.first().map(|m| m.content.as_str()),
            Some("second"),
            "first in tail should be 'second'"
        );
    }

    #[test]
    fn messages_since_beyond_length() {
        let mut session = Session::new();
        push_user_msg(&mut session, "only");

        let tail = session.messages_since(100);
        assert!(tail.is_empty(), "beyond-length index should return empty");
    }

    #[test]
    fn messages_since_zero_returns_all() {
        let mut session = Session::new();
        push_user_msg(&mut session, "first");
        push_user_msg(&mut session, "second");

        let all = session.messages_since(0);
        assert_eq!(all.len(), 2, "index 0 should return all messages");
    }
}

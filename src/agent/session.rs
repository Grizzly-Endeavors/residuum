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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;

    #[test]
    fn session_starts_empty() {
        let session = Session::new();
        assert!(session.messages().is_empty(), "new session should be empty");
    }

    #[test]
    fn session_push_and_get() {
        let mut session = Session::new();
        session.push(Message {
            role: Role::User,
            content: "hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        });

        assert_eq!(session.messages().len(), 1, "should have one message");
        assert_eq!(
            session.messages().first().map(|m| &m.content),
            Some(&"hello".to_string()),
            "content should match"
        );
    }
}

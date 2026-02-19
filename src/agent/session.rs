//! Session message history storage.

use crate::models::Message;

/// In-memory session storing conversation history.
pub struct Session {
    messages: Vec<Message>,
    /// Index up to which messages have been observed by the memory system.
    observed_up_to: usize,
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
            observed_up_to: 0,
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

    /// Get messages that have not yet been observed by the memory system.
    #[must_use]
    pub fn unobserved_messages(&self) -> &[Message] {
        self.messages.get(self.observed_up_to..).unwrap_or_default()
    }

    /// Mark all current messages as observed.
    pub fn mark_observed(&mut self) {
        self.observed_up_to = self.messages.len();
    }

    /// Get the number of unobserved messages.
    #[must_use]
    pub fn unobserved_count(&self) -> usize {
        self.messages.len().saturating_sub(self.observed_up_to)
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
    }

    #[test]
    fn session_push_and_get() {
        let mut session = Session::new();
        push_user_msg(&mut session, "hello");

        assert_eq!(session.messages().len(), 1, "should have one message");
        assert_eq!(
            session.messages().first().map(|m| &m.content),
            Some(&"hello".to_string()),
            "content should match"
        );
    }

    #[test]
    fn unobserved_starts_at_all_messages() {
        let mut session = Session::new();
        push_user_msg(&mut session, "first");
        push_user_msg(&mut session, "second");

        assert_eq!(
            session.unobserved_count(),
            2,
            "all messages should be unobserved initially"
        );
        assert_eq!(
            session.unobserved_messages().len(),
            2,
            "unobserved slice should contain all messages"
        );
    }

    #[test]
    fn mark_observed_advances_watermark() {
        let mut session = Session::new();
        push_user_msg(&mut session, "first");
        push_user_msg(&mut session, "second");

        session.mark_observed();

        assert_eq!(
            session.unobserved_count(),
            0,
            "no messages should be unobserved after marking"
        );
        assert!(
            session.unobserved_messages().is_empty(),
            "unobserved slice should be empty after marking"
        );
    }

    #[test]
    fn new_messages_after_mark_are_unobserved() {
        let mut session = Session::new();
        push_user_msg(&mut session, "first");
        session.mark_observed();

        push_user_msg(&mut session, "second");
        push_user_msg(&mut session, "third");

        assert_eq!(
            session.unobserved_count(),
            2,
            "new messages after mark should be unobserved"
        );
        let unobserved = session.unobserved_messages();
        assert_eq!(unobserved.len(), 2, "should have two unobserved messages");
        assert_eq!(
            unobserved.first().map(|m| m.content.as_str()),
            Some("second"),
            "first unobserved should be 'second'"
        );
    }

    #[test]
    fn unobserved_on_empty_session() {
        let session = Session::new();
        assert_eq!(
            session.unobserved_count(),
            0,
            "empty session has no unobserved"
        );
        assert!(
            session.unobserved_messages().is_empty(),
            "empty session has no unobserved messages"
        );
    }
}

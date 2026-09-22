//! Recent unmentioned messages in a shared conversation, held for the next
//! @mention.
//!
//! Discord, Telegram, and Teams all receive messages in shared conversations
//! (server channels, groups, team channels) that don't address the bot.
//! Those messages are kept here, in memory only, and handed to the agent as
//! background context the next time the bot is addressed in that
//! conversation. Draining on mention means each message reaches the agent at
//! most once. DMs/private chats are never buffered — the bot always acts on
//! them directly, so there is nothing to hold.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;

use chrono::NaiveDateTime;

/// One message observed in a shared conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BufferedMessage {
    pub(crate) sender: String,
    pub(crate) text: String,
    pub(crate) at: NaiveDateTime,
}

/// Per-conversation ring buffers, capped at `capacity` messages each.
pub(crate) struct ContextBuffer {
    capacity: usize,
    conversations: std::sync::Mutex<HashMap<String, VecDeque<BufferedMessage>>>,
}

impl ContextBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            conversations: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Remember a message, evicting the oldest once the conversation is full.
    pub(crate) fn record(&self, conversation_id: &str, message: BufferedMessage) {
        if self.capacity == 0 {
            return;
        }
        let mut conversations = self.lock();
        let buffer = conversations
            .entry(conversation_id.to_string())
            .or_default();
        if buffer.len() == self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(message);
    }

    /// Take everything buffered for a conversation, oldest first.
    pub(crate) fn drain(&self, conversation_id: &str) -> Vec<BufferedMessage> {
        self.lock()
            .remove(conversation_id)
            .map(Vec::from)
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, VecDeque<BufferedMessage>>> {
        // The map holds plain data; a panic mid-update cannot leave it inconsistent.
        self.conversations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Render drained messages as the background note the agent sees before an @mention.
pub(crate) fn render_context(location: &str, messages: &[BufferedMessage]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let mut out = format!(
        "Previous conversation in {location} since you were last mentioned there. \
         This is background only; the message that mentions you follows.\n"
    );
    for m in messages {
        // Writing to a String cannot fail.
        _ = writeln!(out, "[{}] {}: {}", m.at.format("%H:%M"), m.sender, m.text);
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(sender: &str, text: &str, minute: u32) -> BufferedMessage {
        BufferedMessage {
            sender: sender.to_string(),
            text: text.to_string(),
            at: chrono::NaiveDate::from_ymd_opt(2026, 9, 21)
                .unwrap()
                .and_hms_opt(14, minute, 0)
                .unwrap(),
        }
    }

    #[test]
    fn keeps_only_the_newest_messages() {
        let buffer = ContextBuffer::new(2);
        buffer.record("c", msg("Jane", "one", 1));
        buffer.record("c", msg("Sam", "two", 2));
        buffer.record("c", msg("Jane", "three", 3));
        assert_eq!(
            buffer.drain("c"),
            vec![msg("Sam", "two", 2), msg("Jane", "three", 3)]
        );
    }

    #[test]
    fn drain_empties_only_that_conversation() {
        let buffer = ContextBuffer::new(5);
        buffer.record("a", msg("Jane", "in a", 1));
        buffer.record("b", msg("Sam", "in b", 1));
        assert_eq!(buffer.drain("a").len(), 1);
        assert!(
            buffer.drain("a").is_empty(),
            "second mention gets nothing new"
        );
        assert_eq!(buffer.drain("b").len(), 1);
    }

    #[test]
    fn zero_capacity_disables_buffering() {
        let buffer = ContextBuffer::new(0);
        buffer.record("c", msg("Jane", "hi", 1));
        assert!(buffer.drain("c").is_empty());
    }

    #[test]
    fn renders_a_single_background_note() {
        let rendered = render_context(
            "#builds (Eng Team)",
            &[msg("Jane", "build is red", 5), msg("Sam", "on it", 6)],
        )
        .unwrap();
        assert_eq!(
            rendered,
            "Previous conversation in #builds (Eng Team) since you were last mentioned there. \
             This is background only; the message that mentions you follows.\n\
             [14:05] Jane: build is red\n\
             [14:06] Sam: on it"
        );
        assert_eq!(render_context("#builds", &[]), None);
    }
}

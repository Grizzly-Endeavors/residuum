//! In-memory recent message history before observation.

use chrono::{DateTime, Utc};
use residuum_inference::Message;

use crate::prefix_watch::PrefixWatch;
use crate::turn::ModelRequest;

/// In-memory buffer holding recent conversation messages before observation,
/// with the time each message entered the buffer.
///
/// It is the conversation's state, so it also watches the model requests
/// built from it: each request is compared with the previous one, and a
/// change to a part already sent (which ends the provider's cached prefix
/// there) is logged. Clearing or prepending counts as a rewrite, which makes
/// the next request's history change expected.
pub struct RecentMessages {
    messages: Vec<Message>,
    /// When each entry of `messages` (same index) entered the buffer.
    recorded_at: Vec<DateTime<Utc>>,
    prefix_watch: PrefixWatch,
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
            recorded_at: Vec::new(),
            prefix_watch: PrefixWatch::default(),
        }
    }

    /// Add a message to the recent history.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        self.recorded_at.push(Utc::now());
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

    /// When each message from the given index on entered the buffer, in the
    /// same order as [`Self::messages_since`].
    #[must_use]
    pub fn recorded_at_since(&self, idx: usize) -> &[DateTime<Utc>] {
        self.recorded_at.get(idx..).unwrap_or_default()
    }

    /// Get the current message count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Append all messages to the end of the buffer.
    pub fn extend(&mut self, messages: Vec<Message>) {
        let now = Utc::now();
        self.recorded_at
            .extend(std::iter::repeat_n(now, messages.len()));
        self.messages.extend(messages);
    }

    /// Remove all messages from the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.recorded_at.clear();
        self.prefix_watch.history_rewritten();
    }

    /// Compare `request`, built from this history, with the previous request
    /// and log each earlier part that changed. `within_turn` is whether it is
    /// a later call of the same turn.
    pub(crate) fn watch_request(&mut self, request: &ModelRequest, within_turn: bool) {
        self.prefix_watch.observe(request, within_turn);
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
    #[expect(
        clippy::indexing_slicing,
        reason = "bounds guaranteed by loop invariants: i is decremented after the i > 0 check, so i < len; j < i < len at every inner access"
    )]
    pub fn last_exchanges(&self, n: usize) -> Vec<Message> {
        if n == 0 {
            return Vec::new();
        }

        let mut pairs: Vec<(Message, Message)> = Vec::new();
        let mut i = self.messages.len();

        while i > 0 && pairs.len() < n {
            i -= 1;
            let msg = &self.messages[i];

            // Look for an assistant message with text and no tool calls
            if msg.role != residuum_inference::Role::Assistant {
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
                let prev = &self.messages[j];
                if prev.role == residuum_inference::Role::User && !prev.content.is_empty() {
                    pairs.push((prev.clone(), msg.clone()));
                    i = j; // continue scanning before this user message
                    break;
                }
                // Skip tool results and assistant-with-tool-calls (mid-chain)
                if prev.role == residuum_inference::Role::Tool {
                    continue;
                }
                if prev.role == residuum_inference::Role::Assistant && prev.tool_calls.is_some() {
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
        let mut new_recorded_at = vec![Utc::now(); messages.len()];
        new_recorded_at.append(&mut self.recorded_at);
        self.recorded_at = new_recorded_at;
        let mut new_messages = messages;
        new_messages.append(&mut self.messages);
        self.messages = new_messages;
        self.prefix_watch.history_rewritten();
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    fn push_user_msg(recent: &mut RecentMessages, content: &str) {
        recent.push(Message::user(content));
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
        recent.push(Message::assistant(content, None));
    }

    fn push_tool_msg(recent: &mut RecentMessages, content: &str) {
        recent.push(Message::tool(content, "call_1"));
    }

    fn push_assistant_with_tools(recent: &mut RecentMessages) {
        recent.push(Message::assistant(
            String::new(),
            Some(vec![residuum_inference::ToolCall {
                id: "call_1".to_string(),
                name: "exec".to_string(),
                arguments: serde_json::json!({"command": "echo test"}),
                server: None,
            }]),
        ));
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
        assert_eq!(exchanges[0].content, "run a command");
        assert_eq!(exchanges[1].content, "the result was: test");
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
    fn records_when_each_message_entered() {
        let mut recent = RecentMessages::new();
        let before = Utc::now();
        push_user_msg(&mut recent, "first");
        recent.extend(vec![Message::user("second")]);
        recent.prepend(vec![Message::user("zeroth")]);
        let after = Utc::now();

        let recorded = recent.recorded_at_since(0);
        assert_eq!(recorded.len(), 3, "every message should have a time");
        assert!(
            recorded.iter().all(|at| *at >= before && *at <= after),
            "each time should be when the message entered the buffer"
        );
        assert_eq!(
            recent.recorded_at_since(1),
            &recorded[1..],
            "times should line up with messages_since"
        );

        recent.clear();
        assert!(
            recent.recorded_at_since(0).is_empty(),
            "clear should drop the times too"
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

    /// The request a turn would build from `recent`, with a fixed system
    /// prompt and no tools.
    fn request_from(recent: &RecentMessages) -> ModelRequest {
        let mut messages = vec![Message::system("prompt")];
        messages.extend(recent.messages().iter().cloned());
        ModelRequest {
            messages,
            tools: Vec::new(),
            options: residuum_inference::CompletionOptions::default(),
        }
    }

    fn watched_causes(recent: &mut RecentMessages) -> Vec<crate::prefix_watch::Cause> {
        let request = request_from(recent);
        recent
            .prefix_watch
            .compare(&request, false)
            .into_iter()
            .map(|divergence| divergence.cause)
            .collect()
    }

    #[test]
    fn rotating_the_history_makes_the_next_request_s_change_expected() {
        use crate::prefix_watch::Cause;

        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "q1");
        push_assistant_msg(&mut recent, "a1");
        push_user_msg(&mut recent, "q2");
        push_assistant_msg(&mut recent, "a2");
        assert!(watched_causes(&mut recent).is_empty(), "the first request");

        let retained = recent.last_exchanges(1);
        recent.clear();
        recent.prepend(retained);
        assert_eq!(
            watched_causes(&mut recent),
            vec![Cause::HistoryRewritten],
            "rotation rewrites the history on purpose"
        );

        push_user_msg(&mut recent, "q3");
        assert!(
            watched_causes(&mut recent).is_empty(),
            "appending after the rotation keeps the prefix"
        );
    }

    #[test]
    fn a_history_changed_in_place_is_not_a_rewrite() {
        use crate::prefix_watch::Cause;

        let mut recent = RecentMessages::new();
        push_user_msg(&mut recent, "q1");
        push_assistant_msg(&mut recent, "a1");
        assert!(watched_causes(&mut recent).is_empty(), "the first request");

        let mut edited = RecentMessages::new();
        edited.prefix_watch = std::mem::take(&mut recent.prefix_watch);
        push_user_msg(&mut edited, "q1, edited");
        push_assistant_msg(&mut edited, "a1");
        assert_eq!(
            watched_causes(&mut edited),
            vec![Cause::HistoryChanged],
            "a changed message nothing rewrote is a cache break"
        );
    }
}

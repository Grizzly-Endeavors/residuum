//! Broadcast-based display for WebSocket gateway turns.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::channels::TurnDisplay;
use crate::channels::types::ReplyHandle;

use super::protocol::ServerMessage;

/// A [`TurnDisplay`] that broadcasts tool events through a channel.
///
/// Connected WebSocket clients receive these events if they have verbose mode enabled.
pub struct BroadcastDisplay {
    tx: broadcast::Sender<ServerMessage>,
}

impl BroadcastDisplay {
    /// Create a new `BroadcastDisplay` backed by the given broadcast sender.
    #[must_use]
    pub fn new(tx: broadcast::Sender<ServerMessage>) -> Self {
        Self { tx }
    }

    /// Return a clone of the underlying broadcast sender.
    ///
    /// Used to construct per-turn `ChannelAwareDisplay` instances that share
    /// the same broadcast channel.
    #[must_use]
    pub fn sender(&self) -> broadcast::Sender<ServerMessage> {
        self.tx.clone()
    }
}

impl TurnDisplay for BroadcastDisplay {
    fn show_tool_call(&self, id: &str, name: &str, arguments: &serde_json::Value) {
        let msg = ServerMessage::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.clone(),
        };
        if self.tx.send(msg).is_err() {
            tracing::trace!("no broadcast receivers for tool_call event");
        }
    }

    fn show_tool_result(&self, id: &str, name: &str, output: &str, is_error: bool) {
        let msg = ServerMessage::ToolResult {
            tool_call_id: id.to_string(),
            name: name.to_string(),
            output: output.to_string(),
            is_error,
        };
        if self.tx.send(msg).is_err() {
            tracing::trace!("no broadcast receivers for tool_result event");
        }
    }

    fn show_response(&self, content: &str) {
        let msg = ServerMessage::BroadcastResponse {
            content: content.to_string(),
        };
        if self.tx.send(msg).is_err() {
            tracing::trace!("no broadcast receivers for intermediate response");
        }
    }
}

/// A [`TurnDisplay`] that forwards to both the broadcast channel and a reply handle.
///
/// Intermediate text responses (`show_response`) are sent to the reply handle so
/// that non-WebSocket channels (e.g. Discord) receive them during multi-tool-call
/// turns. Tool call/result events only go through the broadcast channel.
pub struct ChannelAwareDisplay {
    broadcast: BroadcastDisplay,
    reply: Arc<dyn ReplyHandle>,
}

impl ChannelAwareDisplay {
    /// Create a new channel-aware display.
    #[must_use]
    pub fn new(tx: broadcast::Sender<ServerMessage>, reply: Arc<dyn ReplyHandle>) -> Self {
        Self {
            broadcast: BroadcastDisplay::new(tx),
            reply,
        }
    }
}

impl TurnDisplay for ChannelAwareDisplay {
    fn show_tool_call(&self, id: &str, name: &str, arguments: &serde_json::Value) {
        self.broadcast.show_tool_call(id, name, arguments);
    }

    fn show_tool_result(&self, id: &str, name: &str, output: &str, is_error: bool) {
        self.broadcast.show_tool_result(id, name, output, is_error);
    }

    fn show_response(&self, content: &str) {
        self.broadcast.show_response(content);

        let reply = Arc::clone(&self.reply);
        let text = content.to_string();
        tokio::spawn(async move {
            reply.send_response(&text).await;
        });
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes after length assertion"
)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type RecordedResponses = Arc<Mutex<Vec<String>>>;

    /// Mock reply handle that records calls via a shared buffer.
    struct MockReplyHandle {
        responses: RecordedResponses,
    }

    impl MockReplyHandle {
        fn new(buf: RecordedResponses) -> Self {
            Self { responses: buf }
        }
    }

    #[async_trait::async_trait]
    impl ReplyHandle for MockReplyHandle {
        async fn send_response(&self, content: &str) {
            self.responses.lock().unwrap().push(content.to_string());
        }

        async fn send_typing(&self) {}

        async fn send_system_event(&self, _source: &str, _content: &str) {}
    }

    #[tokio::test]
    async fn channel_aware_display_forwards_intermediate_text() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let buf: RecordedResponses = Arc::new(Mutex::new(Vec::new()));
        let mock: Arc<dyn ReplyHandle> = Arc::new(MockReplyHandle::new(Arc::clone(&buf)));
        let display = ChannelAwareDisplay::new(tx, mock);

        display.show_response("thinking...");

        // Broadcast should have received it
        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::BroadcastResponse { content } if content == "thinking..."
            ),
            "broadcast should receive intermediate response, got: {msg:?}"
        );

        // Give the spawned task a moment to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let responses = buf.lock().unwrap();
        assert_eq!(responses.len(), 1, "reply handle should receive one call");
        assert_eq!(
            responses[0], "thinking...",
            "reply handle should receive the intermediate text"
        );
    }

    #[tokio::test]
    async fn channel_aware_display_does_not_forward_tool_events() {
        let (tx, _rx) = broadcast::channel::<ServerMessage>(16);
        let buf: RecordedResponses = Arc::new(Mutex::new(Vec::new()));
        let mock: Arc<dyn ReplyHandle> = Arc::new(MockReplyHandle::new(Arc::clone(&buf)));
        let display = ChannelAwareDisplay::new(tx, mock);

        display.show_tool_call("tc-1", "read_file", &serde_json::json!({"path": "foo.rs"}));
        display.show_tool_result("tc-1", "read_file", "contents", false);

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let responses = buf.lock().unwrap();
        assert!(
            responses.is_empty(),
            "tool events should not reach the reply handle"
        );
    }
}

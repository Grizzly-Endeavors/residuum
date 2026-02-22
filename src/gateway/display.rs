//! Broadcast-based display for WebSocket gateway turns.

use tokio::sync::broadcast;

use crate::channels::TurnDisplay;

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
}

impl TurnDisplay for BroadcastDisplay {
    fn show_tool_call(&self, name: &str, arguments: &serde_json::Value) {
        let msg = ServerMessage::ToolCall {
            name: name.to_string(),
            arguments: arguments.clone(),
        };
        if self.tx.send(msg).is_err() {
            tracing::trace!("no broadcast receivers for tool_call event");
        }
    }

    fn show_tool_result(&self, name: &str, output: &str, is_error: bool) {
        let msg = ServerMessage::ToolResult {
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

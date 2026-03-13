//! WebSocket interface — reply handle and bus subscriber.

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "subscriber will be wired in during bus migration")
)]
pub(crate) mod subscriber;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::gateway::protocol::ServerMessage;

use super::types::ReplyHandle;

/// Routes responses back through the WebSocket broadcast channel.
pub struct WsReplyHandle {
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reply_to: String,
}

impl WsReplyHandle {
    /// Create a new `WsReplyHandle` for a specific message correlation ID.
    #[must_use]
    pub fn new(broadcast_tx: broadcast::Sender<ServerMessage>, reply_to: String) -> Self {
        Self {
            broadcast_tx,
            reply_to,
        }
    }
}

#[async_trait]
impl ReplyHandle for WsReplyHandle {
    async fn send_response(&self, content: &str) {
        if self
            .broadcast_tx
            .send(ServerMessage::Response {
                reply_to: self.reply_to.clone(),
                content: content.to_string(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for response");
        }
    }

    async fn send_typing(&self) {
        // WebSocket has no typing indicator — no-op
    }

    async fn send_system_event(&self, source: &str, content: &str) {
        if self
            .broadcast_tx
            .send(ServerMessage::SystemEvent {
                source: source.to_string(),
                content: content.to_string(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for system event");
        }
    }

    async fn send_tool_call(&self, id: &str, name: &str, arguments: &serde_json::Value) {
        if self
            .broadcast_tx
            .send(ServerMessage::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.clone(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for tool_call event");
        }
    }

    async fn send_tool_result(&self, id: &str, name: &str, output: &str, is_error: bool) {
        if self
            .broadcast_tx
            .send(ServerMessage::ToolResult {
                tool_call_id: id.to_string(),
                name: name.to_string(),
                output: output.to_string(),
                is_error,
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for tool_result event");
        }
    }

    async fn send_intermediate(&self, content: &str) {
        if self
            .broadcast_tx
            .send(ServerMessage::BroadcastResponse {
                content: content.to_string(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for intermediate response");
        }
    }

    fn unsolicited_clone(&self) -> Option<Arc<dyn ReplyHandle>> {
        Some(Arc::new(Self::new(
            self.broadcast_tx.clone(),
            "unsolicited".to_string(),
        )))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_response_broadcasts() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        handle.send_response("hello").await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::Response { reply_to, content }
                    if reply_to == "msg-1" && content == "hello"
            ),
            "should broadcast response with correct reply_to, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_system_event_broadcasts() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        handle.send_system_event("cron: test", "event text").await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::SystemEvent { source, content }
                    if source == "cron: test" && content == "event text"
            ),
            "should broadcast system event, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_tool_call_broadcasts() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        handle
            .send_tool_call("tc-1", "exec", &serde_json::json!({"command": "echo hi"}))
            .await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::ToolCall { id, name, .. }
                    if id == "tc-1" && name == "exec"
            ),
            "should broadcast tool call, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_tool_result_broadcasts() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        handle
            .send_tool_result("tc-1", "exec", "hello", false)
            .await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::ToolResult { tool_call_id, name, output, is_error }
                    if tool_call_id == "tc-1" && name == "exec" && output == "hello" && !is_error
            ),
            "should broadcast tool result, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_intermediate_broadcasts() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        handle.send_intermediate("thinking...").await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::BroadcastResponse { content }
                    if content == "thinking..."
            ),
            "should broadcast intermediate response, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn unsolicited_clone_returns_some_and_can_send() {
        let (tx, mut rx) = broadcast::channel::<ServerMessage>(16);
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());

        let cloned = handle.unsolicited_clone();
        assert!(
            cloned.is_some(),
            "WsReplyHandle should support unsolicited_clone"
        );

        let cloned = cloned.unwrap();
        cloned.send_response("unsolicited hello").await;

        let msg = rx.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::Response { reply_to, content }
                    if reply_to == "unsolicited" && content == "unsolicited hello"
            ),
            "cloned handle should broadcast with 'unsolicited' reply_to, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn send_response_no_receivers_does_not_panic() {
        let (tx, _) = broadcast::channel::<ServerMessage>(16);
        // Drop all receivers
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());
        handle.send_response("hello").await;
        // Should not panic
    }
}

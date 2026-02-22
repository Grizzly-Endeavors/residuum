//! WebSocket-specific reply handle for the gateway.

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
    async fn send_response_no_receivers_does_not_panic() {
        let (tx, _) = broadcast::channel::<ServerMessage>(16);
        // Drop all receivers
        let handle = WsReplyHandle::new(tx, "msg-1".to_string());
        handle.send_response("hello").await;
        // Should not panic
    }
}

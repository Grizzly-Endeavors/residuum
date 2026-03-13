//! Normalized message types and reply routing for all interfaces.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;

use crate::models::ImageData;

/// Where a message originated from.
#[derive(Debug, Clone)]
pub struct MessageOrigin {
    /// Endpoint name (e.g. `"websocket"`, `"discord"`, `"webhook"`).
    pub endpoint: String,
    /// Human-readable sender name.
    pub sender_name: String,
    /// Unique sender identifier (user ID, IP, etc.).
    pub sender_id: String,
}

/// A normalized inbound message from any interface.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Correlation ID for reply routing.
    pub id: String,
    /// The user message content.
    pub content: String,
    /// Where this message came from.
    pub origin: MessageOrigin,
    /// When the message was received.
    pub timestamp: DateTime<Utc>,
    /// Inline images attached to the message.
    pub images: Vec<ImageData>,
}

/// Trait for sending responses back to the originating interface.
///
/// Each interface adapter implements this to route replies to the correct
/// destination (WebSocket broadcast, Discord DM, webhook log, etc.).
#[async_trait]
pub trait ReplyHandle: Send + Sync {
    /// Send a text response back to the message sender.
    async fn send_response(&self, content: &str);

    /// Indicate that the agent is working on a response (e.g. typing indicator).
    async fn send_typing(&self);

    /// Send a system event notification (pulse/action alerts).
    async fn send_system_event(&self, source: &str, content: &str);

    /// Start a background typing indicator that re-fires periodically.
    ///
    /// Returns a guard that cancels the indicator on drop. The default
    /// implementation returns a no-op guard suitable for interfaces without
    /// typing indicators.
    fn start_typing(&self) -> TypingGuard {
        TypingGuard::no_op()
    }

    /// Notify the interface that a tool was invoked during the agent turn.
    ///
    /// Default is a no-op — interfaces that don't display tool events
    /// (webhook, Discord) need no changes.
    async fn send_tool_call(&self, _id: &str, _name: &str, _args: &serde_json::Value) {}

    /// Notify the interface that a tool call completed.
    ///
    /// Default is a no-op.
    async fn send_tool_result(&self, _id: &str, _name: &str, _output: &str, _is_error: bool) {}

    /// Send intermediate text the model emitted alongside tool calls.
    ///
    /// Default is a no-op.
    async fn send_intermediate(&self, _content: &str) {}

    /// Create a handle that can send to the same destination without an inbound message.
    ///
    /// Returns `None` for interfaces that don't support unsolicited messaging.
    fn unsolicited_clone(&self) -> Option<Arc<dyn ReplyHandle>> {
        None
    }
}

/// A message paired with its reply handle, ready for the main loop.
pub struct RoutedMessage {
    /// The normalized inbound message.
    pub message: InboundMessage,
    /// Handle for sending responses back to the originating interface.
    pub reply: Arc<dyn ReplyHandle>,
}

/// Cancellation internals for a typing indicator background task.
struct TypingCancel {
    stop_tx: tokio::sync::watch::Sender<bool>,
    handle: JoinHandle<()>,
}

/// RAII guard that keeps a typing indicator alive until dropped.
///
/// For interfaces that support typing indicators (e.g. Discord), this spawns a
/// background task that re-sends the indicator periodically. Dropping the guard
/// signals the task to stop and aborts it.
pub struct TypingGuard {
    cancel: Option<TypingCancel>,
}

impl TypingGuard {
    /// Create a no-op guard that does nothing on drop.
    #[must_use]
    pub fn no_op() -> Self {
        Self { cancel: None }
    }

    /// Create a guard backed by a stop signal and background task handle.
    #[must_use]
    pub(crate) fn new(stop_tx: tokio::sync::watch::Sender<bool>, handle: JoinHandle<()>) -> Self {
        Self {
            cancel: Some(TypingCancel { stop_tx, handle }),
        }
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.stop_tx.send(true).ok();
            cancel.handle.abort();
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn typing_guard_no_op_drops_cleanly() {
        let guard = TypingGuard::no_op();
        assert!(guard.cancel.is_none());
        drop(guard);
    }

    #[test]
    fn default_unsolicited_clone_returns_none() {
        struct DummyReply;
        #[async_trait]
        impl ReplyHandle for DummyReply {
            async fn send_response(&self, _content: &str) {}
            async fn send_typing(&self) {}
            async fn send_system_event(&self, _source: &str, _content: &str) {}
        }
        let handle = DummyReply;
        assert!(handle.unsolicited_clone().is_none());
    }

    #[tokio::test]
    async fn typing_guard_signals_stop_on_drop() {
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async {
            // simulate a long-running task
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        });

        let guard = TypingGuard::new(stop_tx, handle);
        drop(guard);

        // The stop signal should have been sent
        stop_rx.changed().await.unwrap();
        assert!(*stop_rx.borrow(), "stop signal should be true after drop");
    }
}

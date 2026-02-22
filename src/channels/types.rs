//! Normalized message types and reply routing for all channels.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Where a message originated from.
#[derive(Debug, Clone)]
pub struct MessageOrigin {
    /// Channel name (e.g. `"websocket"`, `"discord"`, `"webhook"`).
    pub channel: String,
    /// Human-readable sender name.
    pub sender_name: String,
    /// Unique sender identifier (user ID, IP, etc.).
    pub sender_id: String,
}

/// A normalized inbound message from any channel.
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
}

/// Trait for sending responses back to the originating channel.
///
/// Each channel adapter implements this to route replies to the correct
/// destination (WebSocket broadcast, Discord DM, webhook log, etc.).
#[async_trait]
pub trait ReplyHandle: Send + Sync {
    /// Send a text response back to the message sender.
    async fn send_response(&self, content: &str);

    /// Indicate that the agent is working on a response (e.g. typing indicator).
    async fn send_typing(&self);

    /// Send a system event notification (pulse/cron alerts).
    async fn send_system_event(&self, source: &str, content: &str);
}

/// A message paired with its reply handle, ready for the main loop.
pub struct RoutedMessage {
    /// The normalized inbound message.
    pub message: InboundMessage,
    /// Handle for sending responses back to the originating channel.
    pub reply: Box<dyn ReplyHandle>,
}

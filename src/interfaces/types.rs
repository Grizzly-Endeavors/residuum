//! Normalized message types for all interfaces.

use chrono::{DateTime, Utc};

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

//! Normalized message types for all interfaces.

use chrono::{DateTime, Utc};

use crate::inference::{ImageData, MessageSender};

/// Where a message originated from.
#[derive(Debug, Clone)]
pub struct MessageOrigin {
    /// Endpoint name (e.g. `"ws"`, `"discord"`, `"background"`).
    pub endpoint: String,
    /// The person who sent it, for interfaces that identify one.
    ///
    /// `None` for the local web UI (always the owner) and for internal
    /// origins such as background tasks.
    pub sender: Option<MessageSender>,
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

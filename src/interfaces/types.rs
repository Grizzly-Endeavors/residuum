//! Normalized message types for all interfaces.

use chrono::{DateTime, Utc};

use crate::inference::{ImageData, Message, MessageSender};

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
    /// Earlier conversation supplied as background ahead of this message.
    pub context: Option<String>,
}

impl InboundMessage {
    /// The messages this adds to conversation history: its background
    /// context (if any) as a system message, then the user message itself.
    #[must_use]
    pub fn into_history_messages(self) -> Vec<Message> {
        let user = if self.images.is_empty() {
            Message::user(self.content)
        } else {
            Message::user_with_images(self.content, self.images)
        };
        self.context
            .map(Message::system)
            .into_iter()
            .chain(std::iter::once(user.with_sender(self.origin.sender)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Role;

    fn inbound(context: Option<&str>) -> InboundMessage {
        InboundMessage {
            id: "m1".to_string(),
            content: "can you check the build?".to_string(),
            origin: MessageOrigin {
                endpoint: "teams".to_string(),
                sender: Some(MessageSender {
                    name: "Jane".to_string(),
                    id: "aad-jane".to_string(),
                    interface: "teams".to_string(),
                    location: Some("#builds".to_string()),
                }),
            },
            timestamp: Utc::now(),
            images: vec![],
            context: context.map(str::to_string),
        }
    }

    #[test]
    fn context_precedes_the_attributed_user_message() {
        let messages = inbound(Some("[14:05] Sam: build is red")).into_history_messages();
        let [context, user] = messages.as_slice() else {
            panic!("expected context then user message, got {messages:?}");
        };
        assert_eq!(context.role, Role::System);
        assert_eq!(context.content, "[14:05] Sam: build is red");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.content, "can you check the build?");
        assert_eq!(user.sender.as_ref().map(|s| s.name.as_str()), Some("Jane"));
    }

    #[test]
    fn without_context_only_the_user_message() {
        let messages = inbound(None).into_history_messages();
        let [user] = messages.as_slice() else {
            panic!("expected a single user message, got {messages:?}");
        };
        assert_eq!(user.role, Role::User);
    }
}

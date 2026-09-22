//! Normalized message types for all interfaces.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::inference::{ImageData, Message, MessageSender};

/// Kind of chat conversation, which decides whether the bot needs an @mention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    /// 1:1 chat between one person and the bot.
    Personal,
    /// Group chat with the bot as a member.
    GroupChat,
    /// A channel in a team, server, or similar shared space.
    Channel,
}

/// The conversation a message belongs to on its interface.
///
/// `None` on [`MessageOrigin`] for the local web UI (which has no
/// conversation concept and is always treated as the owner's) and for
/// internal origins such as background and agent messages, so the fields
/// stay unambiguous rather than carrying a made-up id or kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationContext {
    /// Stable id for this conversation — the same id `list_conversations`
    /// and `send_message` use to reach it.
    pub id: String,
    /// Personal / group chat / channel.
    pub kind: ConversationKind,
    /// Whether the sender is the interface's claimed owner.
    pub is_owner: bool,
}

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
    /// The conversation this message belongs to on its interface, when the
    /// interface has one. See [`ConversationContext`] for what `None` means.
    pub conversation: Option<ConversationContext>,
}

impl MessageOrigin {
    /// Whether this message belongs to the main agent's own conversation.
    ///
    /// True for the web UI, background/internal origins (`conversation:
    /// None`), and the owner's own DM on a chat interface. False for every
    /// other admitted conversation — a group chat, a channel (including the
    /// owner speaking in one), or a non-owner DM — which routes to that
    /// conversation's `external` session instead. Admission (whether the
    /// message reaches the agent system at all) is decided upstream by each
    /// interface; this only decides who handles an already-admitted message.
    #[must_use]
    pub fn belongs_to_main(&self) -> bool {
        match &self.conversation {
            None => true,
            Some(ctx) => ctx.kind == ConversationKind::Personal && ctx.is_owner,
        }
    }
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
                conversation: Some(ConversationContext {
                    id: "19:abc@thread.tacv2".to_string(),
                    kind: ConversationKind::Channel,
                    is_owner: true,
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

    fn origin_with(kind: ConversationKind, is_owner: bool) -> MessageOrigin {
        MessageOrigin {
            endpoint: "discord".to_string(),
            sender: None,
            conversation: Some(ConversationContext {
                id: "conv-1".to_string(),
                kind,
                is_owner,
            }),
        }
    }

    #[test]
    fn owner_personal_dm_belongs_to_main() {
        assert!(origin_with(ConversationKind::Personal, true).belongs_to_main());
    }

    #[test]
    fn owner_speaking_in_a_group_chat_does_not_belong_to_main() {
        assert!(!origin_with(ConversationKind::GroupChat, true).belongs_to_main());
    }

    #[test]
    fn owner_speaking_in_a_channel_does_not_belong_to_main() {
        assert!(!origin_with(ConversationKind::Channel, true).belongs_to_main());
    }

    #[test]
    fn non_owner_personal_dm_does_not_belong_to_main() {
        assert!(!origin_with(ConversationKind::Personal, false).belongs_to_main());
    }

    #[test]
    fn non_owner_group_chat_does_not_belong_to_main() {
        assert!(!origin_with(ConversationKind::GroupChat, false).belongs_to_main());
    }

    #[test]
    fn no_conversation_belongs_to_main() {
        // The web UI and background/internal origins.
        let origin = MessageOrigin {
            endpoint: "ws".to_string(),
            sender: None,
            conversation: None,
        };
        assert!(origin.belongs_to_main());
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

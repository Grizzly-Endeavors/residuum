//! Normalized message types for all interfaces.

use serde::{Deserialize, Serialize};

use crate::inference::{AgentSender, MessageSender};

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
    /// The agent that sent it, for a message one agent sent main. `None`
    /// for everything else. Boxed because it is rarely set and
    /// `MessageEvent` is carried inline in several enums.
    pub agent_sender: Option<Box<AgentSender>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn origin_with(kind: ConversationKind, is_owner: bool) -> MessageOrigin {
        MessageOrigin {
            endpoint: "discord".to_string(),
            sender: None,
            conversation: Some(ConversationContext {
                id: "conv-1".to_string(),
                kind,
                is_owner,
            }),
            agent_sender: None,
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
            agent_sender: None,
        };
        assert!(origin.belongs_to_main());
    }
}

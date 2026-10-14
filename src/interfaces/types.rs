//! Normalized message types for all interfaces.

use serde::{Deserialize, Serialize};

use crate::bus::EndpointName;
use crate::inference::{AgentSender, MessageSender};

/// The endpoint name of a message no interface delivered: a result relayed
/// to the main agent, another agent's message, a delivery-failure notice.
pub const BACKGROUND_ENDPOINT: &str = "background";

/// The web UI's endpoint name: always the owner's own channel, with no
/// conversation concept to post elsewhere within.
///
/// The web UI follows the main agent's turns through the main conversation
/// topic. What is published to its endpoint topic is only what the agent
/// posts to the web with `send_message`, files included.
pub const WEB_UI_ENDPOINT: &str = "ws";

/// The endpoint whose topic carries a turn's events for the chat interface
/// behind it: `delivered_to` itself, unless it is the web UI, which takes a
/// turn's lifecycle, intermediate text, reply and failure from the main
/// conversation topic instead. `None` when the turn is delivered nowhere.
///
/// Every other endpoint name is treated as a chat interface, so a new one
/// receives its turns' events without further wiring.
#[must_use]
pub fn chat_interface_endpoint(delivered_to: Option<&EndpointName>) -> Option<&EndpointName> {
    delivered_to.filter(|endpoint| endpoint.as_ref() != WEB_UI_ENDPOINT)
}

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

    /// Whether a person sent this, as opposed to the system or an agent:
    /// false for a background origin and for one agent's message to another.
    #[must_use]
    pub fn is_from_person(&self) -> bool {
        self.endpoint != BACKGROUND_ENDPOINT && self.agent_sender.is_none()
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
            endpoint: WEB_UI_ENDPOINT.to_string(),
            sender: None,
            conversation: None,
            agent_sender: None,
        };
        assert!(origin.belongs_to_main());
    }

    #[test]
    fn a_turn_reaches_the_endpoint_topic_of_a_chat_interface_but_not_the_web_ui() {
        let name = |endpoint: &str| EndpointName::from(endpoint);
        for chat in ["telegram", "discord", "teams", "slack-someday"] {
            let endpoint = name(chat);
            assert_eq!(
                chat_interface_endpoint(Some(&endpoint)),
                Some(&endpoint),
                "{chat} reads its turns' events from its endpoint topic"
            );
        }
        assert_eq!(
            chat_interface_endpoint(Some(&name(WEB_UI_ENDPOINT))),
            None,
            "the web UI follows the main conversation topic"
        );
        assert_eq!(chat_interface_endpoint(None), None, "nowhere to deliver");
    }
}

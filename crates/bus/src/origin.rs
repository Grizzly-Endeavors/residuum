use serde::{Deserialize, Serialize};

use crate::EndpointName;
use residuum_config::WEB_UI_ENDPOINT;
use residuum_inference::{AgentSender, MessageSender};

pub const BACKGROUND_ENDPOINT: &str = "background";

pub const A2A_ENDPOINT: &str = "a2a";

#[must_use]
pub fn chat_interface_endpoint(delivered_to: Option<&EndpointName>) -> Option<&EndpointName> {
    delivered_to.filter(|endpoint| endpoint.as_ref() != WEB_UI_ENDPOINT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Personal,
    GroupChat,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationContext {
    pub id: String,
    pub kind: ConversationKind,
    pub is_owner: bool,
}

#[derive(Debug, Clone)]
pub struct MessageOrigin {
    pub endpoint: String,
    pub sender: Option<MessageSender>,
    pub conversation: Option<ConversationContext>,
    pub agent_sender: Option<Box<AgentSender>>,
}

impl MessageOrigin {
    #[must_use]
    pub fn belongs_to_main(&self) -> bool {
        match &self.conversation {
            None => true,
            Some(ctx) => ctx.kind == ConversationKind::Personal && ctx.is_owner,
        }
    }

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

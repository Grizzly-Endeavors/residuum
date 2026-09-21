//! The subset of the Bot Framework Activity schema the Teams channel uses.
//!
//! Only the fields Residuum reads are modelled; everything else in the
//! payload is ignored on deserialization.

use serde::{Deserialize, Serialize};

/// An inbound activity the Bot Connector sends to the messaging endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Activity {
    /// Activity type (`"message"`, `"conversationUpdate"`, `"installationUpdate"`, ...).
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) id: Option<String>,
    /// Base URL of the Bot Connector instance to reply through.
    pub(super) service_url: Option<String>,
    /// Channel the activity came from; always `"msteams"` for Teams.
    pub(super) channel_id: Option<String>,
    pub(super) from: Option<ChannelAccount>,
    pub(super) conversation: Option<ConversationAccount>,
    /// The bot itself.
    pub(super) recipient: Option<ChannelAccount>,
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) entities: Vec<Entity>,
    #[serde(default)]
    pub(super) attachments: Vec<Attachment>,
    pub(super) channel_data: Option<TeamsChannelData>,
    #[serde(default)]
    pub(super) members_added: Vec<ChannelAccount>,
    #[serde(default)]
    pub(super) members_removed: Vec<ChannelAccount>,
    /// `"add"` / `"remove"` on `installationUpdate`.
    pub(super) action: Option<String>,
}

/// A user or bot on the channel.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChannelAccount {
    pub(super) id: String,
    pub(super) name: Option<String>,
    /// Entra object ID of a user; stable across chats.
    pub(super) aad_object_id: Option<String>,
}

/// The conversation an activity belongs to.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationAccount {
    pub(super) id: String,
    pub(super) name: Option<String>,
    /// `"personal"`, `"groupChat"`, or `"channel"`.
    pub(super) conversation_type: Option<String>,
    pub(super) tenant_id: Option<String>,
}

/// An entity attached to a message; Residuum only reads `mention` entities.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Entity {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) mentioned: Option<ChannelAccount>,
    /// The inline markup for the mention, e.g. `<at>Residuum</at>`.
    pub(super) text: Option<String>,
}

/// A file or card attached to a message.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Attachment {
    pub(super) content_type: String,
    pub(super) content_url: Option<String>,
    pub(super) name: Option<String>,
    pub(super) content: Option<serde_json::Value>,
}

/// Teams-specific routing data on an activity.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct TeamsChannelData {
    pub(super) tenant: Option<IdRef>,
    pub(super) team: Option<NamedRef>,
    pub(super) channel: Option<NamedRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct IdRef {
    pub(super) id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct NamedRef {
    pub(super) name: Option<String>,
}

/// Kind of Teams conversation, which decides whether the bot needs an @mention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConversationKind {
    /// 1:1 chat between one person and the bot.
    Personal,
    /// Group chat with the bot as a member.
    GroupChat,
    /// Standard team channel.
    Channel,
}

impl ConversationKind {
    fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("groupChat") => Self::GroupChat,
            Some("channel") => Self::Channel,
            _ => Self::Personal,
        }
    }
}

impl Activity {
    /// Tenant the activity belongs to, from channel data or the conversation.
    pub(super) fn tenant_id(&self) -> Option<&str> {
        self.channel_data
            .as_ref()
            .and_then(|cd| cd.tenant.as_ref())
            .map(|t| t.id.as_str())
            .or_else(|| {
                self.conversation
                    .as_ref()
                    .and_then(|c| c.tenant_id.as_deref())
            })
    }

    pub(super) fn conversation_kind(&self) -> ConversationKind {
        ConversationKind::from_wire(
            self.conversation
                .as_ref()
                .and_then(|c| c.conversation_type.as_deref()),
        )
    }

    /// Human-readable place this activity came from, e.g. `"#General (Eng Team)"`.
    pub(super) fn location_label(&self) -> String {
        match self.conversation_kind() {
            ConversationKind::Personal => "direct message".to_string(),
            ConversationKind::GroupChat => {
                match self.conversation.as_ref().and_then(|c| c.name.as_deref()) {
                    Some(name) if !name.is_empty() => format!("group chat \"{name}\""),
                    _ => "group chat".to_string(),
                }
            }
            ConversationKind::Channel => {
                let cd = self.channel_data.as_ref();
                // Teams reports the General channel's name as null so clients can localize it.
                let channel = cd
                    .and_then(|cd| cd.channel.as_ref())
                    .and_then(|c| c.name.as_deref())
                    .unwrap_or("General");
                match cd
                    .and_then(|cd| cd.team.as_ref())
                    .and_then(|t| t.name.as_deref())
                {
                    Some(team) => format!("#{channel} ({team})"),
                    None => format!("#{channel}"),
                }
            }
        }
    }

    /// Whether this message @mentions the bot.
    pub(super) fn mentions_bot(&self) -> bool {
        let Some(bot) = &self.recipient else {
            return false;
        };
        self.entities
            .iter()
            .any(|e| e.kind == "mention" && e.mentioned.as_ref().is_some_and(|m| m.id == bot.id))
    }

    /// Message text with the bot's own @mention removed and other mentions
    /// rendered as `@Name`.
    pub(super) fn text_without_bot_mention(&self) -> String {
        let mut text = self.text.clone().unwrap_or_default();
        let bot_id = self.recipient.as_ref().map(|r| r.id.as_str());
        for entity in self.entities.iter().filter(|e| e.kind == "mention") {
            let (Some(markup), Some(mentioned)) = (&entity.text, &entity.mentioned) else {
                continue;
            };
            let replacement = if Some(mentioned.id.as_str()) == bot_id {
                String::new()
            } else {
                format!("@{}", mentioned.name.as_deref().unwrap_or("someone"))
            };
            text = text.replace(markup.as_str(), &replacement);
        }
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: serde_json::Value) -> Activity {
        serde_json::from_value(json).unwrap()
    }

    fn channel_mention() -> Activity {
        parse(serde_json::json!({
            "type": "message",
            "id": "1700000000000",
            "serviceUrl": "https://smba.trafficmanager.net/amer/",
            "channelId": "msteams",
            "from": { "id": "29:user", "name": "Jane Doe", "aadObjectId": "aad-jane" },
            "conversation": { "id": "19:abc@thread.tacv2;messageid=1", "conversationType": "channel", "tenantId": "tenant-1" },
            "recipient": { "id": "28:bot", "name": "Residuum" },
            "text": "<at>Residuum</at> can <at>Sam Lee</at> review this?",
            "entities": [
                { "type": "mention", "text": "<at>Residuum</at>", "mentioned": { "id": "28:bot", "name": "Residuum" } },
                { "type": "mention", "text": "<at>Sam Lee</at>", "mentioned": { "id": "29:sam", "name": "Sam Lee" } },
                { "type": "clientInfo", "locale": "en-US" }
            ],
            "channelData": {
                "tenant": { "id": "tenant-1" },
                "team": { "id": "19:team", "name": "Eng Team" },
                "channel": { "id": "19:abc@thread.tacv2", "name": "builds" }
            }
        }))
    }

    #[test]
    fn detects_bot_mention_and_strips_only_the_bot() {
        let activity = channel_mention();
        assert!(activity.mentions_bot());
        assert_eq!(
            activity.text_without_bot_mention(),
            "can @Sam Lee review this?"
        );
    }

    #[test]
    fn mention_of_someone_else_is_not_a_bot_mention() {
        let mut activity = channel_mention();
        activity.entities.remove(0);
        assert!(!activity.mentions_bot());
    }

    #[test]
    fn location_labels_per_conversation_kind() {
        assert_eq!(channel_mention().location_label(), "#builds (Eng Team)");

        let general = parse(serde_json::json!({
            "type": "message",
            "conversation": { "id": "19:x", "conversationType": "channel" },
            "channelData": { "team": { "id": "19:team", "name": "Eng Team" }, "channel": { "id": "19:x" } }
        }));
        assert_eq!(general.location_label(), "#General (Eng Team)");

        let group = parse(serde_json::json!({
            "type": "message",
            "conversation": { "id": "19:g@thread.v2", "conversationType": "groupChat", "name": "Launch prep" }
        }));
        assert_eq!(group.location_label(), "group chat \"Launch prep\"");

        let personal = parse(serde_json::json!({
            "type": "message",
            "conversation": { "id": "a:1", "conversationType": "personal" }
        }));
        assert_eq!(personal.location_label(), "direct message");
    }

    #[test]
    fn tenant_prefers_channel_data() {
        assert_eq!(channel_mention().tenant_id(), Some("tenant-1"));
        let only_conversation = parse(serde_json::json!({
            "type": "message",
            "conversation": { "id": "a:1", "tenantId": "tenant-2" }
        }));
        assert_eq!(only_conversation.tenant_id(), Some("tenant-2"));
    }
}

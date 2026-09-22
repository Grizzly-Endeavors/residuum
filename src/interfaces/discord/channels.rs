//! Discord server channels: labels, @mention handling, and listing for the agent.

use std::sync::Arc;

use async_trait::async_trait;
use serenity::http::Http;
use serenity::model::channel::{ChannelType, GuildChannel};
use serenity::model::id::{ChannelId, GuildId, UserId};

use crate::interfaces::chat_state::ConversationKind;
use crate::interfaces::conversations::{ConversationSource, KnownConversation};

use super::DiscordState;

/// Most servers listed in `list_conversations`; Discord's page size.
const MAX_GUILDS: u64 = 200;

/// `"#builds (Eng Team)"`.
fn channel_label(channel: &str, guild: &str) -> String {
    format!("#{channel} ({guild})")
}

/// Server channels the bot can post plain messages into.
fn is_text_channel(channel: &GuildChannel) -> bool {
    matches!(
        channel.kind,
        ChannelType::Text
            | ChannelType::News
            | ChannelType::PublicThread
            | ChannelType::PrivateThread
            | ChannelType::NewsThread
    )
}

/// Message text with the bot's own mention removed; other mentions are kept.
pub(super) fn strip_bot_mention(content: &str, bot_id: UserId) -> String {
    content
        .replace(&format!("<@{bot_id}> "), "")
        .replace(&format!("<@!{bot_id}> "), "")
        .replace(&format!("<@{bot_id}>"), "")
        .replace(&format!("<@!{bot_id}>"), "")
        .trim()
        .to_string()
}

/// Human-readable location of a server channel, fetched once and then cached.
pub(super) async fn guild_channel_label(
    state: &DiscordState,
    http: &Http,
    guild_id: GuildId,
    channel_id: ChannelId,
) -> String {
    if let Some(label) = state.cached_label(channel_id) {
        return label;
    }
    let channel = match channel_id.to_channel(http).await {
        Ok(channel) => channel.guild().map(|c| c.name),
        Err(e) => {
            tracing::warn!(error = %e, %channel_id, "failed to look up discord channel name");
            None
        }
    };
    let guild = match guild_id.to_partial_guild(http).await {
        Ok(guild) => Some(guild.name),
        Err(e) => {
            tracing::warn!(error = %e, %guild_id, "failed to look up discord server name");
            None
        }
    };
    match (channel, guild) {
        (Some(channel), Some(guild)) => {
            let label = channel_label(&channel, &guild);
            state.cache_label(channel_id, label.clone());
            label
        }
        // Not cached, so a transient failure is retried on the next message.
        (Some(channel), None) => format!("#{channel}"),
        _ => "a server channel".to_string(),
    }
}

/// The conversations Discord can reach: saved DMs plus every text channel in
/// the servers the bot is a member of.
pub(super) struct DiscordConversations {
    pub(super) state: Arc<DiscordState>,
    pub(super) http: Arc<Http>,
}

#[async_trait]
impl ConversationSource for DiscordConversations {
    async fn owner_dm_conversation_id(&self) -> Option<String> {
        self.state.store.owner().await.map(|o| o.dm_conversation_id)
    }

    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
        use anyhow::Context as _;

        let mut conversations = self.state.store.known_conversations().await;
        let guilds = self
            .http
            .get_guilds(None, Some(MAX_GUILDS))
            .await
            .context("failed to list the discord servers the bot is in")?;
        for guild in guilds {
            let channels = guild.id.channels(&self.http).await.with_context(|| {
                format!("failed to list channels in discord server {}", guild.name)
            })?;
            for channel in channels.values().filter(|c| is_text_channel(c)) {
                let label = channel_label(&channel.name, &guild.name);
                self.state.cache_label(channel.id, label.clone());
                conversations.push(KnownConversation {
                    id: channel.id.to_string(),
                    kind: ConversationKind::Channel,
                    label,
                });
            }
        }
        Ok(conversations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_mention_is_stripped_in_both_forms() {
        let bot = UserId::new(42);
        assert_eq!(
            strip_bot_mention("<@42> can you check the build?", bot),
            "can you check the build?"
        );
        assert_eq!(
            strip_bot_mention("hey <@!42> ping <@7>\nsecond line", bot),
            "hey ping <@7>\nsecond line"
        );
    }

    #[test]
    fn label_names_channel_and_server() {
        assert_eq!(channel_label("builds", "Eng Team"), "#builds (Eng Team)");
    }
}

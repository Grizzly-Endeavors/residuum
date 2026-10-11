//! Slack interface — Socket Mode, DM-only MVP (design: docs/design/slack-interface.md).
//!
//! One interface instance per agent, each with its own app tokens and socket,
//! mirroring the Telegram adapter's shape. Inbound DM events deliver into the
//! agent's main conversation; outbound replies ride the conversation-only
//! contract shared by every external chat interface.
pub(crate) mod api;
pub(crate) mod events;
pub(crate) mod format;
pub(crate) mod handler;
pub(crate) mod socket;
pub(crate) mod subscriber;

use crate::bus::Publisher;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
use crate::interfaces::conversations::{ConversationSource, KnownConversation};
use crate::interfaces::reply_targets::ReplyTargets;

/// Endpoint and interface name for Slack on the bus. Telegram keeps its
/// equivalent in `telegram/mod.rs`; hoisted here from `handler.rs` once the
/// outbound subscriber (plan Task 6) needed it too.
const ENDPOINT: &str = "slack";

/// State shared by the inbound handler and the outbound subscriber —
/// `telegram/mod.rs`'s `TelegramState`, minus the group machinery Slack's
/// DM-only MVP does not have (context buffering, group membership).
pub(crate) struct SlackState {
    /// The owner and every DM channel the bot has seen, keyed by channel id,
    /// persisted in `slack_state.json`.
    pub(crate) store: ChatStateStore<ChatRef>,
    /// Channel each in-flight turn should answer in, by correlation id,
    /// recorded by the handler at dispatch (`telegram/handler.rs`'s
    /// `reply_targets.track`).
    pub(crate) reply_targets: ReplyTargets<String>,
    /// For notifying main when output cannot be delivered (see
    /// `interfaces/outbound.rs`).
    pub(crate) publisher: Publisher,
}

impl SlackState {
    /// Where output for `correlation_id` goes: the channel that started the
    /// turn, or the owner's DM for proactive output — Telegram's
    /// `target_for` verbatim. Like Telegram, Slack does not parse the
    /// correlation at reply time; the handler recorded the mapping when the
    /// message was dispatched, and only the proactive fallback resolves here.
    pub(crate) async fn target_for(&self, correlation_id: &str) -> Option<String> {
        match self.reply_targets.get(correlation_id) {
            Some(channel) => Some(channel),
            None => self.owner_dm().await,
        }
    }

    /// The owner's DM channel, or `None` before any owner is bound. Unlike
    /// Telegram's chat ids there is nothing to parse — a Slack channel id is
    /// already the string `post_message` takes.
    pub(crate) async fn owner_dm(&self) -> Option<String> {
        self.store
            .owner()
            .await
            .map(|owner| owner.dm_conversation_id)
    }
}

#[async_trait::async_trait]
impl ConversationSource for SlackState {
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
        Ok(self.store.known_conversations().await)
    }

    async fn owner_dm_conversation_id(&self) -> Option<String> {
        self.store.owner().await.map(|o| o.dm_conversation_id)
    }
}

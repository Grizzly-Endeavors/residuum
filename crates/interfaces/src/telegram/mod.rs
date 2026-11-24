//! Telegram interface adapter.
//!
//! Uses the teloxide Bot API for long-polling message reception and publishes
//! messages onto the bus for agent processing.
//!
//! - Private messages always reach the agent; in groups only messages
//!   addressed to the bot do (see `groups`).
//! - The owner is whoever first DMs the bot; others are refused unless
//!   `[telegram] respond_to_others` is on. Commands run only for the owner.
//! - Replies go back to the chat a message came from; proactive output goes
//!   to the owner's chat unless `send_message` names a conversation. Notices
//!   and errors only ever go to the owner.

mod groups;
mod handler;
pub(crate) mod subscriber;

use std::path::PathBuf;
use std::sync::Arc;

use teloxide_core::types::ChatId;

use crate::AdapterSenders;
use crate::chat_state::{ChatRef, ChatStateStore};
use crate::context_buffer::ContextBuffer;
use crate::reply_targets::ReplyTargets;
use residuum_bus::Publisher;
use residuum_bus::conversations::{ConversationSource, KnownConversation};
use residuum_config::TelegramConfig;

const ENDPOINT: &str = "telegram";

pub(super) struct TelegramState {
    respond_to_others: bool,
    deliver_intermediate: bool,
    store: ChatStateStore<ChatRef>,
    reply_targets: ReplyTargets<ChatId>,
    context_buffer: ContextBuffer,
    publisher: Publisher,
}

impl TelegramState {
    async fn target_for(&self, correlation_id: &str) -> Option<ChatId> {
        match self.reply_targets.get(correlation_id) {
            Some(chat) => Some(chat),
            None => self.owner_dm().await,
        }
    }

    async fn owner_dm(&self) -> Option<ChatId> {
        let owner = self.store.owner().await?;
        match owner.dm_conversation_id.parse::<i64>() {
            Ok(id) => Some(ChatId(id)),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    chat = %owner.dm_conversation_id,
                    "saved telegram owner chat is not a valid chat ID"
                );
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl ConversationSource for TelegramState {
    async fn conversations(&self) -> anyhow::Result<Vec<KnownConversation>> {
        Ok(self.store.known_conversations().await)
    }

    async fn owner_dm_conversation_id(&self) -> Option<String> {
        self.store.owner().await.map(|o| o.dm_conversation_id)
    }
}

pub struct TelegramInterface {
    cfg: TelegramConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl TelegramInterface {
    #[must_use]
    pub fn new(
        cfg: TelegramConfig,
        senders: AdapterSenders,
        workspace_dir: PathBuf,
        tz: chrono_tz::Tz,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            cfg,
            senders,
            workspace_dir,
            tz,
            shutdown_rx,
        }
    }

    pub async fn start(self) -> anyhow::Result<()> {
        let layout = residuum_config::WorkspaceLayout::new(&self.workspace_dir);
        let (chat_store, chat_state_notice) =
            ChatStateStore::load(layout.telegram_state_json()).await?;
        if let Some(notice) = chat_state_notice {
            residuum_bus::notice::publish_notice(&self.senders.publisher, notice).await;
        }
        let state = Arc::new(TelegramState {
            respond_to_others: self.cfg.respond_to_others,
            deliver_intermediate: self.cfg.deliver_intermediate,
            store: chat_store,
            reply_targets: ReplyTargets::default(),
            context_buffer: ContextBuffer::new(self.cfg.context_messages),
            publisher: self.senders.publisher.clone(),
        });
        let _registration = self
            .senders
            .conversations
            .register(ENDPOINT, Arc::clone(&state) as Arc<dyn ConversationSource>);
        handler::run_telegram_polling(
            &self.cfg.token,
            state,
            self.senders,
            self.workspace_dir,
            self.tz,
            self.shutdown_rx,
        )
        .await
    }
}

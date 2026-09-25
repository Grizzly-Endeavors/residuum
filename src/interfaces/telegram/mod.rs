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

use teloxide::types::ChatId;

use crate::bus::Publisher;
use crate::config::TelegramConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
use crate::interfaces::context_buffer::ContextBuffer;
use crate::interfaces::conversations::{ConversationSource, KnownConversation};
use crate::interfaces::reply_targets::ReplyTargets;

/// Endpoint and interface name for Telegram on the bus.
const ENDPOINT: &str = "telegram";

/// State shared by the polling loop and the outbound subscriber.
pub(super) struct TelegramState {
    respond_to_others: bool,
    /// The owner and every chat the bot has seen, keyed by chat ID.
    store: ChatStateStore<ChatRef>,
    /// Chat each in-flight turn should answer in, by correlation ID.
    reply_targets: ReplyTargets<ChatId>,
    /// Unaddressed group messages held for the next time the bot is
    /// addressed, by chat.
    context_buffer: ContextBuffer,
    /// For notifying main when a conversation session's output can't be
    /// delivered (see `subscriber::deliver_session_response`).
    publisher: Publisher,
}

impl TelegramState {
    /// Where output for `correlation_id` goes: the chat that started the
    /// turn, or the owner's DM for proactive output.
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

/// Telegram interface adapter that routes private and addressed group messages to the agent.
pub struct TelegramInterface {
    cfg: TelegramConfig,
    senders: AdapterSenders,
    workspace_dir: PathBuf,
    tz: chrono_tz::Tz,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl TelegramInterface {
    /// Create a new Telegram interface adapter.
    #[must_use]
    pub(crate) fn new(
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

    /// Start the Telegram long-polling loop.
    ///
    /// This blocks until a shutdown signal is received, an error occurs, or the
    /// task is cancelled. A corrupt saved Telegram state file is moved aside
    /// and started fresh rather than treated as an error (see
    /// [`ChatStateStore::load`]). Verifying the bot token retries with
    /// backoff on a transient failure instead of failing immediately — see
    /// [`crate::interfaces::boot_retry`].
    ///
    /// # Errors
    /// Returns an error if the saved Telegram state cannot be read (a
    /// permissions problem, not corrupt content), the bot still can't
    /// connect after retrying, or the bus subscription fails.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        let layout = crate::workspace::layout::WorkspaceLayout::new(&self.workspace_dir);
        let (chat_store, chat_state_notice) =
            ChatStateStore::load(layout.telegram_state_json()).await?;
        if let Some(notice) = chat_state_notice {
            crate::gateway::helpers::publish_notice(&self.senders.publisher, notice).await;
        }
        let state = Arc::new(TelegramState {
            respond_to_others: self.cfg.respond_to_others,
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

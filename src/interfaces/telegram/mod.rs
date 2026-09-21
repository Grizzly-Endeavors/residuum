//! Telegram interface adapter (DM-only).
//!
//! Uses the teloxide Bot API for long-polling message reception and publishes
//! private messages onto the bus for agent processing.
//!
//! - The owner is whoever first DMs the bot; others are refused unless
//!   `[telegram] respond_to_others` is on. Commands run only for the owner.
//! - Replies go back to the chat a message came from; proactive output
//!   (scheduled results, `send_message`), notices, and errors go to the owner.

mod handler;
pub(crate) mod subscriber;

use std::path::PathBuf;

use teloxide::types::ChatId;

use crate::config::TelegramConfig;
use crate::gateway::event_loop::AdapterSenders;
use crate::interfaces::chat_state::{ChatRef, ChatStateStore};
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

/// Telegram interface adapter that routes private messages to the agent inbound channel.
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
    /// task is cancelled.
    ///
    /// # Errors
    /// Returns an error if the saved Telegram state cannot be loaded, the bot
    /// cannot connect, or the bus subscription fails.
    pub(crate) async fn start(self) -> anyhow::Result<()> {
        let layout = crate::workspace::layout::WorkspaceLayout::new(&self.workspace_dir);
        let state = std::sync::Arc::new(TelegramState {
            respond_to_others: self.cfg.respond_to_others,
            store: ChatStateStore::load(layout.telegram_state_json()).await?,
            reply_targets: ReplyTargets::default(),
        });
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

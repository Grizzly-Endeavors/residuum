//! Telegram reply handle — routes responses back to a private chat.

use async_trait::async_trait;
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use crate::channels::chunking::chunk_text;
use crate::channels::types::{ReplyHandle, TypingGuard};

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

/// Routes responses back to a Telegram private chat.
pub(super) struct TelegramReplyHandle {
    bot: Bot,
    chat_id: ChatId,
}

impl TelegramReplyHandle {
    pub(super) fn new(bot: Bot, chat_id: ChatId) -> Self {
        Self { bot, chat_id }
    }
}

#[async_trait]
impl ReplyHandle for TelegramReplyHandle {
    async fn send_response(&self, content: &str) {
        let chunks = chunk_text(content, TELEGRAM_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.bot.send_message(self.chat_id, &chunk).await {
                tracing::warn!(error = %e, "failed to send telegram message");
            }
        }
    }

    async fn send_typing(&self) {
        if let Err(e) = self
            .bot
            .send_chat_action(self.chat_id, ChatAction::Typing)
            .await
        {
            tracing::trace!(error = %e, "failed to send telegram typing indicator");
        }
    }

    async fn send_system_event(&self, source: &str, content: &str) {
        let text = format!("**[{source}]** {content}");
        let chunks = chunk_text(&text, TELEGRAM_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.bot.send_message(self.chat_id, &chunk).await {
                tracing::warn!(error = %e, "failed to send telegram system event");
            }
        }
    }

    async fn send_intermediate(&self, content: &str) {
        let chunks = chunk_text(content, TELEGRAM_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.bot.send_message(self.chat_id, &chunk).await {
                tracing::warn!(error = %e, "failed to send telegram intermediate text");
            }
        }
    }

    fn start_typing(&self) -> TypingGuard {
        let bot = self.bot.clone();
        let chat_id = self.chat_id;
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            loop {
                if let Err(e) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
                    tracing::trace!(error = %e, "telegram typing indicator send failed");
                }
                tokio::select! {
                    () = tokio::time::sleep(tokio::time::Duration::from_secs(TYPING_INTERVAL_SECS)) => {}
                    _ = stop_rx.changed() => break,
                }
            }
        });

        TypingGuard::new(stop_tx, handle)
    }
}

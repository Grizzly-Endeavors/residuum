//! Telegram bus subscriber — translates typed bus events to Telegram chat messages.

use std::sync::Arc;

use async_trait::async_trait;
use teloxide::Bot;
use teloxide::RequestError;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::chunking::chunk_text;
use crate::interfaces::outbound::ChatOutbound;

use super::TelegramState;

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;
/// Maximum caption length on a photo, audio, or document.
const MAX_CAPTION_CHARS: usize = 1024;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

struct TelegramOutbound {
    bot: Bot,
    state: Arc<TelegramState>,
}

/// Receives events from the bus and delivers them to Telegram chats.
pub(super) async fn run_telegram_subscriber(
    subs: crate::interfaces::BaseSubscribers,
    bot: Bot,
    state: Arc<TelegramState>,
) {
    crate::interfaces::outbound::run(subs, TelegramOutbound { bot, state }).await;
}

#[async_trait]
impl ChatOutbound for TelegramOutbound {
    type Target = ChatId;

    fn name(&self) -> &'static str {
        "telegram"
    }

    fn unknown_conversation_reason(&self) -> &'static str {
        "that is not a Telegram chat ID"
    }

    fn publisher(&self) -> &crate::bus::Publisher {
        &self.state.publisher
    }

    async fn reply_target(&self, correlation_id: &str) -> Option<ChatId> {
        self.state.target_for(correlation_id).await
    }

    fn release_reply(&self, correlation_id: &str) {
        self.state.reply_targets.release(correlation_id);
    }

    async fn conversation_target(&self, conversation_id: &str) -> Option<ChatId> {
        parse_chat_id(conversation_id)
    }

    async fn owner(&self) -> Option<ChatId> {
        self.state.owner_dm().await
    }

    async fn describe(&self, conversation_id: &str, _target: &ChatId) -> String {
        self.state
            .store
            .conversation(conversation_id)
            .await
            .map_or_else(|| format!("chat {conversation_id}"), |chat| chat.label)
    }

    fn target_label(&self, target: &ChatId) -> String {
        target.to_string()
    }

    fn start_typing(&self, chat_id: ChatId) -> tokio::sync::watch::Sender<()> {
        let bot = self.bot.clone();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
        tokio::spawn(async move {
            loop {
                if let Err(e) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
                    tracing::trace!(error = %e, "telegram typing indicator failed");
                }
                tokio::select! {
                    () = tokio::time::sleep(tokio::time::Duration::from_secs(TYPING_INTERVAL_SECS)) => {}
                    // Resolves with an error once the sender is dropped at turn end.
                    _ = stop_rx.changed() => break,
                }
            }
        });
        stop_tx
    }

    async fn send(
        &self,
        chat_id: &ChatId,
        content: &str,
        attachment: Option<&FileAttachment>,
    ) -> Result<(), String> {
        let sent = if let Some(attachment) = attachment {
            send_file(&self.bot, *chat_id, attachment, content).await
        } else if content.is_empty() {
            Ok(())
        } else {
            send_chunks(&self.bot, *chat_id, content).await
        };
        sent.map_err(|e| e.to_string())
    }
}

/// Parse a conversation id into a Telegram chat id, or `None` if it isn't
/// one — silently skipped by the typing indicator, which is purely
/// cosmetic; unlike a message delivery failure, there's nothing here worth
/// notifying the owner about.
fn parse_chat_id(conversation_id: &str) -> Option<ChatId> {
    conversation_id.parse::<i64>().map(ChatId).ok()
}

/// Send `content` in chunks, stopping at the first failure.
async fn send_chunks(bot: &Bot, chat_id: ChatId, content: &str) -> Result<(), RequestError> {
    for chunk in chunk_text(content, TELEGRAM_MAX_CHARS) {
        bot.send_message(chat_id, &chunk).await?;
    }
    Ok(())
}

async fn send_file(
    bot: &Bot,
    chat_id: ChatId,
    attachment: &FileAttachment,
    caption: &str,
) -> Result<(), RequestError> {
    use teloxide::payloads::{SendAudioSetters, SendDocumentSetters, SendPhotoSetters};
    use teloxide::types::InputFile;

    let file = InputFile::file(&attachment.path);
    // Telegram's caption limit counts characters, not bytes.
    let caption_too_long = caption.chars().count() > MAX_CAPTION_CHARS;
    let cap = if caption.is_empty() {
        None
    } else if caption_too_long {
        // Send the full text separately below.
        Some(caption.chars().take(MAX_CAPTION_CHARS).collect::<String>())
    } else {
        Some(caption.to_string())
    };

    if attachment.mime_type.starts_with("image/") {
        let mut req = bot.send_photo(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    } else if attachment.mime_type.starts_with("audio/") {
        let mut req = bot.send_audio(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    } else {
        let mut req = bot.send_document(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    }
    tracing::debug!(
        filename = %attachment.filename,
        endpoint = "telegram",
        "file delivered"
    );

    // If caption was truncated, send the full text separately
    if caption_too_long {
        send_chunks(bot, chat_id, caption).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chat_id_accepts_a_valid_id() {
        assert_eq!(parse_chat_id("123456789"), Some(ChatId(123_456_789)));
        assert_eq!(
            parse_chat_id("-100123456789"),
            Some(ChatId(-100_123_456_789))
        );
    }

    #[test]
    fn parse_chat_id_rejects_non_numeric() {
        assert_eq!(parse_chat_id("not-an-id"), None);
        assert_eq!(parse_chat_id(""), None);
    }
}

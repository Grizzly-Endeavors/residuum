//! Discord bus subscriber — translates typed bus events to Discord messages.

use std::sync::Arc;

use async_trait::async_trait;
use serenity::http::Http;
use serenity::model::id::ChannelId;

use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::chunking::chunk_text;
use crate::interfaces::outbound::ChatOutbound;

use super::DiscordState;

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval between typing indicator re-sends (seconds).
///
/// Discord's typing indicator lasts ~10s, so 8s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 8;

struct DiscordOutbound {
    http: Arc<Http>,
    state: Arc<DiscordState>,
}

/// Receives events from the bus and delivers them to Discord.
pub(super) async fn run_discord_subscriber(
    subs: crate::interfaces::BaseSubscribers,
    http: Arc<Http>,
    state: Arc<DiscordState>,
) {
    crate::interfaces::outbound::run(subs, DiscordOutbound { http, state }).await;
}

#[async_trait]
impl ChatOutbound for DiscordOutbound {
    type Target = ChannelId;

    fn name(&self) -> &'static str {
        "discord"
    }

    fn unknown_conversation_reason(&self) -> &'static str {
        "that is not a Discord channel ID"
    }

    fn publisher(&self) -> &crate::bus::Publisher {
        &self.state.publisher
    }

    async fn reply_target(&self, correlation_id: &str) -> Option<ChannelId> {
        self.state.target_for(correlation_id).await
    }

    fn release_reply(&self, correlation_id: &str) {
        self.state.reply_targets.release(correlation_id);
    }

    async fn conversation_target(&self, conversation_id: &str) -> Option<ChannelId> {
        parse_channel_id(conversation_id)
    }

    async fn describe(&self, _conversation_id: &str, target: &ChannelId) -> String {
        self.state
            .cached_label(*target)
            .unwrap_or_else(|| format!("channel {target}"))
    }

    fn target_label(&self, target: &ChannelId) -> String {
        target.to_string()
    }

    fn start_typing(&self, channel_id: ChannelId) -> tokio::sync::watch::Sender<()> {
        let http = Arc::clone(&self.http);
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
        crate::util::spawn_in_span(async move {
            loop {
                if let Err(e) = channel_id.broadcast_typing(&http).await {
                    tracing::trace!(error = %e, "discord typing indicator failed");
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
        channel_id: &ChannelId,
        content: &str,
        attachment: Option<&FileAttachment>,
    ) -> Result<(), String> {
        let sent = if let Some(attachment) = attachment {
            send_file_attachment(&self.http, *channel_id, attachment, content).await
        } else if content.is_empty() {
            Ok(())
        } else {
            send_chunks(&self.http, *channel_id, content).await
        };
        sent.map_err(|e| e.to_string())
    }
}

/// Parse a conversation id into a Discord channel id, or `None` if it isn't
/// one — silently skipped by the typing indicator, which is purely
/// cosmetic; unlike a message delivery failure, there's nothing here worth
/// notifying the owner about.
fn parse_channel_id(conversation_id: &str) -> Option<ChannelId> {
    match conversation_id.parse::<u64>() {
        Ok(n) if n != 0 => Some(ChannelId::new(n)),
        _ => None,
    }
}

/// Send `content` in chunks, stopping at the first failure.
async fn send_chunks(
    http: &Http,
    channel_id: ChannelId,
    content: &str,
) -> Result<(), Box<serenity::Error>> {
    for chunk in chunk_text(content, DISCORD_MAX_CHARS) {
        channel_id.say(http, &chunk).await.map_err(Box::new)?;
    }
    Ok(())
}

async fn send_file_attachment(
    http: &Http,
    channel_id: ChannelId,
    attachment: &FileAttachment,
    caption: &str,
) -> Result<(), Box<serenity::Error>> {
    use serenity::builder::{CreateAttachment, CreateMessage};

    let file_attachment = CreateAttachment::path(&attachment.path)
        .await
        .map_err(Box::new)?;
    let mut message = CreateMessage::new().add_file(file_attachment);
    if !caption.is_empty() {
        message = message.content(caption);
    }
    channel_id
        .send_message(http, message)
        .await
        .map_err(Box::new)?;
    tracing::debug!(
        filename = %attachment.filename,
        endpoint = "discord",
        "file delivered"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_channel_id_accepts_a_valid_id() {
        assert_eq!(
            parse_channel_id("123456789"),
            Some(ChannelId::new(123_456_789))
        );
    }

    #[test]
    fn parse_channel_id_rejects_zero_and_non_numeric() {
        assert_eq!(parse_channel_id("0"), None);
        assert_eq!(parse_channel_id("not-an-id"), None);
        assert_eq!(parse_channel_id(""), None);
    }
}

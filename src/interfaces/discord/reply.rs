//! Discord reply handle — routes responses back to a DM channel.

use std::sync::Arc;

use serenity::async_trait;
use serenity::model::id::ChannelId;

use crate::interfaces::chunking::chunk_text;
use crate::interfaces::types::{ReplyHandle, TypingGuard};

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval between typing indicator re-sends (seconds).
///
/// Discord's typing indicator lasts ~10s, so 8s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 8;

/// Routes responses back to a Discord DM channel.
pub(super) struct DiscordReplyHandle {
    http: Arc<serenity::http::Http>,
    channel_id: ChannelId,
}

impl DiscordReplyHandle {
    pub(super) fn new(http: Arc<serenity::http::Http>, channel_id: ChannelId) -> Self {
        Self { http, channel_id }
    }
}

#[async_trait]
impl ReplyHandle for DiscordReplyHandle {
    async fn send_response(&self, content: &str) {
        let chunks = chunk_text(content, DISCORD_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.channel_id.say(&self.http, &chunk).await {
                tracing::warn!(error = %e, "failed to send discord message");
            }
        }
    }

    async fn send_typing(&self) {
        if let Err(e) = self.channel_id.broadcast_typing(&self.http).await {
            tracing::trace!(error = %e, "failed to send discord typing indicator");
        }
    }

    async fn send_system_event(&self, source: &str, content: &str) {
        let text = format!("**[{source}]** {content}");
        let chunks = chunk_text(&text, DISCORD_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.channel_id.say(&self.http, &chunk).await {
                tracing::warn!(error = %e, "failed to send discord system event");
            }
        }
    }

    async fn send_intermediate(&self, content: &str) {
        let chunks = chunk_text(content, DISCORD_MAX_CHARS);
        for chunk in chunks {
            if let Err(e) = self.channel_id.say(&self.http, &chunk).await {
                tracing::warn!(error = %e, "failed to send discord intermediate text");
            }
        }
    }

    fn start_typing(&self) -> TypingGuard {
        let http = Arc::clone(&self.http);
        let channel_id = self.channel_id;
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            loop {
                if let Err(e) = channel_id.broadcast_typing(&http).await {
                    tracing::trace!(error = %e, "typing indicator send failed");
                }
                tokio::select! {
                    () = tokio::time::sleep(tokio::time::Duration::from_secs(TYPING_INTERVAL_SECS)) => {}
                    _ = stop_rx.changed() => break,
                }
            }
        });

        TypingGuard::new(stop_tx, handle)
    }

    fn unsolicited_clone(&self) -> Option<Arc<dyn ReplyHandle>> {
        Some(Arc::new(Self::new(Arc::clone(&self.http), self.channel_id)))
    }
}

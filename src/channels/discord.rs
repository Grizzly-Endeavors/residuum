//! Discord channel adapter (DM-only).
//!
//! Feature-gated behind `--features discord`. Implements the serenity `EventHandler`
//! trait to receive DMs and route them through the standard `RoutedMessage` pipeline.

use std::sync::Arc;

use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use tokio::sync::mpsc;

use crate::channels::chunking::chunk_text;
use crate::config::DiscordConfig;

use super::types::{InboundMessage, MessageOrigin, ReplyHandle, RoutedMessage};

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Discord channel adapter that routes DMs to the agent inbound channel.
pub struct DiscordChannel {
    cfg: DiscordConfig,
    inbound_tx: mpsc::Sender<RoutedMessage>,
}

impl DiscordChannel {
    /// Create a new Discord channel adapter.
    #[must_use]
    pub fn new(cfg: DiscordConfig, inbound_tx: mpsc::Sender<RoutedMessage>) -> Self {
        Self { cfg, inbound_tx }
    }

    /// Start the Discord gateway connection.
    ///
    /// This blocks until the connection is closed or an error occurs.
    ///
    /// # Errors
    /// Returns an error if the serenity client cannot be built or the connection fails.
    pub async fn start(self) -> Result<(), serenity::Error> {
        let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let handler = DiscordHandler {
            inbound_tx: self.inbound_tx,
        };

        let mut client = Client::builder(&self.cfg.token, intents)
            .event_handler(handler)
            .await?;

        client.start().await
    }
}

/// Serenity event handler that filters for DMs and forwards them as `RoutedMessage`.
struct DiscordHandler {
    inbound_tx: mpsc::Sender<RoutedMessage>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(bot_name = %ready.user.name, "discord bot connected");
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        // DM-only: ignore guild messages
        if msg.guild_id.is_some() {
            return;
        }

        let origin = MessageOrigin {
            channel: "discord".to_string(),
            sender_name: msg.author.name.clone(),
            sender_id: msg.author.id.to_string(),
        };

        let inbound = InboundMessage {
            id: msg.id.to_string(),
            content: msg.content.clone(),
            origin,
            timestamp: chrono::Utc::now(),
        };

        let reply = Box::new(DiscordReplyHandle {
            http: Arc::clone(&ctx.http),
            channel_id: msg.channel_id,
        });

        let routed = RoutedMessage {
            message: inbound,
            reply,
        };

        if self.inbound_tx.send(routed).await.is_err() {
            tracing::warn!("inbound channel closed, dropping discord message");
        }
    }
}

/// Routes responses back to a Discord DM channel.
struct DiscordReplyHandle {
    http: Arc<serenity::http::Http>,
    channel_id: ChannelId,
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
}

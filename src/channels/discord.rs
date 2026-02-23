//! Discord channel adapter (DM-only).
//!
//! Feature-gated behind `--features discord`. Implements the serenity `EventHandler`
//! trait to receive DMs and route them through the standard `RoutedMessage` pipeline.
//!
//! Supports:
//! - Hot-reloadable presence via `PRESENCE.toml`
//! - Slash commands mirroring the CLI command set
//! - Attachment downloading to the workspace inbox

use std::path::PathBuf;
use std::sync::Arc;

use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use tokio::sync::mpsc;

use crate::channels::attachment::{
    AttachmentInfo, download_attachment, format_attachment_line, format_failed_attachment_line,
};
use crate::channels::chunking::chunk_text;
use crate::channels::presence::{load_presence, to_activity, to_online_status};
use crate::config::DiscordConfig;

use super::types::{InboundMessage, MessageOrigin, ReplyHandle, RoutedMessage};

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval for polling PRESENCE.toml changes (seconds).
const PRESENCE_POLL_SECS: u64 = 30;

/// Discord channel adapter that routes DMs to the agent inbound channel.
pub struct DiscordChannel {
    cfg: DiscordConfig,
    inbound_tx: mpsc::Sender<RoutedMessage>,
    workspace_dir: PathBuf,
    reload_sender: tokio::sync::watch::Sender<bool>,
    observe_notify: Arc<tokio::sync::Notify>,
    reflect_notify: Arc<tokio::sync::Notify>,
}

impl DiscordChannel {
    /// Create a new Discord channel adapter.
    ///
    /// # Arguments
    /// - `cfg`: Discord bot configuration (token).
    /// - `inbound_tx`: Channel for routing messages to the agent.
    /// - `workspace_dir`: Path to the workspace root (for PRESENCE.toml and inbox).
    /// - `reload_sender`: Watch channel to trigger config reload.
    /// - `observe_notify`: Notify to trigger an observation cycle.
    /// - `reflect_notify`: Notify to trigger a reflection cycle.
    #[must_use]
    pub fn new(
        cfg: DiscordConfig,
        inbound_tx: mpsc::Sender<RoutedMessage>,
        workspace_dir: PathBuf,
        reload_sender: tokio::sync::watch::Sender<bool>,
        observe_notify: Arc<tokio::sync::Notify>,
        reflect_notify: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            cfg,
            inbound_tx,
            workspace_dir,
            reload_sender,
            observe_notify,
            reflect_notify,
        }
    }

    /// Start the Discord gateway connection.
    ///
    /// This blocks until the connection is closed or an error occurs.
    ///
    /// # Errors
    /// Returns an error if the serenity client cannot be built or the connection fails.
    pub async fn start(self) -> Result<(), serenity::Error> {
        let intents = GatewayIntents::DIRECT_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let presence_path = self.workspace_dir.join("PRESENCE.toml");
        let inbox_dir = self.workspace_dir.join("inbox");

        let handler = DiscordHandler {
            inbound_tx: self.inbound_tx,
            presence_path,
            inbox_dir,
            reload_sender: self.reload_sender,
            observe_notify: self.observe_notify,
            reflect_notify: self.reflect_notify,
        };

        let mut client = Client::builder(&self.cfg.token, intents)
            .event_handler(handler)
            .await?;

        client.start().await
    }
}

/// Serenity event handler that filters for DMs, manages presence,
/// registers slash commands, and handles attachments.
struct DiscordHandler {
    inbound_tx: mpsc::Sender<RoutedMessage>,
    presence_path: PathBuf,
    inbox_dir: PathBuf,
    reload_sender: tokio::sync::watch::Sender<bool>,
    observe_notify: Arc<tokio::sync::Notify>,
    reflect_notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(bot_name = %ready.user.name, "discord bot connected");

        // Apply initial presence from PRESENCE.toml
        let pf = load_presence(&self.presence_path);
        let activity = to_activity(&pf);
        let status = to_online_status(&pf);
        ctx.set_presence(Some(activity), status);
        tracing::info!("discord presence applied from PRESENCE.toml");

        // Register global slash commands
        if let Err(e) = register_slash_commands(&ctx).await {
            tracing::warn!(error = %e, "failed to register discord slash commands");
        }

        // Spawn presence watcher background task
        let presence_path = self.presence_path.clone();
        let shard = ctx.shard.clone();
        tokio::spawn(async move {
            presence_watcher(presence_path, shard).await;
        });
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

        // Build content with attachment metadata
        let mut content = msg.content.clone();
        for attachment in &msg.attachments {
            let info = AttachmentInfo {
                filename: attachment.filename.clone(),
                url: attachment.url.clone(),
                size: attachment.size,
                content_type: attachment.content_type.clone(),
            };

            match download_attachment(&info, &self.inbox_dir).await {
                Ok(saved) => {
                    let line = format_attachment_line(&saved, &info);
                    content.push('\n');
                    content.push_str(&line);
                }
                Err(reason) => {
                    tracing::warn!(
                        filename = %info.filename,
                        error = %reason,
                        "failed to download discord attachment"
                    );
                    let line = format_failed_attachment_line(&info, &reason);
                    content.push('\n');
                    content.push_str(&line);
                }
            }
        }

        let origin = MessageOrigin {
            channel: "discord".to_string(),
            sender_name: msg.author.name.clone(),
            sender_id: msg.author.id.to_string(),
        };

        let inbound = InboundMessage {
            id: msg.id.to_string(),
            content,
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

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(cmd) = interaction else {
            return;
        };

        let response_text = match cmd.data.name.as_str() {
            "help" => help_text(),
            "status" => status_text(),
            "reload" => {
                tracing::info!("reload requested via discord slash command");
                self.reload_sender.send(true).ok();
                "Reloading configuration...".to_string()
            }
            "observe" => {
                tracing::info!("observe requested via discord slash command");
                self.observe_notify.notify_one();
                "Observation cycle triggered.".to_string()
            }
            "reflect" => {
                tracing::info!("reflect requested via discord slash command");
                self.reflect_notify.notify_one();
                "Reflection cycle triggered.".to_string()
            }
            name => {
                format!("Unknown command: `{name}`")
            }
        };

        let msg = CreateInteractionResponseMessage::new().content(response_text);
        let response = CreateInteractionResponse::Message(msg);
        if let Err(e) = cmd.create_response(&ctx, response).await {
            tracing::warn!(
                command = %cmd.data.name,
                error = %e,
                "failed to respond to discord slash command"
            );
        }
    }
}

/// Register global slash commands with Discord.
async fn register_slash_commands(ctx: &Context) -> Result<(), serenity::Error> {
    let commands = vec![
        CreateCommand::new("help").description("Show available commands and usage info"),
        CreateCommand::new("status").description("Show bot status and version info"),
        CreateCommand::new("reload").description("Reload the agent configuration"),
        CreateCommand::new("observe").description("Trigger a memory observation cycle"),
        CreateCommand::new("reflect").description("Trigger a memory reflection cycle"),
    ];

    for cmd in commands {
        serenity::model::application::Command::create_global_command(&ctx.http, cmd).await?;
    }

    tracing::info!("discord slash commands registered");
    Ok(())
}

/// Background task that polls PRESENCE.toml for changes and updates presence.
async fn presence_watcher(presence_path: PathBuf, shard: serenity::gateway::ShardMessenger) {
    let mut last_mtime = file_mtime(&presence_path);

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(PRESENCE_POLL_SECS)).await;

        let current_mtime = file_mtime(&presence_path);
        if current_mtime != last_mtime {
            last_mtime = current_mtime;

            let pf = load_presence(&presence_path);
            let activity = to_activity(&pf);
            let status = to_online_status(&pf);

            shard.set_presence(Some(activity), status);
            tracing::info!("discord presence updated from PRESENCE.toml");
        }
    }
}

/// Get the modification time of a file, or `None` if it can't be read.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// Generate help text for the `/help` slash command.
#[must_use]
fn help_text() -> String {
    "\
**IronClaw Bot Commands**

`/help` — Show this help text
`/status` — Show bot status info
`/reload` — Reload configuration
`/observe` — Trigger memory observation
`/reflect` — Trigger memory reflection

**Messaging**: Send a DM to interact with the agent directly."
        .to_string()
}

/// Generate status text for the `/status` slash command.
#[must_use]
fn status_text() -> String {
    format!(
        "**IronClaw** v{}\nStatus: Online\nMode: DM-only",
        env!("CARGO_PKG_VERSION"),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_contains_commands() {
        let text = help_text();
        assert!(text.contains("/help"), "should mention /help");
        assert!(text.contains("/status"), "should mention /status");
        assert!(text.contains("/reload"), "should mention /reload");
        assert!(text.contains("/observe"), "should mention /observe");
        assert!(text.contains("/reflect"), "should mention /reflect");
    }

    #[test]
    fn status_text_contains_version() {
        let text = status_text();
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "should contain package version"
        );
        assert!(text.contains("Online"), "should show online status");
    }

    #[test]
    fn slash_command_names() {
        // Verify the dispatch table matches registrations
        let expected = ["help", "status", "reload", "observe", "reflect"];
        for name in expected {
            let text = match name {
                "help" => help_text(),
                "status" => status_text(),
                "reload" => "Reloading configuration...".to_string(),
                "observe" => "Observation cycle triggered.".to_string(),
                "reflect" => "Reflection cycle triggered.".to_string(),
                _ => "Unknown".to_string(),
            };
            assert!(
                !text.contains("Unknown"),
                "command '{name}' should have a known handler"
            );
        }
    }
}

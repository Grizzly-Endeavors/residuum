//! Serenity event handler, slash command registration, and presence watcher.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use tokio::sync::mpsc;

use crate::channels::attachment::{
    AttachmentInfo, download_attachment, format_attachment_line, format_failed_attachment_line,
};
use crate::channels::cli::commands::{
    CommandContext, CommandSideEffect, all_commands, execute_command,
};
use crate::channels::presence::{load_presence, to_activity, to_online_status};
use crate::channels::types::{InboundMessage, MessageOrigin, RoutedMessage};
use crate::gateway::server::ServerCommand;
use crate::inbox;

use super::reply::DiscordReplyHandle;

/// Interval for polling PRESENCE.toml changes (seconds).
const PRESENCE_POLL_SECS: u64 = 30;

/// Serenity event handler that filters for DMs, manages presence,
/// registers slash commands, and handles attachments.
pub(super) struct DiscordHandler {
    pub(super) inbound_tx: mpsc::Sender<RoutedMessage>,
    pub(super) presence_path: PathBuf,
    pub(super) inbox_dir: PathBuf,
    pub(super) reload_sender: tokio::sync::watch::Sender<bool>,
    pub(super) command_tx: mpsc::Sender<ServerCommand>,
    pub(super) tz: chrono_tz::Tz,
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

                    // Create companion inbox item for the attachment
                    let saved_name = saved
                        .local_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
                    let companion = inbox::InboxItem {
                        title: format!("Discord attachment: {}", info.filename),
                        body: format!(
                            "From: {}\nSize: {} bytes\nContent-Type: {content_type_str}",
                            msg.author.name, info.size,
                        ),
                        source: "discord".to_string(),
                        timestamp: crate::time::now_local(self.tz),
                        read: false,
                        attachments: vec![PathBuf::from("inbox").join(&saved_name)],
                    };
                    let filename = inbox::generate_filename(&companion.title, self.tz);
                    if let Err(e) = inbox::save_item(&self.inbox_dir, &filename, &companion).await {
                        tracing::warn!(
                            filename = %info.filename,
                            error = %e,
                            "failed to create companion inbox item for discord attachment"
                        );
                    }
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

        let reply = Arc::new(DiscordReplyHandle::new(
            Arc::clone(&ctx.http),
            msg.channel_id,
        ));

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

        // Extract optional text argument from Discord interaction options
        let cmd_args = cmd
            .data
            .options
            .first()
            .and_then(|opt| opt.value.as_str())
            .map(str::to_string);

        let command_ctx = CommandContext {
            url: "",
            verbose: false,
            channel_name: "discord",
        };

        let result = execute_command(cmd.data.name.as_str(), cmd_args.as_deref(), &command_ctx);

        // Handle side effects
        let response_text = match result.side_effect {
            Some(CommandSideEffect::Reload) => {
                tracing::info!("reload requested via discord slash command");
                self.reload_sender.send(true).ok();
                result.response
            }
            Some(CommandSideEffect::ServerCommand { name, args }) => {
                tracing::info!(command = %name, "server command via discord slash command");
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                self.command_tx
                    .try_send(ServerCommand {
                        name: name.to_string(),
                        args,
                        reply_tx: Some(reply_tx),
                    })
                    .ok();
                match tokio::time::timeout(Duration::from_secs(10), reply_rx).await {
                    Ok(Ok(msg)) => msg,
                    _ => result.response,
                }
            }
            Some(CommandSideEffect::InboxAdd(body)) => {
                let title: String = body
                    .lines()
                    .next()
                    .unwrap_or("Inbox message")
                    .chars()
                    .take(60)
                    .collect();
                let source = format!("discord:{}", cmd.user.name);
                match inbox::quick_add(&self.inbox_dir, &title, &body, &source, self.tz).await {
                    Ok(_) => result.response,
                    Err(e) => format!("failed to add inbox item: {e}"),
                }
            }
            Some(CommandSideEffect::Quit | CommandSideEffect::ToggleVerbose) => {
                // Not applicable to Discord
                result.response
            }
            None => result.response,
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

/// Register global slash commands with Discord from the shared command registry.
///
/// Commands that take arguments (like `/inbox`) get a `text` string option.
/// Client-only commands (quit, verbose) are skipped — they don't apply to Discord.
async fn register_slash_commands(ctx: &Context) -> Result<(), serenity::Error> {
    let skip = ["quit", "exit", "q", "verbose", "v"];

    for info in all_commands() {
        if skip.contains(&info.name) {
            continue;
        }

        let mut cmd = CreateCommand::new(info.name).description(info.help);
        if info.takes_arg {
            cmd = cmd.add_option(
                serenity::builder::CreateCommandOption::new(
                    serenity::all::CommandOptionType::String,
                    "text",
                    "Text argument",
                )
                .required(true),
            );
        }

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

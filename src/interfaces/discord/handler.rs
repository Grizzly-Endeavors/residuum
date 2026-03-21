//! Serenity event handler, slash command registration, and presence watcher.

use std::path::PathBuf;
use std::sync::Arc;

use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use crate::bus::{BusHandle, EndpointName, Publisher};
use crate::gateway::types::{ReloadSignal, ServerCommand};
use crate::inbox;
use crate::interfaces::attachment::{
    AttachmentInfo, MAX_IMAGE_INLINE_SIZE, download_attachment, encode_image_from_file,
    format_attachment_line, format_failed_attachment_line, is_supported_image,
};
use crate::interfaces::cli::commands::{
    CommandContext, CommandSideEffect, all_commands, execute_command,
};
use crate::interfaces::presence::{load_presence, to_activity, to_online_status};
use crate::interfaces::types::MessageOrigin;
use crate::models::ImageData;

/// Interval for polling PRESENCE.toml changes (seconds).
const PRESENCE_POLL_SECS: u64 = 30;

/// Serenity event handler that filters for DMs, manages presence,
/// registers slash commands, and handles attachments.
pub(super) struct DiscordHandler {
    pub(super) publisher: Publisher,
    pub(super) bus_handle: BusHandle,
    pub(super) channel_id: Arc<tokio::sync::Mutex<Option<serenity::model::id::ChannelId>>>,
    pub(super) presence_path: PathBuf,
    pub(super) inbox_dir: PathBuf,
    pub(super) reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub(super) command_tx: tokio::sync::mpsc::Sender<ServerCommand>,
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

        // Subscribe to typed bus topics and spawn subscriber loop
        match super::subscriber::DiscordSubscribers::new(
            &self.bus_handle,
            EndpointName::from("discord"),
        )
        .await
        {
            Ok(subs) => {
                let h = Arc::clone(&ctx.http);
                let cid = Arc::clone(&self.channel_id);
                tokio::spawn(super::subscriber::run_discord_subscriber(subs, h, cid));
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to subscribe to discord bus topics");
            }
        }

        // Spawn presence watcher background task
        let presence_path = self.presence_path.clone();
        let shard = ctx.shard.clone();
        tokio::spawn(async move {
            presence_watcher(presence_path, shard).await;
        });
    }

    async fn message(&self, _ctx: Context, msg: Message) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        // DM-only: ignore guild messages
        if msg.guild_id.is_some() {
            return;
        }

        tracing::debug!(author = %msg.author.name, content_len = msg.content.len(), "discord DM received");

        // Track the DM channel for subscriber output
        {
            let mut cid = self.channel_id.lock().await;
            if cid.is_none() {
                *cid = Some(msg.channel_id);
                tracing::debug!(channel_id = %msg.channel_id, "discord DM channel tracked");
            }
        }

        // Build content with attachment metadata and collect inline images
        let (content, images) = process_discord_attachments(
            &msg.attachments,
            msg.content.clone(),
            &msg.author.name,
            &self.inbox_dir,
            self.tz,
        )
        .await;

        let origin = MessageOrigin {
            endpoint: "discord".to_string(),
            sender_name: msg.author.name.clone(),
            sender_id: msg.author.id.to_string(),
        };

        let msg_event = crate::bus::MessageEvent {
            id: msg.id.to_string(),
            content,
            origin,
            timestamp: crate::time::now_local(self.tz),
            images,
        };

        if let Err(e) = self
            .publisher
            .publish(crate::bus::topics::UserMessage, msg_event)
            .await
        {
            tracing::warn!(error = %e, "failed to publish discord message to bus");
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
        };

        let result = execute_command(cmd.data.name.as_str(), cmd_args.as_deref(), &command_ctx);

        // Handle side effects
        let response_text = match result.side_effect {
            Some(CommandSideEffect::Reload) => {
                tracing::info!("reload requested via discord slash command");
                if self.reload_tx.send(ReloadSignal::Root).is_err() {
                    tracing::warn!("reload_tx closed, reload dropped");
                }
                result.response
            }
            Some(CommandSideEffect::ServerCommand { name, args }) => {
                crate::interfaces::dispatch_server_command(
                    &self.command_tx,
                    name,
                    args,
                    result.response,
                    "discord slash command",
                )
                .await
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

/// Download and process all Discord attachments, encoding supported images inline.
async fn process_discord_attachments(
    attachments: &[serenity::model::channel::Attachment],
    mut content: String,
    author_name: &str,
    inbox_dir: &std::path::Path,
    tz: chrono_tz::Tz,
) -> (String, Vec<ImageData>) {
    let mut images: Vec<ImageData> = Vec::new();

    for attachment in attachments {
        let info = AttachmentInfo {
            filename: attachment.filename.clone(),
            size: attachment.size,
            content_type: attachment.content_type.clone(),
        };

        match download_attachment(&info, &attachment.url, inbox_dir).await {
            Ok(saved) => {
                let line = format_attachment_line(&saved, &info);
                content.push('\n');
                content.push_str(&line);

                // Encode supported images inline for the model
                if is_supported_image(info.content_type.as_deref())
                    && info.size <= MAX_IMAGE_INLINE_SIZE
                {
                    match encode_image_from_file(
                        &saved.local_path,
                        info.content_type.as_deref().unwrap_or("image/jpeg"),
                    )
                    .await
                    {
                        Ok(img) => images.push(img),
                        Err(e) => tracing::warn!(
                            filename = %info.filename,
                            error = %e,
                            "failed to encode discord image for inline delivery"
                        ),
                    }
                } else if is_supported_image(info.content_type.as_deref()) {
                    tracing::warn!(
                        filename = %info.filename,
                        size = info.size,
                        max = MAX_IMAGE_INLINE_SIZE,
                        "discord image exceeds inline size limit, saved but not sent to model"
                    );
                }

                // Create companion inbox item for the attachment
                let Some(file_name_os) = saved.local_path.file_name() else {
                    tracing::warn!(path = %saved.local_path.display(), "attachment path has no filename, skipping companion item");
                    continue;
                };
                let saved_name = file_name_os.to_string_lossy().to_string();
                let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
                let companion = inbox::InboxItem {
                    title: format!("Discord attachment: {}", info.filename),
                    body: format!(
                        "From: {author_name}\nSize: {} bytes\nContent-Type: {content_type_str}",
                        info.size,
                    ),
                    source: "discord".to_string(),
                    timestamp: crate::time::now_local(tz),
                    read: false,
                    attachments: vec![PathBuf::from("inbox").join(&saved_name)],
                };
                let filename = inbox::generate_filename(&companion.title, companion.timestamp);
                if let Err(e) = inbox::save_item(inbox_dir, &filename, &companion).await {
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

    (content, images)
}

/// Register global slash commands with Discord from the shared command registry.
///
/// Commands that take arguments (like `/inbox`) get a `text` string option.
/// Client-only commands (quit, verbose) are skipped — they don't apply to Discord.
async fn register_slash_commands(ctx: &Context) -> Result<(), serenity::Error> {
    for info in all_commands().filter(|c| !c.cli_only) {
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
            tracing::info!(status = ?status, activity_type = ?pf.activity_type, "discord presence updated from PRESENCE.toml");
        }
    }
}

/// Get the modification time of a file, or `None` if it can't be read.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

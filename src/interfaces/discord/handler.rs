//! Serenity event handler and slash command registration.

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
use crate::gateway::types::{ReloadSignal, ServerCommand, StopRequest};
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::{
    AttachmentInfo, download_attachment, finalize_attachment, format_failed_attachment_line,
};
use crate::interfaces::commands::all_commands;
use crate::interfaces::types::MessageOrigin;

/// Serenity event handler that filters for DMs, registers slash commands,
/// and handles attachments.
pub(super) struct DiscordHandler {
    pub(super) publisher: Publisher,
    pub(super) bus_handle: BusHandle,
    pub(super) channel_id: Arc<tokio::sync::Mutex<Option<serenity::model::id::ChannelId>>>,
    pub(super) inbox_dir: PathBuf,
    pub(super) reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub(super) command_tx: tokio::sync::mpsc::Sender<ServerCommand>,
    pub(super) stop_tx: tokio::sync::mpsc::Sender<StopRequest>,
    pub(super) tz: chrono_tz::Tz,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(bot_name = %ready.user.name, "discord bot connected");

        // Register global slash commands
        if let Err(e) = register_commands(&ctx).await {
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

        {
            let _span = tracing::debug_span!("discord_message", author = %msg.author.name, msg_id = %msg.id).entered();
            tracing::debug!(author = %msg.author.name, content_len = msg.content.len(), "discord DM received");
        }

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
            sender: Some(MessageSender {
                name: msg.author.name.clone(),
                id: msg.author.id.to_string(),
                interface: "discord".to_string(),
                location: Some("direct message".to_string()),
            }),
        };

        let msg_event = crate::bus::MessageEvent {
            id: msg.id.to_string(),
            content,
            origin,
            timestamp: crate::time::now_local(self.tz),
            images,
            context: None,
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
        tracing::debug!(command = %cmd.data.name, "discord slash command received");

        // Extract optional text argument from Discord interaction options
        let cmd_args = cmd
            .data
            .options
            .first()
            .and_then(|opt| opt.value.as_str())
            .map(str::to_string);

        let dispatch = crate::interfaces::CommandDispatch {
            reload_tx: &self.reload_tx,
            command_tx: &self.command_tx,
            stop_tx: &self.stop_tx,
            inbox_dir: &self.inbox_dir,
            tz: self.tz,
        };
        let response_text = crate::interfaces::run_chat_command(
            cmd.data.name.as_str(),
            cmd_args.as_deref(),
            &dispatch,
            "discord",
            &cmd.user.name,
        )
        .await;

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
                if let Some(img) = finalize_attachment(
                    &saved,
                    &info,
                    &mut content,
                    author_name,
                    inbox_dir,
                    tz,
                    "Discord",
                )
                .await
                {
                    images.push(img);
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
async fn register_commands(ctx: &Context) -> Result<(), Box<serenity::Error>> {
    for info in all_commands() {
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

        serenity::model::application::Command::create_global_command(&ctx.http, cmd)
            .await
            .map_err(Box::new)?;
    }

    tracing::info!("discord slash commands registered");
    Ok(())
}

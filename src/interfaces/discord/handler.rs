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
use serenity::model::user::User;
use serenity::prelude::*;

use crate::bus::Publisher;
use crate::gateway::types::{ReloadSignal, ServerCommand, StopRequest};
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::{
    AttachmentInfo, download_attachment, finalize_attachment, format_failed_attachment_line,
};
use crate::interfaces::chat_state::{ChatRef, Owner, Standing};
use crate::interfaces::commands::all_commands;
use crate::interfaces::types::MessageOrigin;

use super::DiscordState;

/// Serenity event handler that filters for DMs, enforces who may use the
/// bot, registers slash commands, and handles attachments.
pub(super) struct DiscordHandler {
    pub(super) state: Arc<DiscordState>,
    pub(super) publisher: Publisher,
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

        {
            let _span = tracing::debug_span!("discord_message", author = %msg.author.name, msg_id = %msg.id).entered();
            tracing::debug!(author = %msg.author.name, content_len = msg.content.len(), "discord DM received");
        }

        let channel_key = msg.channel_id.to_string();
        if let Err(e) = self
            .state
            .store
            .remember(&channel_key, ChatRef::direct_message())
            .await
        {
            tracing::warn!(error = %e, channel_id = %msg.channel_id, "failed to save discord conversation");
        }
        claim_owner_if_unset(&self.state, &msg.author, &channel_key).await;

        let sender_id = msg.author.id.to_string();
        if let Err(refusal) = self
            .state
            .store
            .admit(Some(&sender_id), self.state.respond_to_others)
            .await
        {
            tracing::info!(
                sender = %msg.author.name,
                "discord message from someone other than the owner; respond_to_others is off"
            );
            if let Err(e) = msg.channel_id.say(&ctx.http, refusal).await {
                tracing::warn!(error = %e, channel_id = %msg.channel_id, "failed to send discord refusal");
            }
            return;
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
            endpoint: super::ENDPOINT.to_string(),
            sender: Some(MessageSender {
                name: msg.author.name.clone(),
                id: sender_id,
                interface: super::ENDPOINT.to_string(),
                location: Some("direct message".to_string()),
            }),
        };

        let correlation_id = msg.id.to_string();
        self.state
            .reply_targets
            .track(&correlation_id, msg.channel_id);
        let msg_event = crate::bus::MessageEvent {
            id: correlation_id,
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
            tracing::error!(error = %e, "failed to publish discord message to bus");
            if let Err(reply_err) = msg
                .channel_id
                .say(
                    &ctx.http,
                    "Something went wrong handing your message to the agent. Please try again.",
                )
                .await
            {
                tracing::warn!(error = %reply_err, channel_id = %msg.channel_id, "failed to send discord error reply");
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(cmd) = interaction else {
            return;
        };
        tracing::debug!(command = %cmd.data.name, "discord slash command received");

        let standing = self
            .state
            .store
            .standing_of(Some(&cmd.user.id.to_string()))
            .await;
        if !matches!(standing, Standing::Owner) {
            tracing::info!(command = %cmd.data.name, user = %cmd.user.name, "refused discord slash command from someone other than the owner");
            let msg = CreateInteractionResponseMessage::new()
                .content("Only my owner can run commands.")
                .ephemeral(true);
            if let Err(e) = cmd
                .create_response(&ctx, CreateInteractionResponse::Message(msg))
                .await
            {
                tracing::warn!(command = %cmd.data.name, error = %e, "failed to refuse discord slash command");
            }
            return;
        }

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
            super::ENDPOINT,
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

/// Make the sender the owner if nobody is yet; only called for DMs.
async fn claim_owner_if_unset(state: &DiscordState, author: &User, dm_channel: &str) {
    let owner = Owner {
        user_id: author.id.to_string(),
        name: author.name.clone(),
        dm_conversation_id: dm_channel.to_string(),
    };
    match state.store.claim_owner(owner).await {
        Ok(true) => {
            tracing::info!(owner = %author.name, "discord owner set from first direct message");
        }
        Ok(false) => {}
        Err(e) => tracing::error!(error = %e, "failed to save discord owner"),
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

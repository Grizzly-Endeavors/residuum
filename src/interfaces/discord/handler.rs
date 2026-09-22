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
use serenity::model::id::ChannelId;
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
use crate::interfaces::context_buffer::{BufferedMessage, render_context};
use crate::interfaces::types::{ConversationContext, ConversationKind, MessageOrigin};

use super::DiscordState;
use super::channels::{guild_channel_label, strip_bot_mention};

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
        if self.state.bot_id.set(ready.user.id).is_err() {
            tracing::debug!("discord reconnected; bot user already known");
        }

        // Register global slash commands
        if let Err(e) = register_commands(&ctx).await {
            tracing::warn!(error = %e, "failed to register discord slash commands");
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }
        let Some(addressed) = self.addressed_to_agent(&ctx, &msg).await else {
            return;
        };
        {
            let _span = tracing::debug_span!("discord_message", author = %msg.author.name, msg_id = %msg.id).entered();
            tracing::debug!(author = %msg.author.name, location = %addressed.location, content_len = addressed.text.len(), "discord message received");
        }

        let standing = match self
            .state
            .store
            .admit(
                Some(&msg.author.id.to_string()),
                self.state.respond_to_others,
            )
            .await
        {
            Ok(standing) => standing,
            Err(refusal) => {
                tracing::info!(
                    sender = %msg.author.name,
                    location = %addressed.location,
                    "discord message from someone other than the owner; respond_to_others is off"
                );
                say_or_warn(&ctx, msg.channel_id, &refusal).await;
                return;
            }
        };

        // Build content with attachment metadata and collect inline images
        let (content, images) = process_discord_attachments(
            &msg.attachments,
            addressed.text,
            &msg.author.name,
            &self.inbox_dir,
            self.tz,
        )
        .await;

        let correlation_id = msg.id.to_string();
        self.state
            .reply_targets
            .track(&correlation_id, msg.channel_id);
        let msg_event = crate::bus::MessageEvent {
            id: correlation_id,
            content,
            origin: MessageOrigin {
                endpoint: super::ENDPOINT.to_string(),
                sender: Some(MessageSender {
                    name: msg.author.name.clone(),
                    id: msg.author.id.to_string(),
                    interface: super::ENDPOINT.to_string(),
                    location: Some(addressed.location),
                }),
                conversation: Some(ConversationContext {
                    id: addressed.conversation_id,
                    kind: addressed.kind,
                    is_owner: matches!(standing, Standing::Owner),
                }),
            },
            timestamp: crate::time::now_local(self.tz),
            images,
            context: addressed.context,
        };

        if let Err(e) = self
            .publisher
            .publish(crate::bus::topics::UserMessage, msg_event)
            .await
        {
            tracing::error!(error = %e, "failed to publish discord message to bus");
            say_or_warn(
                &ctx,
                msg.channel_id,
                "Something went wrong handing your message to the agent. Please try again.",
            )
            .await;
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

        // Command output can carry internals; in a server only the owner sees it.
        let msg = CreateInteractionResponseMessage::new()
            .content(response_text)
            .ephemeral(cmd.guild_id.is_some());
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

/// A message the bot has decided to act on: where it happened, its text
/// (mention stripped), its conversation identity, and any unmentioned
/// chatter buffered since the last mention there.
struct Addressed {
    location: String,
    text: String,
    conversation_id: String,
    kind: ConversationKind,
    context: Option<String>,
}

impl DiscordHandler {
    /// Where a message was sent and its text, if it is meant for the agent:
    /// every DM, and server messages that @mention the bot (mention removed).
    /// DMs are also recorded, and the first one claims ownership. An
    /// unmentioned server message is buffered as context instead, and `None`
    /// is returned.
    async fn addressed_to_agent(&self, ctx: &Context, msg: &Message) -> Option<Addressed> {
        let Some(guild_id) = msg.guild_id else {
            let channel_key = msg.channel_id.to_string();
            if let Err(e) = self
                .state
                .store
                .remember(&channel_key, ChatRef::direct_message(&msg.author.name))
                .await
            {
                tracing::warn!(error = %e, channel_id = %msg.channel_id, "failed to save discord conversation");
            }
            claim_owner_if_unset(&self.state, &msg.author, &channel_key).await;
            return Some(Addressed {
                location: "direct message".to_string(),
                text: msg.content.clone(),
                conversation_id: channel_key,
                kind: ConversationKind::Personal,
                context: None,
            });
        };
        let Some(&bot_id) = self.state.bot_id.get() else {
            tracing::debug!("discord server message before the connection was ready, ignoring");
            return None;
        };
        let channel_key = msg.channel_id.to_string();
        if !msg.mentions.iter().any(|user| user.id == bot_id) {
            buffer_unmentioned(&self.state, &channel_key, msg, self.tz);
            return None;
        }
        let label = guild_channel_label(&self.state, &ctx.http, guild_id, msg.channel_id).await;
        let context = render_context(&label, &self.state.context_buffer.drain(&channel_key));
        Some(Addressed {
            location: label,
            text: strip_bot_mention(&msg.content, bot_id),
            conversation_id: channel_key,
            kind: ConversationKind::Channel,
            context,
        })
    }
}

/// Hold an unmentioned server message as context for the next @mention in
/// that channel; empty messages (e.g. attachment-only) are dropped.
fn buffer_unmentioned(state: &DiscordState, channel_key: &str, msg: &Message, tz: chrono_tz::Tz) {
    if msg.content.trim().is_empty() {
        return;
    }
    state.context_buffer.record(
        channel_key,
        BufferedMessage {
            sender: msg.author.name.clone(),
            text: msg.content.clone(),
            at: crate::time::now_local(tz),
        },
    );
}

async fn say_or_warn(ctx: &Context, channel_id: ChannelId, text: &str) {
    if let Err(e) = channel_id.say(&ctx.http, text).await {
        tracing::warn!(error = %e, %channel_id, "failed to send discord reply");
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

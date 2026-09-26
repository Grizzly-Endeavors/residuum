//! Serenity event handler and slash command registration.

use std::path::PathBuf;
use std::sync::Arc;

use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::http::Http;
use serenity::model::application::Interaction;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::model::user::User;
use serenity::prelude::*;

use crate::background::registry::SessionRegistry;
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
///
/// `Clone` so the client-build retry in [`super::DiscordInterface::start`]
/// can hand a fresh clone to each attempt — `Client::builder(..)
/// .event_handler(handler)` consumes it by value.
#[derive(Clone)]
pub(super) struct DiscordHandler {
    pub(super) state: Arc<DiscordState>,
    pub(super) publisher: Publisher,
    pub(super) inbox_dir: PathBuf,
    pub(super) reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    pub(super) command_tx: tokio::sync::mpsc::Sender<ServerCommand>,
    pub(super) stop_tx: tokio::sync::mpsc::Sender<StopRequest>,
    pub(super) session_registry: Arc<SessionRegistry>,
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
        let Some(addressed) = self.addressed_to_agent(&ctx.http, &msg).await else {
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
                agent_sender: None,
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

        // A slash command carries no @mention, so its conversation identity
        // is derived the same way `addressed_to_agent` derives it for an
        // ordinary message: no guild means a DM.
        let conversation = ConversationContext {
            id: cmd.channel_id.to_string(),
            kind: if cmd.guild_id.is_some() {
                ConversationKind::Channel
            } else {
                ConversationKind::Personal
            },
            is_owner: matches!(standing, Standing::Owner),
        };
        let dispatch = crate::interfaces::CommandDispatch {
            reload_tx: &self.reload_tx,
            command_tx: &self.command_tx,
            stop_tx: &self.stop_tx,
            session_registry: &self.session_registry,
            inbox_dir: &self.inbox_dir,
            tz: self.tz,
        };
        let response_text = crate::interfaces::run_chat_command(
            cmd.data.name.as_str(),
            cmd_args.as_deref(),
            &dispatch,
            super::ENDPOINT,
            &cmd.user.name,
            Some(&conversation),
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
    async fn addressed_to_agent(&self, http: &Http, msg: &Message) -> Option<Addressed> {
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
        let label = guild_channel_label(&self.state, http, guild_id, msg.channel_id).await;
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
                .required(info.arg_required),
            );
        }

        serenity::model::application::Command::create_global_command(&ctx.http, cmd)
            .await
            .map_err(Box::new)?;
    }

    tracing::info!("discord slash commands registered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use serenity::model::id::{ChannelId, UserId};

    use super::*;
    use crate::interfaces::chat_state::ChatStateStore;
    use crate::interfaces::context_buffer::ContextBuffer;
    use crate::interfaces::reply_targets::ReplyTargets;

    async fn state(context_messages: usize) -> (Arc<DiscordState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(DiscordState {
            respond_to_others: false,
            store: ChatStateStore::load(dir.path().join("discord_state.json"))
                .await
                .unwrap()
                .0,
            reply_targets: ReplyTargets::default(),
            bot_id: OnceLock::new(),
            channel_labels: Mutex::new(HashMap::new()),
            context_buffer: ContextBuffer::new(context_messages),
            publisher: crate::bus::spawn_broker().publisher(),
        });
        (state, dir)
    }

    fn handler(state: Arc<DiscordState>) -> DiscordHandler {
        DiscordHandler {
            state,
            publisher: crate::bus::spawn_broker().publisher(),
            inbox_dir: std::env::temp_dir(),
            reload_tx: tokio::sync::watch::channel(ReloadSignal::Root).0,
            command_tx: tokio::sync::mpsc::channel(1).0,
            stop_tx: tokio::sync::mpsc::channel(1).0,
            session_registry: Arc::new(SessionRegistry::new()),
            tz: chrono_tz::UTC,
        }
    }

    /// Shared server/channel/bot ids for the guild-message fixtures below.
    const GUILD_ID: u64 = 1000;
    const CHANNEL_ID: u64 = 5000;
    const BOT_ID: u64 = 999;

    /// `Message` is `#[non_exhaustive]`, so serenity fixtures are built the
    /// way the gateway itself produces them: deserialized from JSON.
    fn discord_message(json: serde_json::Value) -> Message {
        serde_json::from_value(json).expect("valid minimal discord message fixture")
    }

    fn dm(msg_id: u64, author_id: u64, author_name: &str, content: &str) -> Message {
        discord_message(serde_json::json!({
            "id": msg_id,
            "channel_id": author_id,
            "author": {"id": author_id, "username": author_name},
            "content": content,
            "timestamp": "2024-01-01T00:00:00.000Z",
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0,
        }))
    }

    /// An unmentioned server channel message from `author`.
    fn chatter(msg_id: u64, author_id: u64, author_name: &str, content: &str) -> Message {
        discord_message(serde_json::json!({
            "id": msg_id,
            "channel_id": CHANNEL_ID,
            "guild_id": GUILD_ID,
            "author": {"id": author_id, "username": author_name},
            "content": content,
            "timestamp": "2024-01-01T00:00:00.000Z",
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0,
        }))
    }

    /// A server channel message from `author` that @mentions the bot.
    fn mention(msg_id: u64, author_id: u64, author_name: &str, content: &str) -> Message {
        discord_message(serde_json::json!({
            "id": msg_id,
            "channel_id": CHANNEL_ID,
            "guild_id": GUILD_ID,
            "author": {"id": author_id, "username": author_name},
            "content": content,
            "timestamp": "2024-01-01T00:00:00.000Z",
            "tts": false,
            "mention_everyone": false,
            "mentions": [{"id": BOT_ID, "username": "ResiBot"}],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0,
        }))
    }

    #[tokio::test]
    async fn dm_populates_personal_conversation_and_claims_owner() {
        let (state, _dir) = state(10).await;
        let h = handler(Arc::clone(&state));
        let http = Http::new("test-token");
        let msg = dm(1, 111, "Bear", "hello");

        let addressed = h
            .addressed_to_agent(&http, &msg)
            .await
            .expect("dm is always addressed");
        assert_eq!(addressed.location, "direct message");
        assert_eq!(addressed.text, "hello");
        assert_eq!(addressed.conversation_id, "111");
        assert_eq!(addressed.kind, ConversationKind::Personal);
        assert_eq!(addressed.context, None);
        assert!(
            matches!(state.store.standing_of(Some("111")).await, Standing::Owner),
            "first DM sender becomes the owner"
        );
    }

    #[tokio::test]
    async fn unmentioned_server_message_is_buffered_not_addressed() {
        let (state, _dir) = state(10).await;
        state.bot_id.set(UserId::new(BOT_ID)).unwrap();
        let h = handler(Arc::clone(&state));
        let http = Http::new("test-token");
        let msg = chatter(2, 222, "Sam", "build is red");

        assert!(h.addressed_to_agent(&http, &msg).await.is_none());
    }

    #[tokio::test]
    async fn mention_carries_the_chatter_since_the_last_mention() {
        let (state, _dir) = state(10).await;
        state.bot_id.set(UserId::new(BOT_ID)).unwrap();
        state.cache_label(ChannelId::new(CHANNEL_ID), "#builds (Eng)".to_string());
        let h = handler(Arc::clone(&state));
        let http = Http::new("test-token");

        let unmentioned = chatter(2, 222, "Sam", "build is red");
        assert!(
            h.addressed_to_agent(&http, &unmentioned).await.is_none(),
            "unmentioned chatter is buffered, not addressed"
        );

        let mentioned = mention(3, 333, "Bear", "<@999> can you look?");
        let addressed = h
            .addressed_to_agent(&http, &mentioned)
            .await
            .expect("mention is addressed");
        assert_eq!(addressed.text, "can you look?");
        assert_eq!(addressed.kind, ConversationKind::Channel);
        assert_eq!(addressed.conversation_id, CHANNEL_ID.to_string());
        let context = addressed.context.expect("buffered chatter attached");
        assert!(context.contains("Sam: build is red"), "{context}");

        let mentioned_again = mention(4, 333, "Bear", "<@999> thanks");
        let addressed_again = h
            .addressed_to_agent(&http, &mentioned_again)
            .await
            .expect("second mention is addressed");
        assert_eq!(
            addressed_again.context, None,
            "already-delivered chatter is not repeated"
        );
    }

    #[tokio::test]
    async fn context_buffer_is_bounded_by_context_messages() {
        let (state, _dir) = state(1).await;
        state.bot_id.set(UserId::new(BOT_ID)).unwrap();
        state.cache_label(ChannelId::new(CHANNEL_ID), "#builds".to_string());
        let h = handler(Arc::clone(&state));
        let http = Http::new("test-token");

        h.addressed_to_agent(&http, &chatter(2, 222, "Sam", "first"))
            .await;
        h.addressed_to_agent(&http, &chatter(3, 222, "Sam", "second"))
            .await;

        let mentioned = mention(4, 333, "Bear", "<@999> hi");
        let addressed = h.addressed_to_agent(&http, &mentioned).await.unwrap();
        let context = addressed.context.expect("one buffered message fits");
        assert!(!context.contains("first"), "{context}");
        assert!(context.contains("second"), "{context}");
    }
}

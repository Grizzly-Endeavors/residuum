//! Telegram long-polling message handler and command dispatch.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

use teloxide::Bot;
use teloxide::payloads::GetUpdatesSetters;
use teloxide::requests::Requester;
use teloxide::types::{
    Audio, BotCommand, ChatId, ChatMemberUpdated, Document, PhotoSize, UpdateKind, UserId, Video,
    Voice,
};

use crate::bus::{EndpointName, Publisher};
use crate::gateway::event_loop::AdapterSenders;
use crate::gateway::types::{ReloadSignal, ServerCommand, StopRequest};
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::chat_state::{ChatRef, Owner, Standing};
use crate::interfaces::commands::all_commands;
use crate::interfaces::context_buffer::{BufferedMessage, render_context};
use crate::interfaces::types::{ConversationContext, ConversationKind, MessageOrigin};

use super::groups::{addressed_text, group_label};
use super::{ENDPOINT, TelegramState};

/// Shared gateway references threaded through telegram message dispatch.
struct TelegramContext<'a> {
    state: &'a TelegramState,
    /// The bot's `@username`, without the `@`, for spotting mentions in groups.
    bot_username: &'a str,
    bot_id: UserId,
    publisher: &'a Publisher,
    inbox_dir: &'a Path,
    reload_tx: &'a tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: &'a tokio::sync::mpsc::Sender<ServerCommand>,
    stop_tx: &'a tokio::sync::mpsc::Sender<StopRequest>,
    tz: chrono_tz::Tz,
}

/// Metadata for a Telegram file attachment.
struct AttachmentMeta<'a> {
    file_id: &'a str,
    filename: &'a str,
    size: u32,
    content_type: Option<String>,
}

/// Run the Telegram long-polling loop.
///
/// Connects to the Telegram API, verifies the bot token, then enters an
/// infinite polling loop that dispatches messages to the agent. Returns
/// cleanly when the shutdown signal fires.
///
/// # Errors
/// Returns an error if the initial `get_me` verification or the bus
/// subscription fails.
pub(super) async fn run_telegram_polling(
    token: &str,
    state: Arc<TelegramState>,
    senders: AdapterSenders,
    workspace_dir: std::path::PathBuf,
    tz: chrono_tz::Tz,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let publisher = senders.publisher;
    let bus_handle = senders.bus_handle;
    let reload_tx = senders.reload;
    let command_tx = senders.command;
    let stop_tx = senders.stop;
    // TCP keepalive detects silently-dropped connections (e.g. NAT timeout);
    // pool_idle_timeout evicts stale connections before they poison the pool.
    // Without these, long-poll requests reuse dead connections indefinitely.
    let http_client = teloxide::net::default_reqwest_settings()
        .tcp_keepalive(Duration::from_mins(1))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;
    let bot = Bot::with_client(token, http_client);
    let inbox_dir =
        crate::workspace::layout::WorkspaceLayout::new(&workspace_dir).agent_inbox_dir();

    // Verify the bot token is valid
    let me = bot.get_me().await?;
    tracing::info!(
        bot_name = %me.first_name,
        bot_username = %me.username(),
        "telegram bot connected"
    );

    if let Err(e) = register_commands(&bot).await {
        tracing::warn!(error = %e, "failed to register telegram bot commands");
    }

    let subs = crate::interfaces::BaseSubscribers::new(&bus_handle, EndpointName::from(ENDPOINT))
        .await
        .context("failed to subscribe to telegram bus topics")?;
    let _outbound = OutboundTask(tokio::spawn(super::subscriber::run_telegram_subscriber(
        subs,
        bot.clone(),
        Arc::clone(&state),
    )));

    let mut offset: i32 = 0;
    let mut consecutive_errors: u32 = 0;

    loop {
        let updates = tokio::select! {
            result = bot.get_updates().offset(offset).timeout(30) => {
                match result {
                    Ok(updates) => {
                        if consecutive_errors > 0 {
                            tracing::info!(
                                attempts = consecutive_errors,
                                "telegram polling recovered"
                            );
                            consecutive_errors = 0;
                        }
                        updates
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        if consecutive_errors == 1 {
                            tracing::warn!(error = %e, "telegram polling error, retrying");
                        } else {
                            tracing::debug!(
                                error = %e,
                                attempt = consecutive_errors,
                                "telegram polling still failing"
                            );
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                tracing::info!("telegram adapter received shutdown signal");
                return Ok(());
            }
        };

        let ctx = TelegramContext {
            state: &state,
            bot_username: me.username(),
            bot_id: me.id,
            publisher: &publisher,
            inbox_dir: &inbox_dir,
            reload_tx: &reload_tx,
            command_tx: &command_tx,
            stop_tx: &stop_tx,
            tz,
        };
        for update in updates {
            // UpdateId wraps u32; offset is i32 per Telegram API
            offset = (update.id.0).cast_signed() + 1;
            handle_update(&bot, update.kind, &ctx).await;
        }
    }
}

async fn handle_update(bot: &Bot, update: UpdateKind, ctx: &TelegramContext<'_>) {
    if let UpdateKind::MyChatMember(change) = &update {
        track_group_membership(ctx.state, change).await;
        return;
    }
    let UpdateKind::Message(msg) = update else {
        return;
    };
    if let Some(&new_id) = msg.migrate_to_chat_id() {
        forget_migrated_group(ctx.state, msg.chat.id, new_id).await;
        return;
    }
    // Skip messages without a sender (channel posts) and bots' own.
    let Some(ref from) = msg.from else {
        return;
    };
    if from.is_bot {
        return;
    }
    dispatch_message(bot, &msg, from, ctx).await;
}

/// Keep the saved group list in step with the bot being added or removed.
async fn track_group_membership(state: &TelegramState, change: &ChatMemberUpdated) {
    let chat = &change.chat;
    if !(chat.is_group() || chat.is_supergroup()) {
        return;
    }
    let key = chat.id.to_string();
    let label = group_label(chat.title());
    let result = if change.new_chat_member.is_present() {
        tracing::info!(group = %label, "telegram bot added to a group");
        state
            .store
            .remember(
                &key,
                ChatRef {
                    kind: ConversationKind::GroupChat,
                    label,
                },
            )
            .await
    } else {
        tracing::info!(group = %label, "telegram bot removed from a group");
        state.store.forget(&key).await
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, chat_id = %chat.id, "failed to save telegram group membership");
    }
}

/// A group upgraded to a supergroup gets a new chat ID; the old one stops working.
async fn forget_migrated_group(state: &TelegramState, old: ChatId, new: ChatId) {
    tracing::info!(%old, %new, "telegram group moved to a new chat ID");
    if let Err(e) = state.store.forget(&old.to_string()).await {
        tracing::warn!(error = %e, chat_id = %old, "failed to forget migrated telegram group");
    }
}

/// Aborts the outbound subscriber when the polling loop exits, so a
/// restarted adapter never leaves a second subscriber delivering duplicates.
struct OutboundTask(tokio::task::JoinHandle<()>);

impl Drop for OutboundTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Register slash commands with the Telegram API so users see autocomplete.
///
/// Mirrors the Discord `register_slash_commands` pattern.
///
/// # Errors
/// Returns an error if the Telegram `setMyCommands` API call fails.
async fn register_commands(bot: &Bot) -> anyhow::Result<()> {
    let commands: Vec<BotCommand> = all_commands()
        .map(|info| BotCommand::new(info.name, info.help))
        .collect();

    bot.set_my_commands(commands).await?;
    tracing::info!("telegram bot commands registered");
    Ok(())
}

/// Dispatch a single incoming message: commands, text, or attachments.
///
/// Every private message is handled; group messages only when addressed to
/// the bot (see [`super::groups`]).
async fn dispatch_message(
    bot: &Bot,
    msg: &teloxide::types::Message,
    from: &teloxide::types::User,
    ctx: &TelegramContext<'_>,
) {
    let chat_id = msg.chat.id;
    let Some(addressed) = addressed_to_agent(msg, from, ctx).await else {
        return;
    };
    let Addressed {
        location,
        text,
        conversation_id,
        kind,
        context,
    } = addressed;
    {
        let _span = tracing::debug_span!("telegram_message",
            sender = %build_sender_name(from),
            chat_id = %msg.chat.id,
            msg_id = %msg.id
        )
        .entered();
        tracing::debug!(sender = %build_sender_name(from), chat_id = %chat_id, location = %location, "telegram message received");
    }

    let Some(standing) = admit_sender(bot, chat_id, from, ctx.state).await else {
        return;
    };

    if let Some(cmd_text) = text.strip_prefix('/') {
        let (cmd_name, cmd_args) = match cmd_text.split_once(' ') {
            Some((name, args)) => (name, Some(args)),
            None => (cmd_text, None),
        };

        // Strip @botname suffix from commands (e.g. /help@mybot)
        let cmd_name = cmd_name.split_once('@').map_or(cmd_name, |(name, _)| name);

        if matches!(standing, Standing::Owner) {
            handle_command(bot, chat_id, from, cmd_name, cmd_args, ctx).await;
        } else {
            tracing::info!(command = %cmd_name, sender = %build_sender_name(from), "refused telegram command from someone other than the owner");
            send_reply(bot, chat_id, "Only my owner can run commands.").await;
        }
        return;
    }

    // Build content with attachment metadata and collect inline images
    let mut content = text;
    let mut images: Vec<ImageData> = Vec::new();

    process_attachments(
        bot,
        msg,
        &mut content,
        &mut images,
        ctx.inbox_dir,
        from,
        ctx.tz,
    )
    .await;

    // Skip empty messages (no text, no attachments processed)
    if content.is_empty() {
        tracing::debug!(sender = %build_sender_name(from), chat_id = %chat_id, "telegram message had no content, dropping");
        return;
    }

    let sender_name = build_sender_name(from);

    let origin = MessageOrigin {
        endpoint: ENDPOINT.to_string(),
        sender: Some(MessageSender {
            name: sender_name.clone(),
            id: from.id.to_string(),
            interface: ENDPOINT.to_string(),
            location: Some(location),
        }),
        conversation: Some(ConversationContext {
            id: conversation_id,
            kind,
            is_owner: matches!(standing, Standing::Owner),
        }),
    };

    // Telegram message IDs are only unique within a chat.
    let correlation_id = format!("telegram-{chat_id}-{}", msg.id);
    ctx.state.reply_targets.track(&correlation_id, chat_id);
    let msg_event = crate::bus::MessageEvent {
        id: correlation_id,
        content,
        origin,
        timestamp: crate::time::now_local(ctx.tz),
        images,
        context,
    };

    if let Err(e) = ctx
        .publisher
        .publish(crate::bus::topics::UserMessage, msg_event)
        .await
    {
        tracing::error!(sender = %sender_name, chat_id = %chat_id, error = %e, "failed to publish telegram message to bus");
        send_reply(
            bot,
            chat_id,
            "Something went wrong handing your message to the agent. Please try again.",
        )
        .await;
    }
}

/// A message the bot has decided to act on: where it happened, its text
/// (mention stripped), its conversation identity, and any unmentioned
/// chatter buffered since it was last addressed there.
struct Addressed {
    location: String,
    text: String,
    conversation_id: String,
    kind: ConversationKind,
    context: Option<String>,
}

/// Where a message was sent and its text (or caption), if it is meant for
/// the agent. Private chats are also recorded, and the first one claims
/// ownership; addressed groups are recorded so the agent can post there
/// later. An unaddressed group message is buffered as context instead, and
/// `None` is returned.
async fn addressed_to_agent(
    msg: &teloxide::types::Message,
    from: &teloxide::types::User,
    ctx: &TelegramContext<'_>,
) -> Option<Addressed> {
    let chat = &msg.chat;
    let text = msg.text().or_else(|| msg.caption()).unwrap_or_default();
    let key = chat.id.to_string();
    let (location, text, reference) = if chat.is_private() {
        let reference = ChatRef::direct_message(&build_sender_name(from));
        ("direct message".to_string(), text.to_string(), reference)
    } else if chat.is_group() || chat.is_supergroup() {
        let replies_to_bot = msg
            .reply_to_message()
            .and_then(|m| m.from.as_ref())
            .is_some_and(|u| u.id == ctx.bot_id);
        let Some(text) = addressed_text(text, ctx.bot_username, replies_to_bot) else {
            buffer_unmentioned(ctx, &key, from, text);
            return None;
        };
        let label = group_label(chat.title());
        let reference = ChatRef {
            kind: ConversationKind::GroupChat,
            label: label.clone(),
        };
        (label, text, reference)
    } else {
        return None;
    };

    let kind = reference.kind;
    let is_private = kind == ConversationKind::Personal;
    if let Err(e) = ctx.state.store.remember(&key, reference).await {
        tracing::warn!(error = %e, chat_id = %chat.id, "failed to save telegram conversation");
    }
    if is_private {
        claim_owner_if_unset(ctx.state, from, &key).await;
    }
    let context = if is_private {
        None
    } else {
        render_context(&location, &ctx.state.context_buffer.drain(&key))
    };
    Some(Addressed {
        location,
        text,
        conversation_id: key,
        kind,
        context,
    })
}

/// Hold an unaddressed group message as context for the next time the bot is
/// addressed in that chat; empty messages (e.g. attachment-only) are dropped.
fn buffer_unmentioned(
    ctx: &TelegramContext<'_>,
    chat_key: &str,
    from: &teloxide::types::User,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    ctx.state.context_buffer.record(
        chat_key,
        BufferedMessage {
            sender: build_sender_name(from),
            text: text.to_string(),
            at: crate::time::now_local(ctx.tz),
        },
    );
}

/// Decide whether the sender may use the bot. Refused senders are told why
/// and get `None`.
async fn admit_sender(
    bot: &Bot,
    chat_id: ChatId,
    from: &teloxide::types::User,
    state: &TelegramState,
) -> Option<Standing> {
    match state
        .store
        .admit(Some(&from.id.to_string()), state.respond_to_others)
        .await
    {
        Ok(standing) => Some(standing),
        Err(refusal) => {
            tracing::info!(
                sender = %build_sender_name(from),
                "telegram message from someone other than the owner; respond_to_others is off"
            );
            send_reply(bot, chat_id, &refusal).await;
            None
        }
    }
}

/// Make the sender the owner if nobody is yet; only called for private chats.
async fn claim_owner_if_unset(state: &TelegramState, from: &teloxide::types::User, dm_chat: &str) {
    let owner = Owner {
        user_id: from.id.to_string(),
        name: build_sender_name(from),
        dm_conversation_id: dm_chat.to_string(),
    };
    match state.store.claim_owner(owner).await {
        Ok(true) => {
            tracing::info!(owner = %build_sender_name(from), "telegram owner set from first direct message");
        }
        Ok(false) => {}
        Err(e) => tracing::error!(error = %e, "failed to save telegram owner"),
    }
}

async fn send_reply(bot: &Bot, chat_id: ChatId, text: &str) {
    if let Err(e) = bot.send_message(chat_id, text).await {
        tracing::warn!(%chat_id, error = %e, "failed to send telegram reply");
    }
}

/// Build a display name from a Telegram user.
fn build_sender_name(user: &teloxide::types::User) -> String {
    match &user.last_name {
        Some(last) => format!("{} {last}", user.first_name),
        None => user.first_name.clone(),
    }
}

/// Handle a Telegram /command.
async fn handle_command(
    bot: &Bot,
    chat_id: ChatId,
    from: &teloxide::types::User,
    cmd_name: &str,
    cmd_args: Option<&str>,
    ctx: &TelegramContext<'_>,
) {
    tracing::debug!(command = %cmd_name, "telegram command received");
    let dispatch = crate::interfaces::CommandDispatch {
        reload_tx: ctx.reload_tx,
        command_tx: ctx.command_tx,
        stop_tx: ctx.stop_tx,
        inbox_dir: ctx.inbox_dir,
        tz: ctx.tz,
    };
    let response_text = crate::interfaces::run_chat_command(
        cmd_name,
        cmd_args,
        &dispatch,
        ENDPOINT,
        &build_sender_name(from),
    )
    .await;

    if let Err(e) = bot.send_message(chat_id, &response_text).await {
        tracing::warn!(
            command = %cmd_name,
            error = %e,
            "failed to send telegram command response"
        );
    }
}

fn doc_as_meta(doc: &Document) -> AttachmentMeta<'_> {
    AttachmentMeta {
        file_id: &doc.file.id.0,
        filename: doc.file_name.as_deref().unwrap_or("document"),
        size: doc.file.size,
        content_type: doc.mime_type.as_ref().map(ToString::to_string),
    }
}

fn photo_as_meta(photo: &PhotoSize) -> AttachmentMeta<'_> {
    AttachmentMeta {
        file_id: &photo.file.id.0,
        filename: "photo.jpg",
        size: photo.file.size,
        content_type: Some("image/jpeg".to_string()),
    }
}

fn audio_as_meta(audio: &Audio) -> AttachmentMeta<'_> {
    AttachmentMeta {
        file_id: &audio.file.id.0,
        filename: audio.file_name.as_deref().unwrap_or("audio"),
        size: audio.file.size,
        content_type: audio.mime_type.as_ref().map(ToString::to_string),
    }
}

fn voice_as_meta(voice: &Voice) -> AttachmentMeta<'_> {
    AttachmentMeta {
        file_id: &voice.file.id.0,
        filename: "voice.ogg",
        size: voice.file.size,
        content_type: voice.mime_type.as_ref().map(ToString::to_string),
    }
}

fn video_as_meta(video: &Video) -> AttachmentMeta<'_> {
    AttachmentMeta {
        file_id: &video.file.id.0,
        filename: video.file_name.as_deref().unwrap_or("video.mp4"),
        size: video.file.size,
        content_type: video.mime_type.as_ref().map(ToString::to_string),
    }
}

/// Extract and process all attachment types from a Telegram message.
async fn process_attachments(
    bot: &Bot,
    msg: &teloxide::types::Message,
    content: &mut String,
    images: &mut Vec<ImageData>,
    inbox_dir: &Path,
    from: &teloxide::types::User,
    tz: chrono_tz::Tz,
) {
    if let Some(doc) = msg.document() {
        handle_attachment(bot, content, images, &doc_as_meta(doc), inbox_dir, from, tz).await;
    }

    if let Some(photos) = msg.photo()
        && let Some(photo) = photos.last()
    {
        handle_attachment(
            bot,
            content,
            images,
            &photo_as_meta(photo),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    if let Some(audio) = msg.audio() {
        handle_attachment(
            bot,
            content,
            images,
            &audio_as_meta(audio),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    if let Some(voice) = msg.voice() {
        handle_attachment(
            bot,
            content,
            images,
            &voice_as_meta(voice),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    if let Some(video) = msg.video() {
        handle_attachment(
            bot,
            content,
            images,
            &video_as_meta(video),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }
}

/// Download a Telegram file and append attachment metadata to the content string.
async fn handle_attachment(
    bot: &Bot,
    content: &mut String,
    images: &mut Vec<ImageData>,
    meta: &AttachmentMeta<'_>,
    inbox_dir: &Path,
    from: &teloxide::types::User,
    tz: chrono_tz::Tz,
) {
    use crate::interfaces::attachment::{
        AttachmentInfo, SavedAttachment, finalize_attachment, format_failed_attachment_line,
    };
    use teloxide::net::Download;

    let filename = meta.filename;

    let info = AttachmentInfo {
        filename: filename.to_string(),
        size: meta.size,
        content_type: meta.content_type.clone(),
    };

    // Two-step Telegram download: get_file → download_file
    let download_result: Result<SavedAttachment, String> = async {
        let file = bot
            .get_file(teloxide::types::FileId(meta.file_id.to_string()))
            .await
            .map_err(|e| format!("failed to get file info for '{filename}': {e}"))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let saved_name = format!("{timestamp}_{filename}");
        let local_path = inbox_dir.join(&saved_name);

        let mut dst = tokio::fs::File::create(&local_path)
            .await
            .map_err(|e| format!("failed to create file '{filename}': {e}"))?;

        bot.download_file(&file.path, &mut dst)
            .await
            .map_err(|e| format!("failed to download '{filename}': {e}"))?;

        Ok(SavedAttachment { local_path })
    }
    .await;

    match download_result {
        Ok(saved) => {
            let author = build_sender_name(from);
            if let Some(img) =
                finalize_attachment(&saved, &info, content, &author, inbox_dir, tz, "Telegram")
                    .await
            {
                images.push(img);
            }
        }
        Err(reason) => {
            tracing::warn!(
                filename = %filename,
                error = %reason,
                "failed to download telegram attachment"
            );
            let line = format_failed_attachment_line(&info, &reason);
            content.push('\n');
            content.push_str(&line);
        }
    }
}

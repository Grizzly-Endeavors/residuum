//! Telegram long-polling message handler and command dispatch.

use std::path::Path;
use std::time::Duration;

use teloxide::Bot;
use teloxide::payloads::GetUpdatesSetters;
use teloxide::requests::Requester;
use teloxide::types::{Audio, BotCommand, ChatId, Document, PhotoSize, UpdateKind, Video, Voice};

use crate::bus::{BusHandle, EndpointName, Publisher};
use crate::gateway::event_loop::AdapterSenders;
use crate::gateway::types::{ReloadSignal, ServerCommand};
use crate::interfaces::cli::commands::{
    CommandContext, CommandSideEffect, all_commands, execute_command,
};
use crate::interfaces::types::MessageOrigin;
use crate::models::ImageData;

/// Shared gateway references threaded through telegram message dispatch.
struct TelegramContext<'a> {
    publisher: &'a Publisher,
    inbox_dir: &'a Path,
    reload_tx: &'a tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: &'a tokio::sync::mpsc::Sender<ServerCommand>,
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
/// Returns an error if the initial `get_me` verification fails.
pub(super) async fn run_telegram_polling(
    token: &str,
    senders: AdapterSenders,
    workspace_dir: std::path::PathBuf,
    tz: chrono_tz::Tz,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let publisher = senders.publisher;
    let bus_handle = senders.bus_handle;
    let reload_tx = senders.reload;
    let command_tx = senders.command;
    // TCP keepalive detects silently-dropped connections (e.g. NAT timeout);
    // pool_idle_timeout evicts stale connections before they poison the pool.
    // Without these, long-poll requests reuse dead connections indefinitely.
    let http_client = teloxide::net::default_reqwest_settings()
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;
    let bot = Bot::with_client(token, http_client);
    let inbox_dir = workspace_dir.join("inbox");

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

    let mut offset: i32 = 0;
    let mut consecutive_errors: u32 = 0;
    let mut subscriber_spawned = false;

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

        for update in updates {
            // UpdateId wraps u32; offset is i32 per Telegram API
            offset = (update.id.0).cast_signed() + 1;

            let UpdateKind::Message(msg) = update.kind else {
                continue;
            };

            // DM-only: skip non-private chats
            if !msg.chat.is_private() {
                continue;
            }

            // Skip messages without a sender (channel posts)
            let Some(ref from) = msg.from else {
                continue;
            };

            // Skip bot's own messages
            if from.is_bot {
                continue;
            }

            // Spawn subscriber loops on first private DM
            if !subscriber_spawned {
                subscriber_spawned = true;
                let chat_id = msg.chat.id;
                spawn_telegram_subscribers(&bus_handle, &bot, chat_id).await;
            }

            let ctx = TelegramContext {
                publisher: &publisher,
                inbox_dir: &inbox_dir,
                reload_tx: &reload_tx,
                command_tx: &command_tx,
                tz,
            };
            dispatch_message(&bot, &msg, from, &ctx).await;
        }
    }
}

/// Spawn typed bus subscriber loop for Telegram output.
async fn spawn_telegram_subscribers(bus_handle: &BusHandle, bot: &Bot, chat_id: ChatId) {
    match super::subscriber::TelegramSubscribers::new(bus_handle, EndpointName::from("telegram"))
        .await
    {
        Ok(subs) => {
            let b = bot.clone();
            tokio::spawn(super::subscriber::run_telegram_subscriber(subs, b, chat_id));
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to telegram bus topics");
        }
    }

    tracing::debug!(%chat_id, "telegram subscriber loops spawned");
}

/// Register slash commands with the Telegram API so users see autocomplete.
///
/// Mirrors the Discord `register_slash_commands` pattern. Client-only commands
/// (quit, exit, verbose toggles) are skipped.
///
/// # Errors
/// Returns an error if the Telegram `setMyCommands` API call fails.
async fn register_commands(bot: &Bot) -> anyhow::Result<()> {
    let commands: Vec<BotCommand> = all_commands()
        .filter(|info| !info.cli_only)
        .map(|info| BotCommand::new(info.name, info.help))
        .collect();

    bot.set_my_commands(commands).await?;
    tracing::info!("telegram bot commands registered");
    Ok(())
}

/// Dispatch a single incoming private message: commands, text, or attachments.
async fn dispatch_message(
    bot: &Bot,
    msg: &teloxide::types::Message,
    from: &teloxide::types::User,
    ctx: &TelegramContext<'_>,
) {
    let chat_id = msg.chat.id;

    tracing::debug!(sender = %build_sender_name(from), chat_id = %chat_id, "telegram message received");

    // Check for /command prefix
    if let Some(text) = msg.text()
        && let Some(cmd_text) = text.strip_prefix('/')
    {
        let (cmd_name, cmd_args) = match cmd_text.split_once(' ') {
            Some((name, args)) => (name, Some(args)),
            None => (cmd_text, None),
        };

        // Strip @botname suffix from commands (e.g. /help@mybot)
        let cmd_name = cmd_name.split_once('@').map_or(cmd_name, |(name, _)| name);

        handle_command(bot, chat_id, from, cmd_name, cmd_args, ctx).await;
        return;
    }

    // Build content with attachment metadata and collect inline images
    let mut content = msg.text().unwrap_or("").to_string();
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
        endpoint: "telegram".to_string(),
        sender_name: sender_name.clone(),
        sender_id: from.id.to_string(),
    };

    let msg_event = crate::bus::MessageEvent {
        id: msg.id.to_string(),
        content,
        origin,
        timestamp: crate::time::now_local(ctx.tz),
        images,
    };

    if let Err(e) = ctx
        .publisher
        .publish(crate::bus::topics::UserMessage, msg_event)
        .await
    {
        tracing::warn!(sender = %sender_name, chat_id = %chat_id, error = %e, "failed to publish telegram message to bus");
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
    let command_ctx = CommandContext {
        url: "",
        verbose: false,
    };

    let result = execute_command(cmd_name, cmd_args, &command_ctx);

    let response_text = match result.side_effect {
        Some(CommandSideEffect::Reload) => {
            tracing::info!("reload requested via telegram command");
            if ctx.reload_tx.send(ReloadSignal::Root).is_err() {
                tracing::warn!("reload_tx closed, reload dropped");
            }
            result.response
        }
        Some(CommandSideEffect::ServerCommand { name, args }) => {
            crate::interfaces::dispatch_server_command(
                ctx.command_tx,
                name,
                args,
                result.response,
                "telegram command",
            )
            .await
        }
        Some(CommandSideEffect::InboxAdd(body)) => {
            let source = format!("telegram:{}", build_sender_name(from));
            crate::interfaces::inbox_add_from_command(
                ctx.inbox_dir,
                &body,
                &source,
                ctx.tz,
                result.response,
            )
            .await
        }
        Some(CommandSideEffect::Quit | CommandSideEffect::ToggleVerbose) | None => result.response,
    };

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

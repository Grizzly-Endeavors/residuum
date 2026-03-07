//! Telegram long-polling message handler and command dispatch.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use teloxide::Bot;
use teloxide::payloads::GetUpdatesSetters;
use teloxide::requests::Requester;
use teloxide::types::{BotCommand, ChatId, UpdateKind};
use tokio::sync::mpsc;

use crate::gateway::types::{ReloadSignal, ServerCommand};
use crate::inbox;
use crate::interfaces::attachment::{
    MAX_IMAGE_INLINE_SIZE, encode_image_from_file, is_supported_image,
};
use crate::interfaces::cli::commands::{
    CommandContext, CommandSideEffect, all_commands, execute_command,
};
use crate::interfaces::types::{InboundMessage, MessageOrigin, RoutedMessage};
use crate::models::ImageData;

use super::reply::TelegramReplyHandle;

/// Shared gateway references threaded through telegram message dispatch.
struct TelegramContext<'a> {
    inbound_tx: &'a mpsc::Sender<RoutedMessage>,
    inbox_dir: &'a Path,
    reload_tx: &'a tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: &'a mpsc::Sender<ServerCommand>,
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
    inbound_tx: mpsc::Sender<RoutedMessage>,
    workspace_dir: std::path::PathBuf,
    reload_tx: tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
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

    loop {
        let updates = tokio::select! {
            result = bot.get_updates().offset(offset).timeout(30) => {
                match result {
                    Ok(updates) => updates,
                    Err(e) => {
                        tracing::warn!(error = %e, "telegram polling error, retrying in 5s");
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

            let ctx = TelegramContext {
                inbound_tx: &inbound_tx,
                inbox_dir: &inbox_dir,
                reload_tx: &reload_tx,
                command_tx: &command_tx,
                tz,
            };
            dispatch_message(&bot, &msg, from, &ctx).await;
        }
    }
}

/// Register slash commands with the Telegram API so users see autocomplete.
///
/// Mirrors the Discord `register_slash_commands` pattern. Client-only commands
/// (quit, exit, verbose toggles) are skipped.
///
/// # Errors
/// Returns an error if the Telegram `setMyCommands` API call fails.
async fn register_commands(bot: &Bot) -> anyhow::Result<()> {
    let skip = ["quit", "exit", "q", "verbose", "v"];

    let commands: Vec<BotCommand> = all_commands()
        .filter(|info| !skip.contains(&info.name))
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

    // Check for /command prefix
    if let Some(text) = msg.text()
        && let Some(cmd_text) = text.strip_prefix('/')
    {
        let (cmd_name, cmd_args) = match cmd_text.split_once(' ') {
            Some((name, args)) => (name, Some(args)),
            None => (cmd_text, None),
        };

        // Strip @botname suffix from commands (e.g. /help@mybot)
        let cmd_name = cmd_name.split('@').next().unwrap_or(cmd_name);

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
        return;
    }

    let sender_name = build_sender_name(from);

    let origin = MessageOrigin {
        interface: "telegram".to_string(),
        sender_name,
        sender_id: from.id.to_string(),
    };

    let inbound = InboundMessage {
        id: msg.id.to_string(),
        content,
        origin,
        timestamp: chrono::Utc::now(),
        images,
    };

    let reply = Arc::new(TelegramReplyHandle::new(bot.clone(), chat_id));

    let routed = RoutedMessage {
        message: inbound,
        reply,
    };

    if ctx.inbound_tx.send(routed).await.is_err() {
        tracing::warn!("inbound channel closed, dropping telegram message");
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
        interface_name: "telegram",
    };

    let result = execute_command(cmd_name, cmd_args, &command_ctx);

    let response_text = match result.side_effect {
        Some(CommandSideEffect::Reload) => {
            tracing::info!("reload requested via telegram command");
            ctx.reload_tx.send(ReloadSignal::Root).ok();
            result.response
        }
        Some(CommandSideEffect::ServerCommand { name, args }) => {
            tracing::info!(command = %name, "server command via telegram command");
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            ctx.command_tx
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
            let source = format!("telegram:{}", build_sender_name(from));
            match inbox::quick_add(ctx.inbox_dir, &title, &body, &source, ctx.tz).await {
                Ok(_) => result.response,
                Err(e) => format!("failed to add inbox item: {e}"),
            }
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
    // Handle document attachments
    if let Some(doc) = msg.document() {
        let meta = AttachmentMeta {
            file_id: &doc.file.id.0,
            filename: doc.file_name.as_deref().unwrap_or("document"),
            size: doc.file.size,
            content_type: doc.mime_type.as_ref().map(ToString::to_string),
        };
        handle_attachment(bot, content, images, &meta, inbox_dir, from, tz).await;
    }

    // Handle photo attachments (use largest size)
    if let Some(photos) = msg.photo()
        && let Some(photo) = photos.last()
    {
        let meta = AttachmentMeta {
            file_id: &photo.file.id.0,
            filename: "photo.jpg",
            size: photo.file.size,
            content_type: Some("image/jpeg".to_string()),
        };
        handle_attachment(bot, content, images, &meta, inbox_dir, from, tz).await;
    }

    // Handle audio attachments
    if let Some(audio) = msg.audio() {
        let meta = AttachmentMeta {
            file_id: &audio.file.id.0,
            filename: audio.file_name.as_deref().unwrap_or("audio"),
            size: audio.file.size,
            content_type: audio.mime_type.as_ref().map(ToString::to_string),
        };
        handle_attachment(bot, content, images, &meta, inbox_dir, from, tz).await;
    }

    // Handle voice attachments
    if let Some(voice) = msg.voice() {
        let meta = AttachmentMeta {
            file_id: &voice.file.id.0,
            filename: "voice.ogg",
            size: voice.file.size,
            content_type: voice.mime_type.as_ref().map(ToString::to_string),
        };
        handle_attachment(bot, content, images, &meta, inbox_dir, from, tz).await;
    }

    // Handle video attachments
    if let Some(video) = msg.video() {
        let meta = AttachmentMeta {
            file_id: &video.file.id.0,
            filename: video.file_name.as_deref().unwrap_or("video.mp4"),
            size: video.file.size,
            content_type: video.mime_type.as_ref().map(ToString::to_string),
        };
        handle_attachment(bot, content, images, &meta, inbox_dir, from, tz).await;
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
        AttachmentInfo, SavedAttachment, format_attachment_line, format_failed_attachment_line,
    };
    use teloxide::net::Download;

    let filename = meta.filename;

    let info = AttachmentInfo {
        filename: filename.to_string(),
        url: String::new(), // Telegram doesn't use URL-based download
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
                        filename = %filename,
                        error = %e,
                        "failed to encode telegram image for inline delivery"
                    ),
                }
            } else if is_supported_image(info.content_type.as_deref()) {
                tracing::warn!(
                    filename = %filename,
                    size = info.size,
                    "telegram image exceeds inline size limit, saved but not sent to model"
                );
            }

            // Create companion inbox item
            let saved_name = saved
                .local_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let content_type_str = info.content_type.as_deref().unwrap_or("unknown");
            let companion = inbox::InboxItem {
                title: format!("Telegram attachment: {filename}"),
                body: format!(
                    "From: {}\nSize: {} bytes\nContent-Type: {content_type_str}",
                    build_sender_name(from),
                    info.size,
                ),
                source: "telegram".to_string(),
                timestamp: crate::time::now_local(tz),
                read: false,
                attachments: vec![std::path::PathBuf::from("inbox").join(&saved_name)],
            };
            let item_filename = inbox::generate_filename(&companion.title, tz);
            if let Err(e) = inbox::save_item(inbox_dir, &item_filename, &companion).await {
                tracing::warn!(
                    filename = %filename,
                    error = %e,
                    "failed to create companion inbox item for telegram attachment"
                );
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

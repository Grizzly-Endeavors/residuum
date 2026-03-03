//! Telegram long-polling message handler and command dispatch.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use teloxide::Bot;
use teloxide::payloads::GetUpdatesSetters;
use teloxide::requests::Requester;
use teloxide::types::{ChatId, UpdateKind};
use tokio::sync::mpsc;

use crate::gateway::server::{ReloadSignal, ServerCommand};
use crate::inbox;
use crate::interfaces::cli::commands::{CommandContext, CommandSideEffect, execute_command};
use crate::interfaces::types::{InboundMessage, MessageOrigin, RoutedMessage};

use super::reply::TelegramReplyHandle;

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
    let bot = Bot::new(token);
    let inbox_dir = workspace_dir.join("inbox");

    // Verify the bot token is valid
    let me = bot.get_me().await?;
    tracing::info!(
        bot_name = %me.first_name,
        bot_username = %me.username(),
        "telegram bot connected"
    );

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

            dispatch_message(
                &bot,
                &msg,
                from,
                &inbound_tx,
                &inbox_dir,
                &reload_tx,
                &command_tx,
                tz,
            )
            .await;
        }
    }
}

/// Dispatch a single incoming private message: commands, text, or attachments.
#[expect(
    clippy::too_many_arguments,
    reason = "threads subsystem references from the polling loop for message dispatch"
)]
#[expect(
    clippy::too_many_lines,
    reason = "sequential attachment handling for each media type; splitting would obscure the flow"
)]
async fn dispatch_message(
    bot: &Bot,
    msg: &teloxide::types::Message,
    from: &teloxide::types::User,
    inbound_tx: &mpsc::Sender<RoutedMessage>,
    inbox_dir: &Path,
    reload_tx: &tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: &mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
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

        handle_command(
            bot, chat_id, from, cmd_name, cmd_args, inbox_dir, reload_tx, command_tx, tz,
        )
        .await;
        return;
    }

    // Build content with attachment metadata
    let mut content = msg.text().unwrap_or("").to_string();

    // Handle document attachments
    if let Some(doc) = msg.document() {
        handle_attachment(
            bot,
            &mut content,
            &doc.file.id.0,
            doc.file_name.as_deref().unwrap_or("document"),
            doc.file.size,
            doc.mime_type.as_ref().map(ToString::to_string),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    // Handle photo attachments (use largest size)
    if let Some(photos) = msg.photo()
        && let Some(photo) = photos.last()
    {
        handle_attachment(
            bot,
            &mut content,
            &photo.file.id.0,
            "photo.jpg",
            photo.file.size,
            Some("image/jpeg".to_string()),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    // Handle audio attachments
    if let Some(audio) = msg.audio() {
        handle_attachment(
            bot,
            &mut content,
            &audio.file.id.0,
            audio.file_name.as_deref().unwrap_or("audio"),
            audio.file.size,
            audio.mime_type.as_ref().map(ToString::to_string),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    // Handle voice attachments
    if let Some(voice) = msg.voice() {
        handle_attachment(
            bot,
            &mut content,
            &voice.file.id.0,
            "voice.ogg",
            voice.file.size,
            voice.mime_type.as_ref().map(ToString::to_string),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

    // Handle video attachments
    if let Some(video) = msg.video() {
        handle_attachment(
            bot,
            &mut content,
            &video.file.id.0,
            video.file_name.as_deref().unwrap_or("video.mp4"),
            video.file.size,
            video.mime_type.as_ref().map(ToString::to_string),
            inbox_dir,
            from,
            tz,
        )
        .await;
    }

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
    };

    let reply = Arc::new(TelegramReplyHandle::new(bot.clone(), chat_id));

    let routed = RoutedMessage {
        message: inbound,
        reply,
    };

    if inbound_tx.send(routed).await.is_err() {
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
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors discord handler's command dispatch with all needed context"
)]
async fn handle_command(
    bot: &Bot,
    chat_id: ChatId,
    from: &teloxide::types::User,
    cmd_name: &str,
    cmd_args: Option<&str>,
    inbox_dir: &Path,
    reload_tx: &tokio::sync::watch::Sender<ReloadSignal>,
    command_tx: &mpsc::Sender<ServerCommand>,
    tz: chrono_tz::Tz,
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
            reload_tx.send(ReloadSignal::Root).ok();
            result.response
        }
        Some(CommandSideEffect::ServerCommand { name, args }) => {
            tracing::info!(command = %name, "server command via telegram command");
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            command_tx
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
            match inbox::quick_add(inbox_dir, &title, &body, &source, tz).await {
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

/// Download a Telegram file and append attachment metadata to the content string.
#[expect(
    clippy::too_many_arguments,
    reason = "attachment handling requires file metadata, bot reference, and inbox context"
)]
async fn handle_attachment(
    bot: &Bot,
    content: &mut String,
    file_id: &str,
    filename: &str,
    size: u32,
    content_type: Option<String>,
    inbox_dir: &Path,
    from: &teloxide::types::User,
    tz: chrono_tz::Tz,
) {
    use crate::interfaces::attachment::{
        AttachmentInfo, SavedAttachment, format_attachment_line, format_failed_attachment_line,
    };
    use teloxide::net::Download;

    let info = AttachmentInfo {
        filename: filename.to_string(),
        url: String::new(), // Telegram doesn't use URL-based download
        size,
        content_type,
    };

    // Two-step Telegram download: get_file → download_file
    let download_result: Result<SavedAttachment, String> = async {
        let file = bot
            .get_file(teloxide::types::FileId(file_id.to_string()))
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

//! Telegram bus subscriber — translates typed bus events to Telegram chat messages.

use std::collections::HashMap;
use std::sync::Arc;

use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use teloxide::RequestError;

use crate::bus::{ErrorEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent};
use crate::interfaces::chunking::chunk_text;

use super::TelegramState;

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;
/// Maximum caption length on a photo, audio, or document.
const MAX_CAPTION_CHARS: usize = 1024;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

/// Receives events from the bus and delivers them to Telegram chats.
pub(super) async fn run_telegram_subscriber(
    mut subs: crate::interfaces::BaseSubscribers,
    bot: Bot,
    state: Arc<TelegramState>,
) {
    // One typing loop per in-flight turn, stopped by dropping its sender.
    let mut typing: HashMap<String, tokio::sync::watch::Sender<()>> = HashMap::new();
    let mut clean_exit = true;

    loop {
        tokio::select! {
            event = subs.turn_lifecycle.recv() => {
                match event {
                    Ok(Some(TurnLifecycleEvent::Started { correlation_id })) => {
                        if let Some(chat_id) = state.target_for(&correlation_id).await {
                            typing.insert(correlation_id, spawn_typing(bot.clone(), chat_id));
                        }
                    }
                    Ok(Some(TurnLifecycleEvent::Ended { correlation_id })) => {
                        typing.remove(&correlation_id);
                        state.reply_targets.release(&correlation_id);
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.response.recv() => {
                match event {
                    Ok(Some(resp)) => deliver_response(&bot, &state, resp).await,
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.intermediate.recv() => {
                match event {
                    Ok(Some(im)) => {
                        if let Some(chat_id) = target_or_warn(&state, &im.correlation_id).await {
                            send_or_warn(&bot, chat_id, &im.content).await;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            // System notices and errors can carry internals; they only ever
            // go to the owner.
            event = subs.notice.recv() => {
                match event {
                    Ok(Some(NoticeEvent { message })) => {
                        if let Some(chat_id) = state.owner_dm().await {
                            send_or_warn(&bot, chat_id, &message).await;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.error.recv() => {
                match event {
                    Ok(Some(ErrorEvent { message, .. })) => {
                        if let Some(chat_id) = state.owner_dm().await {
                            send_or_warn(&bot, chat_id, &format!("**Error:** {message}")).await;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
        }
    }

    if clean_exit {
        tracing::debug!("telegram subscriber loop ended");
    } else {
        tracing::warn!("telegram subscriber loop ended unexpectedly");
    }
}

async fn target_or_warn(state: &TelegramState, correlation_id: &str) -> Option<ChatId> {
    let target = state.target_for(correlation_id).await;
    if target.is_none() {
        tracing::warn!(
            correlation_id,
            "no telegram chat to deliver to; the owner has not messaged the bot yet"
        );
    }
    target
}

fn spawn_typing(bot: Bot, chat_id: ChatId) -> tokio::sync::watch::Sender<()> {
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        loop {
            if let Err(e) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
                tracing::trace!(error = %e, "telegram typing indicator failed");
            }
            tokio::select! {
                () = tokio::time::sleep(tokio::time::Duration::from_secs(TYPING_INTERVAL_SECS)) => {}
                // Resolves with an error once the sender is dropped at turn end.
                _ = stop_rx.changed() => break,
            }
        }
    });
    stop_tx
}

async fn deliver_response(bot: &Bot, state: &TelegramState, resp: ResponseEvent) {
    let target = match &resp.conversation {
        Some(id) => {
            let Ok(n) = id.parse::<i64>() else {
                tracing::warn!(conversation = %id, "telegram message addressed to an invalid chat ID");
                notify_owner_of_failure(bot, state, id, "that is not a Telegram chat ID").await;
                return;
            };
            ChatId(n)
        }
        None => match target_or_warn(state, &resp.correlation_id).await {
            Some(target) => target,
            None => return,
        },
    };
    let sent = if let Some(ref att) = resp.attachment {
        send_file(bot, target, att, &resp.content).await
    } else if resp.content.is_empty() {
        Ok(())
    } else {
        send_chunks(bot, target, &resp.content).await
    };
    if let Err(e) = sent {
        tracing::warn!(chat_id = %target, error = %e, "failed to deliver telegram message");
        if let Some(id) = &resp.conversation {
            let place = state
                .store
                .conversation(id)
                .await
                .map_or_else(|| format!("chat {id}"), |c| c.label);
            notify_owner_of_failure(bot, state, &place, &e.to_string()).await;
        }
    }
}

/// Tell the owner a message the agent addressed to a specific conversation
/// did not go out, since nobody else will see that it failed.
async fn notify_owner_of_failure(bot: &Bot, state: &TelegramState, place: &str, reason: &str) {
    if let Some(dm) = state.owner_dm().await {
        send_or_warn(
            bot,
            dm,
            &format!("**Error:** I couldn't post a message to {place}: {reason}"),
        )
        .await;
    }
}

async fn send_or_warn(bot: &Bot, chat_id: ChatId, content: &str) {
    if let Err(e) = send_chunks(bot, chat_id, content).await {
        tracing::warn!(chat_id = %chat_id, error = %e, "failed to send telegram message");
    }
}

/// Send `content` in chunks, stopping at the first failure.
async fn send_chunks(bot: &Bot, chat_id: ChatId, content: &str) -> Result<(), RequestError> {
    for chunk in chunk_text(content, TELEGRAM_MAX_CHARS) {
        bot.send_message(chat_id, &chunk).await?;
    }
    Ok(())
}

async fn send_file(
    bot: &Bot,
    chat_id: ChatId,
    attachment: &crate::interfaces::attachment::FileAttachment,
    caption: &str,
) -> Result<(), RequestError> {
    use teloxide::payloads::{SendAudioSetters, SendDocumentSetters, SendPhotoSetters};
    use teloxide::types::InputFile;

    let file = InputFile::file(&attachment.path);
    // Telegram's caption limit counts characters, not bytes.
    let caption_too_long = caption.chars().count() > MAX_CAPTION_CHARS;
    let cap = if caption.is_empty() {
        None
    } else if caption_too_long {
        // Send the full text separately below.
        Some(caption.chars().take(MAX_CAPTION_CHARS).collect::<String>())
    } else {
        Some(caption.to_string())
    };

    if attachment.mime_type.starts_with("image/") {
        let mut req = bot.send_photo(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    } else if attachment.mime_type.starts_with("audio/") {
        let mut req = bot.send_audio(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    } else {
        let mut req = bot.send_document(chat_id, file);
        if let Some(ref c) = cap {
            req = req.caption(c);
        }
        req.await?;
    }
    tracing::debug!(
        filename = %attachment.filename,
        endpoint = "telegram",
        "file delivered"
    );

    // If caption was truncated, send the full text separately
    if caption_too_long {
        send_chunks(bot, chat_id, caption).await?;
    }
    Ok(())
}

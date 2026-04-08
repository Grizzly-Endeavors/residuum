//! Telegram bus subscriber — translates typed bus events to Telegram chat messages.

use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use crate::bus::{ErrorEvent, NoticeEvent, TurnLifecycleEvent};
use crate::interfaces::chunking::chunk_text;

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

/// Typed subscribers for a single Telegram connection.
pub(crate) type TelegramSubscribers = crate::interfaces::BaseSubscribers;

/// Receives events from the bus and delivers them to the Telegram chat.
pub(crate) async fn run_telegram_subscriber(
    mut subs: TelegramSubscribers,
    bot: Bot,
    chat_id: ChatId,
) {
    let mut typing_cancel: Option<tokio::sync::watch::Sender<bool>> = None;
    let mut clean_exit = true;

    loop {
        tokio::select! {
            event = subs.turn_lifecycle.recv() => {
                match event {
                    Ok(Some(TurnLifecycleEvent::Started { .. })) => {
                        let b = bot.clone();
                        let cid = chat_id;
                        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
                        typing_cancel = Some(stop_tx);
                        tokio::spawn(async move {
                            loop {
                                if let Err(e) = b.send_chat_action(cid, ChatAction::Typing).await {
                                    tracing::trace!(error = %e, "telegram typing indicator failed");
                                }
                                tokio::select! {
                                    () = tokio::time::sleep(tokio::time::Duration::from_secs(TYPING_INTERVAL_SECS)) => {}
                                    _ = stop_rx.changed() => break,
                                }
                            }
                        });
                    }
                    Ok(Some(TurnLifecycleEvent::Ended { .. })) => {
                        typing_cancel.take();
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.response.recv() => {
                match event {
                    Ok(Some(resp)) => send_chunks(&bot, chat_id, &resp.content).await,
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.intermediate.recv() => {
                match event {
                    Ok(Some(im)) => send_chunks(&bot, chat_id, &im.content).await,
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.notice.recv() => {
                match event {
                    Ok(Some(NoticeEvent { message })) => {
                        send_chunks(&bot, chat_id, &message).await;
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.error.recv() => {
                match event {
                    Ok(Some(ErrorEvent { message, .. })) => {
                        let text = format!("**Error:** {message}");
                        send_chunks(&bot, chat_id, &text).await;
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

async fn send_chunks(bot: &Bot, chat_id: ChatId, content: &str) {
    let chunks = chunk_text(content, TELEGRAM_MAX_CHARS);
    for chunk in chunks {
        if let Err(e) = bot.send_message(chat_id, &chunk).await {
            tracing::warn!(chat_id = %chat_id, error = %e, "failed to send telegram message");
        }
    }
}

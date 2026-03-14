//! Telegram bus subscriber — translates `BusEvent`s to Telegram chat messages.

use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use crate::bus::{BusEvent, Subscriber};
use crate::interfaces::chunking::chunk_text;

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

/// Receives events from the bus and delivers them to the Telegram chat.
pub(crate) async fn run_telegram_subscriber(mut subscriber: Subscriber, bot: Bot, chat_id: ChatId) {
    let mut typing_cancel: Option<tokio::sync::watch::Sender<bool>> = None;

    while let Some(event) = subscriber.recv().await {
        match event {
            BusEvent::TurnStarted { .. } => {
                // Start a typing indicator loop
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
            BusEvent::TurnEnded { .. } => {
                // Cancel typing loop
                typing_cancel.take();
            }
            BusEvent::Response(resp) => {
                send_chunks(&bot, chat_id, &resp.content).await;
            }
            BusEvent::Intermediate(im) => {
                send_chunks(&bot, chat_id, &im.content).await;
            }
            BusEvent::SystemEvent(se) => {
                let text = format!("**[{}]** {}", se.source, se.content);
                send_chunks(&bot, chat_id, &text).await;
            }
            BusEvent::Error {
                correlation_id: _,
                message,
            } => {
                let text = format!("**Error:** {message}");
                send_chunks(&bot, chat_id, &text).await;
            }
            BusEvent::Notice { message } => {
                send_chunks(&bot, chat_id, &message).await;
            }
            // Tool calls/results are not surfaced in Telegram
            BusEvent::ToolCall(_)
            | BusEvent::ToolResult(_)
            | BusEvent::Message(_)
            | BusEvent::Notification(_)
            | BusEvent::AgentResult(_)
            | BusEvent::WebhookPayload { .. }
            | BusEvent::SpawnRequest(_) => {}
        }
    }

    tracing::debug!("telegram subscriber loop ended (broker shut down)");
}

async fn send_chunks(bot: &Bot, chat_id: ChatId, content: &str) {
    let chunks = chunk_text(content, TELEGRAM_MAX_CHARS);
    for chunk in chunks {
        if let Err(e) = bot.send_message(chat_id, &chunk).await {
            tracing::warn!(error = %e, "failed to send telegram message");
        }
    }
}

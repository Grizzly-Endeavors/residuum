//! Telegram bus subscriber — translates typed bus events to Telegram chat messages.

use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

use crate::bus::{
    EndpointName, IntermediateEvent, ResponseEvent, Subscriber, SystemMessageEvent,
    TurnLifecycleEvent, topics,
};
use crate::interfaces::chunking::chunk_text;

/// Maximum message length for Telegram.
const TELEGRAM_MAX_CHARS: usize = 4096;

/// Interval between typing indicator re-sends (seconds).
///
/// Telegram's typing indicator lasts ~5s, so 4s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 4;

/// Typed subscribers for a single Telegram connection.
pub(crate) struct TelegramSubscribers {
    response: Subscriber<ResponseEvent>,
    turn_lifecycle: Subscriber<TurnLifecycleEvent>,
    intermediate: Subscriber<IntermediateEvent>,
    system: Subscriber<SystemMessageEvent>,
}

impl TelegramSubscribers {
    /// Create all typed subscribers for a Telegram connection.
    pub(crate) async fn new(
        bus_handle: &crate::bus::BusHandle,
        ep: EndpointName,
    ) -> Result<Self, crate::bus::BusError> {
        Ok(Self {
            response: bus_handle.subscribe(topics::Response(ep.clone())).await?,
            turn_lifecycle: bus_handle
                .subscribe(topics::TurnLifecycle(ep.clone()))
                .await?,
            intermediate: bus_handle.subscribe(topics::Intermediate(ep)).await?,
            system: bus_handle.subscribe(topics::SystemMessage).await?,
        })
    }
}

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
            event = subs.system.recv() => {
                match event {
                    Ok(Some(SystemMessageEvent::Notice { message })) => {
                        send_chunks(&bot, chat_id, &message).await;
                    }
                    Ok(Some(SystemMessageEvent::Error { message, .. })) => {
                        let text = format!("**Error:** {message}");
                        send_chunks(&bot, chat_id, &text).await;
                    }
                    Ok(Some(SystemMessageEvent::Event(se))) => {
                        let text = format!("**[{}]** {}", se.source, se.content);
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

//! Discord bus subscriber — translates typed bus events to Discord DM messages.

use std::sync::Arc;

use serenity::model::id::ChannelId;
use tokio::sync::Mutex;

use crate::bus::{
    EndpointName, IntermediateEvent, ResponseEvent, SystemMessageEvent, TurnLifecycleEvent,
    TypedSubscriber, topics,
};
use crate::interfaces::chunking::chunk_text;

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval between typing indicator re-sends (seconds).
///
/// Discord's typing indicator lasts ~10s, so 8s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 8;

/// Typed subscribers for a single Discord connection.
pub(crate) struct DiscordSubscribers {
    response: TypedSubscriber<ResponseEvent>,
    turn_lifecycle: TypedSubscriber<TurnLifecycleEvent>,
    intermediate: TypedSubscriber<IntermediateEvent>,
    system: TypedSubscriber<SystemMessageEvent>,
}

impl DiscordSubscribers {
    /// Create all typed subscribers for a Discord connection.
    pub(crate) async fn new(
        bus_handle: &crate::bus::BusHandle,
        ep: EndpointName,
    ) -> Result<Self, crate::bus::BusError> {
        Ok(Self {
            response: bus_handle
                .subscribe_typed(topics::Response(ep.clone()))
                .await?,
            turn_lifecycle: bus_handle
                .subscribe_typed(topics::TurnLifecycle(ep.clone()))
                .await?,
            intermediate: bus_handle.subscribe_typed(topics::Intermediate(ep)).await?,
            system: bus_handle.subscribe_typed(topics::SystemMessage).await?,
        })
    }
}

/// Receives events from the bus and delivers them to the Discord DM channel.
pub(crate) async fn run_discord_subscriber(
    mut subs: DiscordSubscribers,
    http: Arc<serenity::http::Http>,
    channel_id: Arc<Mutex<Option<ChannelId>>>,
) {
    let mut typing_cancel: Option<tokio::sync::watch::Sender<bool>> = None;

    loop {
        let Some(cid) = *channel_id.lock().await else {
            // No channel yet — wait a bit and retry
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        };

        tokio::select! {
            event = subs.turn_lifecycle.recv() => {
                match event {
                    Ok(Some(TurnLifecycleEvent::Started { .. })) => {
                        let h = Arc::clone(&http);
                        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
                        typing_cancel = Some(stop_tx);
                        tokio::spawn(async move {
                            loop {
                                if let Err(e) = cid.broadcast_typing(&h).await {
                                    tracing::trace!(error = %e, "discord typing indicator failed");
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
                    _ => break,
                }
            }
            event = subs.response.recv() => {
                match event {
                    Ok(Some(resp)) => send_chunks(&http, cid, &resp.content).await,
                    _ => break,
                }
            }
            event = subs.intermediate.recv() => {
                match event {
                    Ok(Some(im)) => send_chunks(&http, cid, &im.content).await,
                    _ => break,
                }
            }
            event = subs.system.recv() => {
                match event {
                    Ok(Some(SystemMessageEvent::Notice { message })) => {
                        send_chunks(&http, cid, &message).await;
                    }
                    Ok(Some(SystemMessageEvent::Error { message, .. })) => {
                        let text = format!("**Error:** {message}");
                        send_chunks(&http, cid, &text).await;
                    }
                    Ok(Some(SystemMessageEvent::Event(se))) => {
                        let text = format!("**[{}]** {}", se.source, se.content);
                        send_chunks(&http, cid, &text).await;
                    }
                    _ => break,
                }
            }
        }
    }

    tracing::debug!("discord subscriber loop ended (broker shut down)");
}

async fn send_chunks(http: &serenity::http::Http, channel_id: ChannelId, content: &str) {
    let chunks = chunk_text(content, DISCORD_MAX_CHARS);
    for chunk in chunks {
        if let Err(e) = channel_id.say(http, &chunk).await {
            tracing::warn!(error = %e, "failed to send discord message");
        }
    }
}

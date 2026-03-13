//! Discord bus subscriber — translates `BusEvent`s to Discord DM messages.

use std::sync::Arc;

use serenity::model::id::ChannelId;
use tokio::sync::Mutex;

use crate::bus::{BusEvent, Subscriber};
use crate::interfaces::chunking::chunk_text;

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval between typing indicator re-sends (seconds).
///
/// Discord's typing indicator lasts ~10s, so 8s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 8;

/// Receives events from the bus and delivers them to the Discord DM channel.
pub(crate) async fn run_discord_subscriber(
    mut subscriber: Subscriber,
    http: Arc<serenity::http::Http>,
    channel_id: Arc<Mutex<Option<ChannelId>>>,
) {
    let mut typing_cancel: Option<tokio::sync::watch::Sender<bool>> = None;

    while let Some(event) = subscriber.recv().await {
        let Some(cid) = *channel_id.lock().await else {
            tracing::trace!("discord subscriber: no channel_id yet, skipping event");
            continue;
        };

        match event {
            BusEvent::TurnStarted { .. } => {
                // Start a typing indicator loop
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
            BusEvent::TurnEnded { .. } => {
                // Cancel typing loop
                typing_cancel.take();
            }
            BusEvent::Response(resp) => {
                send_chunks(&http, cid, &resp.content).await;
            }
            BusEvent::Intermediate(im) => {
                send_chunks(&http, cid, &im.content).await;
            }
            BusEvent::SystemEvent(se) => {
                let text = format!("**[{}]** {}", se.source, se.content);
                send_chunks(&http, cid, &text).await;
            }
            // Tool calls/results are not surfaced in Discord
            BusEvent::ToolCall(_)
            | BusEvent::ToolResult(_)
            | BusEvent::Message(_)
            | BusEvent::Notification(_)
            | BusEvent::AgentResult(_)
            | BusEvent::WebhookPayload { .. } => {}
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

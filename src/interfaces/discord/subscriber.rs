//! Discord bus subscriber — translates typed bus events to Discord messages.

use std::collections::HashMap;
use std::sync::Arc;

use serenity::model::id::ChannelId;

use serenity::http::Http;

use crate::bus::{ErrorEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent};
use crate::interfaces::chunking::chunk_text;

use super::DiscordState;

/// Maximum message length for Discord.
const DISCORD_MAX_CHARS: usize = 2000;

/// Interval between typing indicator re-sends (seconds).
///
/// Discord's typing indicator lasts ~10s, so 8s provides overlap.
const TYPING_INTERVAL_SECS: u64 = 8;

/// Receives events from the bus and delivers them to Discord.
pub(super) async fn run_discord_subscriber(
    mut subs: crate::interfaces::BaseSubscribers,
    http: Arc<serenity::http::Http>,
    state: Arc<DiscordState>,
) {
    // One typing loop per in-flight turn, stopped by dropping its sender.
    let mut typing: HashMap<String, tokio::sync::watch::Sender<()>> = HashMap::new();
    let mut clean_exit = true;

    loop {
        tokio::select! {
            event = subs.turn_lifecycle.recv() => {
                match event {
                    Ok(Some(TurnLifecycleEvent::Started { correlation_id })) => {
                        if let Some(cid) = state.target_for(&correlation_id).await {
                            typing.insert(correlation_id, spawn_typing(Arc::clone(&http), cid));
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
                    Ok(Some(resp)) => deliver_response(&http, &state, resp).await,
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.intermediate.recv() => {
                match event {
                    Ok(Some(im)) => {
                        if let Some(cid) = target_or_warn(&state, &im.correlation_id).await {
                            send_or_warn(&http, cid, &im.content).await;
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
                        if let Some(cid) = state.owner_dm().await {
                            send_or_warn(&http, cid, &message).await;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
            event = subs.error.recv() => {
                match event {
                    Ok(Some(ErrorEvent { message, .. })) => {
                        if let Some(cid) = state.owner_dm().await {
                            send_or_warn(&http, cid, &format!("**Error:** {message}")).await;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => { clean_exit = false; break; }
                }
            }
        }
    }

    if clean_exit {
        tracing::debug!("discord subscriber loop ended");
    } else {
        tracing::warn!("discord subscriber loop ended unexpectedly");
    }
}

async fn target_or_warn(state: &DiscordState, correlation_id: &str) -> Option<ChannelId> {
    let target = state.target_for(correlation_id).await;
    if target.is_none() {
        tracing::warn!(
            correlation_id,
            "no discord channel to deliver to; the owner has not messaged the bot yet"
        );
    }
    target
}

fn spawn_typing(
    http: Arc<serenity::http::Http>,
    channel_id: ChannelId,
) -> tokio::sync::watch::Sender<()> {
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        loop {
            if let Err(e) = channel_id.broadcast_typing(&http).await {
                tracing::trace!(error = %e, "discord typing indicator failed");
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

async fn deliver_response(http: &Http, state: &DiscordState, resp: ResponseEvent) {
    let target = match &resp.conversation {
        Some(id) => match id.parse::<u64>() {
            Ok(n) if n != 0 => ChannelId::new(n),
            _ => {
                tracing::warn!(conversation = %id, "discord message addressed to an invalid channel ID");
                notify_owner_of_failure(http, state, id, "that is not a Discord channel ID").await;
                return;
            }
        },
        None => match target_or_warn(state, &resp.correlation_id).await {
            Some(target) => target,
            None => return,
        },
    };
    let sent = if let Some(ref att) = resp.attachment {
        send_file_attachment(http, target, att, &resp.content).await
    } else if resp.content.is_empty() {
        Ok(())
    } else {
        send_chunks(http, target, &resp.content).await
    };
    if let Err(e) = sent {
        tracing::warn!(channel_id = %target, error = %e, "failed to deliver discord message");
        if resp.conversation.is_some() {
            let place = state
                .cached_label(target)
                .unwrap_or_else(|| format!("channel {target}"));
            notify_owner_of_failure(http, state, &place, &e.to_string()).await;
        }
    }
}

/// Tell the owner a message the agent addressed to a specific conversation
/// did not go out, since nobody else will see that it failed.
async fn notify_owner_of_failure(http: &Http, state: &DiscordState, place: &str, reason: &str) {
    if let Some(dm) = state.owner_dm().await {
        send_or_warn(
            http,
            dm,
            &format!("**Error:** I couldn't post a message to {place}: {reason}"),
        )
        .await;
    }
}

async fn send_or_warn(http: &Http, channel_id: ChannelId, content: &str) {
    if let Err(e) = send_chunks(http, channel_id, content).await {
        tracing::warn!(channel_id = %channel_id, error = %e, "failed to send discord message");
    }
}

/// Send `content` in chunks, stopping at the first failure.
async fn send_chunks(
    http: &Http,
    channel_id: ChannelId,
    content: &str,
) -> Result<(), Box<serenity::Error>> {
    for chunk in chunk_text(content, DISCORD_MAX_CHARS) {
        channel_id.say(http, &chunk).await.map_err(Box::new)?;
    }
    Ok(())
}

async fn send_file_attachment(
    http: &Http,
    channel_id: ChannelId,
    attachment: &crate::interfaces::attachment::FileAttachment,
    caption: &str,
) -> Result<(), Box<serenity::Error>> {
    use serenity::builder::{CreateAttachment, CreateMessage};

    let file_attachment = CreateAttachment::path(&attachment.path)
        .await
        .map_err(Box::new)?;
    let mut message = CreateMessage::new().add_file(file_attachment);
    if !caption.is_empty() {
        message = message.content(caption);
    }
    channel_id
        .send_message(http, message)
        .await
        .map_err(Box::new)?;
    tracing::debug!(
        filename = %attachment.filename,
        endpoint = "discord",
        "file delivered"
    );
    Ok(())
}

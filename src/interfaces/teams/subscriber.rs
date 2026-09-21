//! Delivers agent output from the bus to Teams conversations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::bus::{ErrorEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent};
use crate::interfaces::BaseSubscribers;

use super::TeamsRuntime;
use super::connector::typing_activity;
use super::store::ConversationRef;

/// Per-message text budget. Teams rejects activities over ~28 KB; this
/// leaves room for the JSON envelope and multi-byte characters.
pub(super) const MAX_MESSAGE_BYTES: usize = 20_000;
/// Teams shows a typing indicator for roughly three seconds.
const TYPING_INTERVAL: Duration = Duration::from_secs(3);

pub(super) async fn run_teams_subscriber(rt: Arc<TeamsRuntime>, mut subs: BaseSubscribers) {
    // One typing loop per in-flight turn, stopped by dropping its sender.
    let mut typing: HashMap<String, tokio::sync::watch::Sender<()>> = HashMap::new();

    loop {
        tokio::select! {
            event = subs.turn_lifecycle.recv() => match event {
                Ok(Some(TurnLifecycleEvent::Started { correlation_id })) => {
                    if let Some(target) = rt.target_for(&correlation_id).await {
                        typing.insert(correlation_id, spawn_typing(Arc::clone(&rt), target));
                    }
                }
                Ok(Some(TurnLifecycleEvent::Ended { correlation_id })) => {
                    typing.remove(&correlation_id);
                    rt.release_reply_target(&correlation_id);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams lifecycle subscription failed");
                    break;
                }
            },
            event = subs.response.recv() => match event {
                Ok(Some(response)) => deliver_response(&rt, response).await,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams response subscription failed");
                    break;
                }
            },
            event = subs.intermediate.recv() => match event {
                Ok(Some(im)) => {
                    if let Some(target) = target_or_warn(&rt, &im.correlation_id).await {
                        rt.send_text(&target, &im.content).await;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams intermediate subscription failed");
                    break;
                }
            },
            // System notices and errors can carry internals; they only ever
            // go to the owner, never into a shared chat.
            event = subs.notice.recv() => match event {
                Ok(Some(NoticeEvent { message })) => {
                    if let Some(dm) = rt.owner_dm().await {
                        rt.send_text(&dm, &message).await;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams notice subscription failed");
                    break;
                }
            },
            event = subs.error.recv() => match event {
                Ok(Some(ErrorEvent { message, .. })) => {
                    if let Some(dm) = rt.owner_dm().await {
                        rt.send_text(&dm, &format!("**Error:** {message}")).await;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams error subscription failed");
                    break;
                }
            },
        }
    }
    tracing::debug!("teams subscriber loop ended");
}

async fn target_or_warn(rt: &TeamsRuntime, correlation_id: &str) -> Option<ConversationRef> {
    let target = rt.target_for(correlation_id).await;
    if target.is_none() {
        tracing::warn!(
            correlation_id,
            "no teams conversation to deliver to; the owner has not messaged the bot yet"
        );
    }
    target
}

async fn deliver_response(rt: &TeamsRuntime, response: ResponseEvent) {
    let Some(target) = target_or_warn(rt, &response.correlation_id).await else {
        return;
    };
    if let Some(attachment) = &response.attachment {
        // Bots can only send files in Teams through a consent-card upload
        // flow, which this interface does not implement.
        tracing::warn!(
            file = %attachment.path.display(),
            "teams cannot deliver file attachments; sending the text with a note"
        );
        let note = format!(
            "{}\n\n_I made a file for you ({}), but I can't send files over Teams yet. \
             It's saved at `{}` and available in the web UI._",
            response.content,
            attachment.filename,
            attachment.path.display()
        );
        rt.send_text(&target, note.trim_start()).await;
    } else if !response.content.is_empty() {
        rt.send_text(&target, &response.content).await;
    }
}

fn spawn_typing(rt: Arc<TeamsRuntime>, target: ConversationRef) -> tokio::sync::watch::Sender<()> {
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        loop {
            if let Err(e) = rt
                .connector
                .send_activity(&target, &typing_activity())
                .await
            {
                tracing::trace!(error = %e, "teams typing indicator failed");
            }
            tokio::select! {
                () = tokio::time::sleep(TYPING_INTERVAL) => {}
                // Resolves with an error once the sender is dropped at turn end.
                _ = stop_rx.changed() => break,
            }
        }
    });
    stop_tx
}

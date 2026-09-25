//! Delivers agent output from the bus to Teams conversations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::bus::{
    ConversationTypingEvent, ErrorEvent, NoticeEvent, ResponseEvent, SessionResponseEvent,
    TurnLifecycleEvent,
};
use crate::interfaces::BaseSubscribers;
use crate::interfaces::notify_main_of_undeliverable_session_output;

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
    // Same, but for conversation sessions' own turns — keyed by conversation
    // id rather than correlation id; see `crate::interfaces::BaseSubscribers`.
    let mut conversation_typing: HashMap<String, tokio::sync::watch::Sender<()>> = HashMap::new();

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
                    rt.reply_targets.release(&correlation_id);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams lifecycle subscription failed");
                    break;
                }
            },
            event = subs.conversation_typing.recv() => match event {
                Ok(Some(ConversationTypingEvent { conversation_id, active: true })) => {
                    if let Some(target) = rt.store.conversation(&conversation_id).await {
                        conversation_typing
                            .insert(conversation_id, spawn_typing(Arc::clone(&rt), target));
                    }
                }
                Ok(Some(ConversationTypingEvent { conversation_id, active: false })) => {
                    conversation_typing.remove(&conversation_id);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams conversation-typing subscription failed");
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
            event = subs.session_response.recv() => match event {
                Ok(Some(response)) => deliver_session_response(&rt, response).await,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "teams session response subscription failed");
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
    let target = match &response.conversation {
        Some(id) => {
            let Some(target) = rt.store.conversation(id).await else {
                tracing::warn!(conversation = %id, "teams message addressed to an unknown conversation");
                notify_owner_of_failure(rt, id, "the bot no longer knows that conversation").await;
                return;
            };
            target
        }
        None => match target_or_warn(rt, &response.correlation_id).await {
            Some(target) => target,
            None => return,
        },
    };
    let text = match &response.attachment {
        Some(attachment) => {
            // Bots can only send files in Teams through a consent-card upload
            // flow, which this interface does not implement.
            tracing::warn!(
                file = %attachment.path.display(),
                "teams cannot deliver file attachments; sending the text with a note"
            );
            format!(
                "{}\n\n_I made a file for you ({}), but I can't send files over Teams yet. \
                 It's saved at `{}` and available in the web UI._",
                response.content,
                attachment.filename,
                attachment.path.display()
            )
            .trim_start()
            .to_string()
        }
        None if response.content.is_empty() => return,
        None => response.content,
    };
    if let Err(e) = rt.try_send_text(&target, &text).await {
        tracing::error!(error = %e, conversation = %target.label, "failed to send teams message");
        if response.conversation.is_some() {
            notify_owner_of_failure(rt, &target.label, &e.to_string()).await;
        }
    }
}

/// Deliver a conversation session's turn output to its own Teams conversation.
///
/// Never falls back to the owner's DM on an unresolvable target — unlike
/// [`deliver_response`], which the main agent's own delivery still uses.
/// Either failure mode here (an unknown conversation, or the send itself
/// failing) drops the output and notifies main instead, per the design's
/// "only main talks to the owner" rule.
async fn deliver_session_response(rt: &TeamsRuntime, resp: SessionResponseEvent) {
    let Some(target) = rt.store.conversation(&resp.conversation_id).await else {
        notify_main_of_undeliverable_session_output(
            &rt.publisher,
            &resp.session_address,
            &resp.conversation_id,
            "the bot no longer knows that conversation",
        )
        .await;
        return;
    };
    let text = match &resp.attachment {
        Some(attachment) => {
            tracing::warn!(
                file = %attachment.path.display(),
                "teams cannot deliver file attachments; sending the text with a note"
            );
            format!(
                "{}\n\n_I made a file for you ({}), but I can't send files over Teams yet. \
                 It's saved at `{}` and available in the web UI._",
                resp.content,
                attachment.filename,
                attachment.path.display()
            )
            .trim_start()
            .to_string()
        }
        None if resp.content.is_empty() => return,
        None => resp.content,
    };
    if let Err(e) = rt.try_send_text(&target, &text).await {
        notify_main_of_undeliverable_session_output(
            &rt.publisher,
            &resp.session_address,
            &resp.conversation_id,
            &e.to_string(),
        )
        .await;
    }
}

/// Tell the owner a message the agent addressed to a specific conversation
/// did not go out, since nobody else will see that it failed.
async fn notify_owner_of_failure(rt: &TeamsRuntime, place: &str, reason: &str) {
    if let Some(dm) = rt.owner_dm().await {
        rt.send_text(
            &dm,
            &format!("**Error:** I couldn't post a message to {place}: {reason}"),
        )
        .await;
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

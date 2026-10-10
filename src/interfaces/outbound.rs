//! Shared outbound loop for Discord, Telegram, and Teams.
//!
//! Each platform implements [`ChatOutbound`] for its own connection, id
//! parsing, and send API. This loop is the delivery policy those three
//! share: where a turn's replies go, typing indicators, owner-only notices,
//! and the rule that a conversation session never falls back to the owner's
//! direct messages.

use std::ops::ControlFlow;

use async_trait::async_trait;

use crate::bus::{
    BusError, ConversationTypingEvent, ErrorEvent, IntermediateEvent, NoticeEvent, ResponseEvent,
    SessionResponseEvent, TurnLifecycleEvent,
};
use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::notify_main_of_undeliverable_session_output;

use super::BaseSubscribers;

/// What a chat platform provides so [`run`] can deliver bus events.
#[async_trait]
pub(crate) trait ChatOutbound: Send + Sync {
    /// Platform conversation handle (a channel id, chat id, or Teams ref).
    type Target: Clone + Send + Sync + 'static;

    /// Short name used in logs (`"discord"`, `"telegram"`, `"teams"`).
    fn name(&self) -> &'static str;

    /// Owner-facing reason when `conversation_id` does not resolve.
    fn unknown_conversation_reason(&self) -> &'static str;

    /// Publishes a notice to main when a session's own output cannot be delivered.
    fn publisher(&self) -> &crate::bus::Publisher;

    /// Where a main-agent turn replies: the conversation that started it, or
    /// the owner's direct messages for proactive output.
    async fn reply_target(&self, correlation_id: &str) -> Option<Self::Target>;

    /// Drop the reply mapping for a turn that has ended.
    fn release_reply(&self, correlation_id: &str);

    /// Resolve a conversation id to a send target. `None` when the id is not
    /// one this platform can post to.
    async fn conversation_target(&self, conversation_id: &str) -> Option<Self::Target>;

    /// The owner's direct-message target, when the platform knows one.
    async fn owner(&self) -> Option<Self::Target>;

    /// Human-readable place name for an owner-facing delivery failure.
    async fn describe(&self, conversation_id: &str, target: &Self::Target) -> String;

    /// Whether system notices and errors mirror to the owner's direct
    /// messages on this platform. Adapters without a knob keep the stock
    /// always-on behavior.
    fn mirror_system_notices(&self) -> bool {
        true
    }

    /// Log-facing label for a target (an id or a conversation name).
    fn target_label(&self, target: &Self::Target) -> String;

    /// Start a typing indicator. Dropping the sender stops it.
    fn start_typing(&self, target: Self::Target) -> tokio::sync::watch::Sender<()>;

    /// Send text, or a file when `attachment` is set.
    ///
    /// Empty content with no attachment succeeds without sending. A file the
    /// platform cannot transmit is the platform's to turn into text.
    ///
    /// # Errors
    /// Returns a plain-language reason when the send fails. The loop decides
    /// who hears about it.
    async fn send(
        &self,
        target: &Self::Target,
        content: &str,
        attachment: Option<&FileAttachment>,
    ) -> Result<(), String>;
}

/// Receive bus events for one chat platform until a subscription closes.
pub(crate) async fn run<C: ChatOutbound>(mut subs: BaseSubscribers, chat: C) {
    // One typing loop per in-flight turn, stopped by dropping its sender.
    let mut typing: std::collections::HashMap<String, tokio::sync::watch::Sender<()>> =
        std::collections::HashMap::new();
    // Same, for conversation sessions' own turns — keyed by conversation id
    // rather than correlation id, since a session's turn isn't a reply to
    // any one message.
    let mut conversation_typing: std::collections::HashMap<String, tokio::sync::watch::Sender<()>> =
        std::collections::HashMap::new();
    let clean_exit;

    loop {
        tokio::select! {
            event = subs.turn_lifecycle.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(TurnLifecycleEvent::Started { correlation_id }) => {
                        if let Some(target) = chat.reply_target(&correlation_id).await {
                            typing.insert(correlation_id, chat.start_typing(target));
                        }
                    }
                    ControlFlow::Continue(TurnLifecycleEvent::Ended { correlation_id }) => {
                        typing.remove(&correlation_id);
                        chat.release_reply(&correlation_id);
                    }
                }
            }
            event = subs.conversation_typing.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(ConversationTypingEvent { conversation_id, active: true }) => {
                        if let Some(target) = chat.conversation_target(&conversation_id).await {
                            conversation_typing.insert(conversation_id, chat.start_typing(target));
                        }
                    }
                    ControlFlow::Continue(ConversationTypingEvent { conversation_id, active: false }) => {
                        conversation_typing.remove(&conversation_id);
                    }
                }
            }
            event = subs.response.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(resp) => deliver_main(&chat, resp).await,
                }
            }
            event = subs.session_response.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(resp) => deliver_session(&chat, resp).await,
                }
            }
            event = subs.intermediate.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(im) => deliver_intermediate(&chat, im).await,
                }
            }
            // System notices and errors can carry internals; they only ever
            // go to the owner.
            event = subs.notice.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(NoticeEvent { message }) => {
                        if chat.mirror_system_notices() {
                            send_to_owner(&chat, &message).await;
                        }
                    }
                }
            }
            event = subs.error.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(ErrorEvent { message, .. }) => {
                        if chat.mirror_system_notices() {
                            send_to_owner(&chat, &format!("**Error:** {message}")).await;
                        }
                    }
                }
            }
        }
    }

    log_subscriber_exit(chat.name(), clean_exit);
}

/// `Break(true)` is a clean close. `Break(false)` is a failed subscription.
fn take_event<T>(event: Result<Option<T>, BusError>, interface: &str) -> ControlFlow<bool, T> {
    match event {
        Ok(Some(value)) => ControlFlow::Continue(value),
        Ok(None) => ControlFlow::Break(true),
        Err(e) => {
            tracing::warn!(error = %e, interface, "subscriber subscription failed");
            ControlFlow::Break(false)
        }
    }
}

async fn deliver_intermediate<C: ChatOutbound>(chat: &C, event: IntermediateEvent) {
    let Some(target) = reply_target_or_warn(chat, &event.correlation_id).await else {
        return;
    };
    if let Err(reason) = chat.send(&target, &event.content, None).await {
        tracing::warn!(
            interface = chat.name(),
            target = %chat.target_label(&target),
            error = %reason,
            "failed to send intermediate message"
        );
    }
}

fn log_subscriber_exit(interface: &str, clean_exit: bool) {
    if clean_exit {
        tracing::debug!(interface, "subscriber loop ended");
    } else {
        tracing::warn!(interface, "subscriber loop ended unexpectedly");
    }
}

async fn deliver_main<C: ChatOutbound>(chat: &C, response: ResponseEvent) {
    let (target, addressed) = if let Some(id) = response.conversation.clone() {
        let Some(target) = chat.conversation_target(&id).await else {
            tracing::warn!(
                interface = chat.name(),
                conversation = %id,
                "message addressed to an unknown conversation"
            );
            notify_owner(chat, &id, chat.unknown_conversation_reason()).await;
            return;
        };
        (target, Some(id))
    } else {
        let Some(target) = reply_target_or_warn(chat, &response.correlation_id).await else {
            return;
        };
        (target, None)
    };
    if let Err(reason) = chat
        .send(&target, &response.content, response.attachment.as_ref())
        .await
    {
        tracing::warn!(
            interface = chat.name(),
            target = %chat.target_label(&target),
            error = %reason,
            "failed to deliver message"
        );
        if let Some(id) = addressed {
            let place = chat.describe(&id, &target).await;
            notify_owner(chat, &place, &reason).await;
        }
    }
}

/// A conversation session's output never falls back to the owner's direct
/// messages. Either failure mode notifies main instead.
async fn deliver_session<C: ChatOutbound>(chat: &C, response: SessionResponseEvent) {
    let Some(target) = chat.conversation_target(&response.conversation_id).await else {
        notify_main_of_undeliverable_session_output(
            chat.publisher(),
            &response.session_address,
            &response.conversation_id,
            chat.unknown_conversation_reason(),
        )
        .await;
        return;
    };
    if let Err(reason) = chat
        .send(&target, &response.content, response.attachment.as_ref())
        .await
    {
        notify_main_of_undeliverable_session_output(
            chat.publisher(),
            &response.session_address,
            &response.conversation_id,
            &reason,
        )
        .await;
    }
}

async fn reply_target_or_warn<C: ChatOutbound>(
    chat: &C,
    correlation_id: &str,
) -> Option<C::Target> {
    let target = chat.reply_target(correlation_id).await;
    if target.is_none() {
        tracing::warn!(
            correlation_id,
            interface = chat.name(),
            "no conversation to deliver to; the owner has not messaged the bot yet"
        );
    }
    target
}

async fn send_to_owner<C: ChatOutbound>(chat: &C, text: &str) {
    let Some(owner) = chat.owner().await else {
        return;
    };
    if let Err(reason) = chat.send(&owner, text, None).await {
        tracing::warn!(
            interface = chat.name(),
            error = %reason,
            "failed to send a notice to the owner"
        );
    }
}

/// Tell the owner a message addressed to a specific conversation did not go
/// out, since nobody else will see that it failed.
async fn notify_owner<C: ChatOutbound>(chat: &C, place: &str, reason: &str) {
    send_to_owner(
        chat,
        &format!("**Error:** I couldn't post a message to {place}: {reason}"),
    )
    .await;
}

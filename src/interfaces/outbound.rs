//! Shared outbound loop for Discord, Telegram, and Teams.
//!
//! Each platform implements [`ChatOutbound`] for its own connection, id
//! parsing, and send API. This loop is the delivery policy those three
//! share: where a turn's replies go, typing indicators, and the rule that a
//! conversation session never falls back to the owner's direct messages.
//!
//! Only conversation goes out: agent replies, and a turn's failure in place
//! of its reply. System notices and errors stay in the web UI. A message the
//! agent addressed to a conversation that cannot be delivered becomes a web
//! UI notice and a note to main, never a chat message.

use std::ops::ControlFlow;

use async_trait::async_trait;

use crate::bus::{
    BusError, ConversationTypingEvent, ErrorEvent, IntermediateEvent, MessageEvent, ResponseEvent,
    SessionResponseEvent, TurnLifecycleEvent, topics,
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

    /// Plain-language reason when `conversation_id` does not resolve.
    fn unknown_conversation_reason(&self) -> &'static str;

    /// Publishes delivery-failure notices and notes to main.
    fn publisher(&self) -> &crate::bus::Publisher;

    /// Where a main-agent turn replies: the conversation that started it, or
    /// the owner's direct messages for proactive output.
    async fn reply_target(&self, correlation_id: &str) -> Option<Self::Target>;

    /// Drop the reply mapping for a turn that has ended.
    fn release_reply(&self, correlation_id: &str);

    /// Resolve a conversation id to a send target. `None` when the id is not
    /// one this platform can post to.
    async fn conversation_target(&self, conversation_id: &str) -> Option<Self::Target>;

    /// Human-readable place name for an owner-facing delivery failure.
    async fn describe(&self, conversation_id: &str, target: &Self::Target) -> String;

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
            event = subs.turn_error.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(err) => deliver_turn_error(&chat, err).await,
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
            report_undelivered_message(chat, &id, chat.unknown_conversation_reason()).await;
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
            report_undelivered_message(chat, &place, &reason).await;
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

/// A main-agent turn failed. The chat that started it hears why in place of
/// the reply; proactive turns tell the owner's direct messages, as their
/// replies would.
async fn deliver_turn_error<C: ChatOutbound>(chat: &C, event: ErrorEvent) {
    let Some(target) = reply_target_or_warn(chat, &event.correlation_id).await else {
        return;
    };
    let text = format!("**Error:** {}", event.message);
    if let Err(reason) = chat.send(&target, &text, None).await {
        tracing::warn!(
            interface = chat.name(),
            target = %chat.target_label(&target),
            error = %reason,
            "failed to send a turn error"
        );
    }
}

/// A message the agent addressed to a specific conversation did not go out.
/// The owner sees it in the web UI and main hears about it, since nobody in
/// the chat will.
async fn report_undelivered_message<C: ChatOutbound>(chat: &C, place: &str, reason: &str) {
    let text = format!(
        "I couldn't post a message to {place} on {}: {reason}",
        chat.name()
    );
    crate::gateway::helpers::publish_notice(chat.publisher(), text.clone()).await;
    let event = MessageEvent::from_background(format!("[Delivery Failed] {text}"));
    if let Err(e) = chat.publisher().publish(topics::UserMessage, event).await {
        tracing::warn!(
            interface = chat.name(),
            error = %e,
            "failed to notify main about an undelivered message"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::bus::{BusHandle, EndpointName, NoticeEvent, NotifyName, SYSTEM_CHANNEL};
    use crate::testing::{clock, wait};

    const ENDPOINT: &str = "fakechat";

    /// A chat platform that records what it sends. Targets are plain strings:
    /// `"owner-dm"` for the owner, otherwise a conversation id.
    #[derive(Clone)]
    struct FakeChat {
        publisher: crate::bus::Publisher,
        replies: HashMap<String, String>,
        conversations: Vec<String>,
        failing: Vec<String>,
        sent: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl FakeChat {
        fn new(handle: &BusHandle) -> Self {
            Self {
                publisher: handle.publisher(),
                replies: HashMap::new(),
                conversations: Vec::new(),
                failing: Vec::new(),
                sent: Arc::default(),
            }
        }

        fn sent(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ChatOutbound for FakeChat {
        type Target = String;

        fn name(&self) -> &'static str {
            "fakechat"
        }

        fn unknown_conversation_reason(&self) -> &'static str {
            "no such conversation"
        }

        fn publisher(&self) -> &crate::bus::Publisher {
            &self.publisher
        }

        async fn reply_target(&self, correlation_id: &str) -> Option<String> {
            Some(
                self.replies
                    .get(correlation_id)
                    .cloned()
                    .unwrap_or_else(|| "owner-dm".to_string()),
            )
        }

        fn release_reply(&self, _correlation_id: &str) {}

        async fn conversation_target(&self, conversation_id: &str) -> Option<String> {
            self.conversations
                .iter()
                .find(|c| *c == conversation_id)
                .cloned()
        }

        async fn describe(&self, conversation_id: &str, _target: &String) -> String {
            conversation_id.to_string()
        }

        fn target_label(&self, target: &String) -> String {
            target.clone()
        }

        fn start_typing(&self, _target: String) -> tokio::sync::watch::Sender<()> {
            tokio::sync::watch::channel(()).0
        }

        async fn send(
            &self,
            target: &String,
            content: &str,
            _attachment: Option<&FileAttachment>,
        ) -> Result<(), String> {
            if self.failing.contains(target) {
                return Err("the platform refused it".to_string());
            }
            self.sent
                .lock()
                .unwrap()
                .push((target.clone(), content.to_string()));
            Ok(())
        }
    }

    async fn start(handle: &BusHandle, chat: FakeChat) {
        let subs = BaseSubscribers::new(handle, EndpointName::from(ENDPOINT))
            .await
            .unwrap();
        tokio::spawn(run(subs, chat));
    }

    fn response(correlation_id: &str, content: &str, conversation: Option<&str>) -> ResponseEvent {
        ResponseEvent {
            correlation_id: correlation_id.to_string(),
            content: content.to_string(),
            timestamp: chrono::Utc::now().naive_utc(),
            attachment: None,
            conversation: conversation.map(str::to_string),
        }
    }

    async fn wait_for_sends(chat: &FakeChat, count: usize) {
        wait::until_true(format!("{count} send(s) to the chat"), || {
            chat.sent().len() >= count
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn system_notices_and_errors_never_reach_the_chat() {
        let handle = crate::bus::spawn_broker();
        let chat = FakeChat::new(&handle);
        start(&handle, chat.clone()).await;
        let publisher = handle.publisher();
        let system = || topics::Notification(NotifyName::from(SYSTEM_CHANNEL));

        publisher
            .publish(
                system(),
                NoticeEvent {
                    message: "Config reloaded".to_string(),
                },
            )
            .await
            .unwrap();
        publisher
            .publish(
                system(),
                ErrorEvent {
                    correlation_id: String::new(),
                    message: "memory index failed".to_string(),
                    details: None,
                },
            )
            .await
            .unwrap();
        // A reply sent after them proves the loop has had its chance to
        // deliver both.
        publisher
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                response("m1", "hello", None),
            )
            .await
            .unwrap();

        wait_for_sends(&chat, 1).await;
        clock::elapse(Duration::from_millis(50)).await;
        assert_eq!(
            chat.sent(),
            vec![("owner-dm".to_string(), "hello".to_string())],
            "only the agent's reply may go out; system output stays in the web UI"
        );
    }

    #[tokio::test]
    async fn turn_error_answers_the_conversation_that_started_the_turn() {
        let handle = crate::bus::spawn_broker();
        let mut chat = FakeChat::new(&handle);
        chat.replies.insert("m1".to_string(), "group-7".to_string());
        start(&handle, chat.clone()).await;

        handle
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                ErrorEvent {
                    correlation_id: "m1".to_string(),
                    message: "The model is unavailable.".to_string(),
                    details: Some("503 from provider".to_string()),
                },
            )
            .await
            .unwrap();

        wait_for_sends(&chat, 1).await;
        assert_eq!(
            chat.sent(),
            vec![(
                "group-7".to_string(),
                "**Error:** The model is unavailable.".to_string()
            )],
            "the failure goes where the reply would have, without technical details"
        );
    }

    #[tokio::test]
    async fn undeliverable_addressed_message_becomes_a_notice_not_a_chat_message() {
        let handle = crate::bus::spawn_broker();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut main: crate::bus::Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let mut chat = FakeChat::new(&handle);
        chat.conversations.push("group-7".to_string());
        chat.failing.push("group-7".to_string());
        start(&handle, chat.clone()).await;

        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();
        publisher
            .publish(endpoint(), response("m1", "hi all", Some("group-7")))
            .await
            .unwrap();
        publisher
            .publish(endpoint(), response("m2", "hi ghost", Some("gone-1")))
            .await
            .unwrap();

        for place in ["group-7", "gone-1"] {
            let notice = notices.recv().await.unwrap().unwrap();
            assert!(
                notice.message.contains(place),
                "notice should name {place}: {}",
                notice.message
            );
            let note = main.recv().await.unwrap().unwrap();
            assert!(note.content.starts_with("[Delivery Failed]"));
            assert!(note.content.contains(place));
        }
        assert!(
            chat.sent().is_empty(),
            "a delivery failure must not be posted to the owner's chat"
        );
    }
}

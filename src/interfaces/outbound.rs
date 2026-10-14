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
//! UI notice and a note to main, never a chat message. Replies that stop
//! getting through are reported the same way, once per run of failures (see
//! [`DeliveryHealth`]).

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

    /// Product name the owner knows the platform by, for notices.
    fn display_name(&self) -> &'static str;

    /// What the owner can do about a failed send, in plain language, when the
    /// platform recognizes the cause in `reason` (the error [`Self::send`]
    /// returned). `None` when the cause has no known fix.
    fn explain_failure(&self, _reason: &str) -> Option<String> {
        None
    }

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
    let mut health = DeliveryHealth::default();
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
                    ControlFlow::Continue(resp) => deliver_main(&chat, &mut health, resp).await,
                }
            }
            event = subs.session_response.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(resp) => deliver_session(&chat, &mut health, resp).await,
                }
            }
            event = subs.intermediate.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(im) => deliver_intermediate(&chat, &mut health, im).await,
                }
            }
            event = subs.turn_error.recv() => {
                match take_event(event, chat.name()) {
                    ControlFlow::Break(clean) => {
                        clean_exit = clean;
                        break;
                    }
                    ControlFlow::Continue(err) => deliver_turn_error(&chat, &mut health, err).await,
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

/// Whether this interface's replies have been getting through. The loop owns
/// one per interface. A reply nobody receives looks, from the chat, like the
/// agent ignoring the message, so the first failure after working delivery
/// tells the owner and main; the rest of that run of failures stays in the
/// logs, and the next success logs the recovery and starts over.
#[derive(Default)]
struct DeliveryHealth {
    /// Replies that failed since the last send that got through.
    failures_in_a_row: u32,
}

impl DeliveryHealth {
    /// A send got through.
    fn delivered(&mut self, interface: &str) {
        if self.failures_in_a_row > 0 {
            tracing::info!(
                interface,
                failed_replies = self.failures_in_a_row,
                "replies are being delivered again"
            );
            self.failures_in_a_row = 0;
        }
    }

    /// A reply did not get through. Reported when it starts a run of
    /// failures.
    async fn failed<C: ChatOutbound>(&mut self, chat: &C, reason: &str) {
        self.failures_in_a_row = self.failures_in_a_row.saturating_add(1);
        if self.failures_in_a_row == 1 {
            report_failing_replies(chat, reason).await;
        }
    }
}

/// Send a reply to `target` and record the outcome in `health`.
async fn send_reply<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    target: &C::Target,
    reply: OutgoingReply<'_>,
) {
    match chat.send(target, reply.content, reply.attachment).await {
        Ok(()) => health.delivered(chat.name()),
        Err(reason) => {
            tracing::warn!(
                interface = chat.name(),
                target = %chat.target_label(target),
                error = %reason,
                reply = reply.what,
                "failed to deliver a reply"
            );
            health.failed(chat, &reason).await;
        }
    }
}

/// One reply [`send_reply`] sends: its text, an optional file, and what kind
/// of reply it is for the log.
struct OutgoingReply<'a> {
    content: &'a str,
    attachment: Option<&'a FileAttachment>,
    what: &'static str,
}

async fn deliver_intermediate<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    event: IntermediateEvent,
) {
    let Some(target) = reply_target_or_warn(chat, &event.correlation_id).await else {
        return;
    };
    let reply = OutgoingReply {
        content: &event.content,
        attachment: None,
        what: "intermediate message",
    };
    send_reply(chat, health, &target, reply).await;
}

fn log_subscriber_exit(interface: &str, clean_exit: bool) {
    if clean_exit {
        tracing::debug!(interface, "subscriber loop ended");
    } else {
        tracing::warn!(interface, "subscriber loop ended unexpectedly");
    }
}

/// A reply's failure is reported through `health`. A message the agent
/// addressed to a conversation is reported every time it fails, since the
/// cause may be that one conversation and the agent chose to send it there.
async fn deliver_main<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    response: ResponseEvent,
) {
    let Some(id) = response.conversation.as_deref() else {
        let Some(target) = reply_target_or_warn(chat, &response.correlation_id).await else {
            return;
        };
        let reply = OutgoingReply {
            content: &response.content,
            attachment: response.attachment.as_ref(),
            what: "reply",
        };
        send_reply(chat, health, &target, reply).await;
        return;
    };
    let Some(target) = chat.conversation_target(id).await else {
        tracing::warn!(
            interface = chat.name(),
            conversation = %id,
            "message addressed to an unknown conversation"
        );
        report_undelivered_message(chat, id, chat.unknown_conversation_reason()).await;
        return;
    };
    match chat
        .send(&target, &response.content, response.attachment.as_ref())
        .await
    {
        Ok(()) => health.delivered(chat.name()),
        Err(reason) => {
            tracing::warn!(
                interface = chat.name(),
                target = %chat.target_label(&target),
                error = %reason,
                "failed to deliver message"
            );
            let place = chat.describe(id, &target).await;
            report_undelivered_message(chat, &place, &reason).await;
        }
    }
}

/// A conversation session's output never falls back to the owner's direct
/// messages. Either failure mode notifies main instead.
async fn deliver_session<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    response: SessionResponseEvent,
) {
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
    match chat
        .send(&target, &response.content, response.attachment.as_ref())
        .await
    {
        Ok(()) => health.delivered(chat.name()),
        Err(reason) => {
            notify_main_of_undeliverable_session_output(
                chat.publisher(),
                &response.session_address,
                &response.conversation_id,
                &reason,
            )
            .await;
        }
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
async fn deliver_turn_error<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    event: ErrorEvent,
) {
    let Some(target) = reply_target_or_warn(chat, &event.correlation_id).await else {
        return;
    };
    let text = format!("**Error:** {}", event.message);
    let reply = OutgoingReply {
        content: &text,
        attachment: None,
        what: "turn error",
    };
    send_reply(chat, health, &target, reply).await;
}

/// Replies on this interface have started failing, so the chat sees the
/// agent ignore its messages. The owner gets a plain-language error in the
/// web UI, with the platform's own wording as its details, and main hears
/// about it so it knows the chat is not seeing its answers.
async fn report_failing_replies<C: ChatOutbound>(chat: &C, reason: &str) {
    let platform = chat.display_name();
    let fix = chat.explain_failure(reason);
    let next_step = fix.clone().unwrap_or_else(|| {
        format!(
            "Check the {platform} connection in Settings → Connections; \
             the details show what {platform} reported."
        )
    });
    crate::gateway::helpers::publish_error(
        chat.publisher(),
        format!(
            "Replies to {platform} aren't getting through, so messages there look unanswered. \
             The replies are still here in the web UI. {next_step}"
        ),
        Some(reason.to_string()),
    )
    .await;
    let fix_note = fix.map(|fix| format!(" {fix}")).unwrap_or_default();
    let event = MessageEvent::from_background(format!(
        "[Delivery Failed] Your replies on {platform} aren't reaching the chat: {reason}.{fix_note} \
         The user has been told in the web UI. Further failures on {platform} won't be reported \
         until a reply gets through again."
    ));
    if let Err(e) = chat.publisher().publish(topics::UserMessage, event).await {
        tracing::warn!(
            interface = chat.name(),
            error = %e,
            "failed to notify main that replies are failing"
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

        fn display_name(&self) -> &'static str {
            "FakeChat"
        }

        fn explain_failure(&self, reason: &str) -> Option<String> {
            reason
                .contains("refused")
                .then(|| "Reconnect FakeChat.".to_string())
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

    /// The one note main received before the first that names `marker`.
    async fn only_note_to_main_before(
        main: &mut crate::bus::Subscriber<MessageEvent>,
        marker: &str,
    ) -> String {
        let mut before = Vec::new();
        loop {
            let note = wait::next_event(format!("a note to main naming {marker}"), main).await;
            if note.content.contains(marker) {
                break;
            }
            before.push(note.content);
        }
        let [note] = <[String; 1]>::try_from(before).unwrap_or_else(|before| {
            panic!("expected exactly one note to main before {marker}, got {before:?}")
        });
        note
    }

    /// Waits for the owner-facing error that says `FakeChat` replies are failing.
    async fn expect_failing_replies_error(errors: &mut crate::bus::Subscriber<ErrorEvent>) {
        let error = wait::next_event("the failing-replies error", errors).await;
        assert!(
            error.message.contains("FakeChat") && error.message.contains("Reconnect FakeChat."),
            "the error should name the platform and its fix: {}",
            error.message
        );
        assert!(
            !error.message.contains("refused"),
            "the platform's raw reason belongs in the details: {}",
            error.message
        );
        assert_eq!(error.details.as_deref(), Some("the platform refused it"));
    }

    fn assert_failing_replies_note(note: &str) {
        assert!(
            note.starts_with("[Delivery Failed]")
                && note.contains("FakeChat")
                && note.contains("the platform refused it"),
            "main should hear that FakeChat replies are failing, and why: {note}"
        );
    }

    async fn subscribe_to_reports(
        handle: &BusHandle,
    ) -> (
        crate::bus::Subscriber<ErrorEvent>,
        crate::bus::Subscriber<MessageEvent>,
    ) {
        let errors = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let main = handle.subscribe(topics::UserMessage).await.unwrap();
        (errors, main)
    }

    #[tokio::test]
    async fn failing_replies_are_reported_once_until_one_gets_through() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
        let mut chat = FakeChat::new(&handle);
        chat.replies.insert("ok".to_string(), "group-7".to_string());
        chat.failing.push("owner-dm".to_string());
        start(&handle, chat.clone()).await;
        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();

        // A message addressed to an unknown conversation is reported every
        // time, so its note marks the point where the replies before it have
        // all been handled.
        for id in ["m1", "m2", "m3"] {
            publisher
                .publish(endpoint(), response(id, "hello", None))
                .await
                .unwrap();
        }
        publisher
            .publish(endpoint(), response("fence-1", "hi", Some("gone-1")))
            .await
            .unwrap();
        // Three failed replies in a row tell main once.
        let first_run = only_note_to_main_before(&mut main, "gone-1").await;
        assert_failing_replies_note(&first_run);
        expect_failing_replies_error(&mut errors).await;

        // A reply that gets through ends the run, so the next failure is
        // reported again.
        publisher
            .publish(endpoint(), response("ok", "back again", None))
            .await
            .unwrap();
        publisher
            .publish(endpoint(), response("m4", "hello", None))
            .await
            .unwrap();
        publisher
            .publish(endpoint(), response("fence-2", "hi", Some("gone-2")))
            .await
            .unwrap();
        let second_run = only_note_to_main_before(&mut main, "gone-2").await;
        assert_failing_replies_note(&second_run);
        expect_failing_replies_error(&mut errors).await;
        assert_eq!(
            chat.sent(),
            vec![("group-7".to_string(), "back again".to_string())]
        );
    }

    #[tokio::test]
    async fn a_failed_intermediate_message_is_reported() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
        let mut chat = FakeChat::new(&handle);
        chat.failing.push("owner-dm".to_string());
        start(&handle, chat.clone()).await;

        handle
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                IntermediateEvent {
                    correlation_id: "m1".to_string(),
                    content: "working on it".to_string(),
                },
            )
            .await
            .unwrap();

        expect_failing_replies_error(&mut errors).await;
        let note = wait::next_event("the failing-replies note to main", &mut main).await;
        assert_failing_replies_note(&note.content);
    }

    #[tokio::test]
    async fn a_failed_turn_error_is_reported() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
        let mut chat = FakeChat::new(&handle);
        chat.failing.push("owner-dm".to_string());
        start(&handle, chat.clone()).await;

        handle
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                ErrorEvent {
                    correlation_id: "m1".to_string(),
                    message: "The model is unavailable.".to_string(),
                    details: None,
                },
            )
            .await
            .unwrap();

        expect_failing_replies_error(&mut errors).await;
        let note = wait::next_event("the failing-replies note to main", &mut main).await;
        assert_failing_replies_note(&note.content);
    }
}

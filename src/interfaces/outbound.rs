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
//!
//! Text the agent writes before a tool call (intermediate text) reaches the
//! chat only when the platform's `deliver_intermediate` setting is on. Off,
//! the chat gets each turn's final reply, and the typing indicator, which
//! runs from the turn's start to its end, shows the turn is still working.
//! The web UI shows intermediate text either way.

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

    /// Whether intermediate (pre-tool-call) text is posted to the chat, from
    /// the platform's `deliver_intermediate` setting.
    fn delivers_intermediate(&self) -> bool;

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
///
/// Output with nowhere to go because nobody has messaged the bot yet is
/// reported the same way, once until something is delivered. Main's turn on
/// that report outputs to the same interface, so the once-only flag is what
/// keeps it from reporting itself.
#[derive(Default)]
struct DeliveryHealth {
    /// Replies that failed since the last send that got through.
    failures_in_a_row: u32,
    /// Output was dropped for want of an owner and that has been reported.
    no_owner_reported: bool,
}

impl DeliveryHealth {
    /// A send got through.
    fn delivered(&mut self, interface: &str) {
        self.no_owner_reported = false;
        if self.failures_in_a_row > 0 {
            tracing::info!(
                interface,
                failed_replies = self.failures_in_a_row,
                "replies are being delivered again"
            );
            self.failures_in_a_row = 0;
        }
    }

    /// Output had no conversation to go to. Reported the first time since
    /// the last delivered send.
    async fn no_owner<C: ChatOutbound>(&mut self, chat: &C) {
        if !self.no_owner_reported {
            self.no_owner_reported = true;
            report_no_owner(chat).await;
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

/// Send a reply to the conversation `correlation_id` answers and record the
/// outcome in `health`. `what` names the kind of reply in the log.
async fn send_reply<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    correlation_id: &str,
    content: &str,
    attachment: Option<&FileAttachment>,
    what: &'static str,
) {
    let Some(target) = reply_target(chat, health, correlation_id).await else {
        return;
    };
    match chat.send(&target, content, attachment).await {
        Ok(()) => health.delivered(chat.name()),
        Err(reason) => {
            tracing::warn!(
                interface = chat.name(),
                target = %chat.target_label(&target),
                error = %reason,
                reply = what,
                "failed to deliver a reply"
            );
            health.failed(chat, &reason).await;
        }
    }
}

async fn deliver_intermediate<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    event: IntermediateEvent,
) {
    // Ahead of `send_reply`, so an unsent intermediate never counts toward
    // delivery health or warns about a missing owner.
    if !chat.delivers_intermediate() {
        tracing::trace!(
            interface = chat.name(),
            correlation_id = %event.correlation_id,
            "intermediate text not posted; deliver_intermediate is off"
        );
        return;
    }
    send_reply(
        chat,
        health,
        &event.correlation_id,
        &event.content,
        None,
        "intermediate message",
    )
    .await;
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
        send_reply(
            chat,
            health,
            &response.correlation_id,
            &response.content,
            response.attachment.as_ref(),
            "reply",
        )
        .await;
        return;
    };
    let Some(target) = chat.conversation_target(id).await else {
        tracing::warn!(
            interface = chat.name(),
            conversation = %id,
            "message addressed to an unknown conversation"
        );
        report_undelivered_message(chat, id, UndeliveredCause::UnknownConversation).await;
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
            report_undelivered_message(chat, &place, UndeliveredCause::Refused(&reason)).await;
        }
    }
}

/// A conversation session's output never falls back to the owner's direct
/// messages. Either failure mode notifies main instead. Its intermediate
/// text follows the same `deliver_intermediate` rule as a main turn's, and
/// is skipped before any target lookup so it is never reported undelivered.
async fn deliver_session<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    response: SessionResponseEvent,
) {
    if !response.is_final && !chat.delivers_intermediate() {
        tracing::trace!(
            interface = chat.name(),
            session = %response.session_address,
            "session intermediate text not posted; deliver_intermediate is off"
        );
        return;
    }
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

/// Where a main-agent turn's output goes. `None` means nobody has messaged
/// the bot yet, which is logged every time and reported through `health`.
async fn reply_target<C: ChatOutbound>(
    chat: &C,
    health: &mut DeliveryHealth,
    correlation_id: &str,
) -> Option<C::Target> {
    let target = chat.reply_target(correlation_id).await;
    if target.is_none() {
        tracing::warn!(
            correlation_id,
            interface = chat.name(),
            "no conversation to deliver to; the owner has not messaged the bot yet"
        );
        health.no_owner(chat).await;
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
    let text = format!("**Error:** {}", event.message);
    send_reply(
        chat,
        health,
        &event.correlation_id,
        &text,
        None,
        "turn error",
    )
    .await;
}

/// The owner's next step for a send `chat` refused for `reason`: the
/// platform's fix when it knows one, otherwise where to look.
fn next_step_for<C: ChatOutbound>(chat: &C, reason: &str) -> String {
    chat.explain_failure(reason).unwrap_or_else(|| {
        let platform = chat.display_name();
        format!(
            "Check the {platform} connection in this agent's Settings, under Connections; \
             the details show what {platform} reported."
        )
    })
}

/// Publish a `[Delivery Failed]` note to main.
async fn tell_main<C: ChatOutbound>(chat: &C, note: String) {
    let event = MessageEvent::from_background(format!("[Delivery Failed] {note}"));
    if let Err(e) = chat.publisher().publish(topics::UserMessage, event).await {
        tracing::warn!(
            interface = chat.name(),
            error = %e,
            "failed to tell main about undelivered output"
        );
    }
}

/// Replies on this interface have started failing, so the chat sees the
/// agent ignore its messages. The owner gets a plain-language error in the
/// web UI, with the platform's own wording as its details, and main hears
/// about it so it knows the chat is not seeing its answers.
async fn report_failing_replies<C: ChatOutbound>(chat: &C, reason: &str) {
    let platform = chat.display_name();
    let next_step = next_step_for(chat, reason);
    crate::gateway::helpers::publish_error(
        chat.publisher(),
        format!(
            "Replies to {platform} aren't getting through, so messages there look unanswered. \
             The replies are still here in the web UI. {next_step}"
        ),
        Some(reason.to_string()),
    )
    .await;
    let fix_note = chat
        .explain_failure(reason)
        .map(|fix| format!(" {fix}"))
        .unwrap_or_default();
    tell_main(
        chat,
        format!(
            "Your replies on {platform} aren't reaching the chat: {reason}.{fix_note} \
             The user has been told in the web UI. Further failures on {platform} won't be \
             reported until a reply gets through again."
        ),
    )
    .await;
}

/// Main's output on this interface has nowhere to go: nobody has messaged
/// the bot, so there is no owner conversation to send to. The owner hears how
/// to set that up, and main hears that the output went nowhere.
async fn report_no_owner<C: ChatOutbound>(chat: &C) {
    let platform = chat.display_name();
    crate::gateway::helpers::publish_notice(
        chat.publisher(),
        format!(
            "The agent tried to send you something on {platform}, but you haven't messaged the \
             bot there yet, so it doesn't know where to reach you. Send the bot a direct message \
             on {platform} once. Until then, what it sends there only shows here in the web UI."
        ),
    )
    .await;
    tell_main(
        chat,
        format!(
            "Your output to {platform} went nowhere: the user hasn't messaged the bot there yet, \
             so it has no owner to send to. The user has been told in the web UI. This won't be \
             reported again until something is delivered on {platform}."
        ),
    )
    .await;
}

/// Why a message the agent addressed to a conversation did not go out.
enum UndeliveredCause<'a> {
    /// The conversation id does not resolve; the platform's plain-language
    /// reason says why.
    UnknownConversation,
    /// The platform refused the send, with its own error.
    Refused(&'a str),
}

/// A message the agent addressed to a specific conversation did not go out.
/// The owner sees it in the web UI and main hears about it, since nobody in
/// the chat will.
async fn report_undelivered_message<C: ChatOutbound>(
    chat: &C,
    place: &str,
    cause: UndeliveredCause<'_>,
) {
    let platform = chat.display_name();
    let (message, details, reason) = match cause {
        UndeliveredCause::UnknownConversation => {
            let reason = chat.unknown_conversation_reason();
            (
                format!(
                    "A message the agent sent to {place} on {platform} didn't go out: {reason}."
                ),
                None,
                reason,
            )
        }
        UndeliveredCause::Refused(reason) => (
            format!(
                "A message the agent sent to {place} on {platform} didn't go out. {}",
                next_step_for(chat, reason)
            ),
            Some(reason.to_string()),
            reason,
        ),
    };
    crate::gateway::helpers::publish_error(chat.publisher(), message, details).await;
    let fix_note = chat
        .explain_failure(reason)
        .map(|fix| format!(" {fix}"))
        .unwrap_or_default();
    tell_main(
        chat,
        format!(
            "Your message to {place} on {platform} didn't go out: {reason}.{fix_note} \
             The user has been told in the web UI."
        ),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::bus::{
        BusHandle, EndpointName, NoticeEvent, NotifyName, SYSTEM_CHANNEL, SessionAddress,
    };
    use crate::testing::{bus_wait, clock, wait};

    const ENDPOINT: &str = "fakechat";

    /// A chat platform that records what it sends. Targets are plain strings:
    /// `"owner-dm"` for the owner, otherwise a conversation id.
    #[derive(Clone)]
    struct FakeChat {
        publisher: crate::bus::Publisher,
        replies: HashMap<String, String>,
        conversations: Vec<String>,
        failing: Vec<String>,
        /// While set, nobody has messaged the bot: turns have no reply target.
        no_owner: Arc<AtomicBool>,
        deliver_intermediate: bool,
        sent: Arc<Mutex<Vec<(String, String)>>>,
        /// Every typing indicator started, by target. One is still running
        /// while the loop holds its sender.
        typing: Arc<Mutex<Vec<TypingIndicator>>>,
    }

    type TypingIndicator = (String, tokio::sync::watch::Receiver<()>);

    impl FakeChat {
        fn new(handle: &BusHandle) -> Self {
            Self {
                publisher: handle.publisher(),
                replies: HashMap::new(),
                conversations: Vec::new(),
                failing: Vec::new(),
                no_owner: Arc::default(),
                deliver_intermediate: false,
                sent: Arc::default(),
                typing: Arc::default(),
            }
        }

        fn sent(&self) -> Vec<(String, String)> {
            self.sent.lock().unwrap().clone()
        }

        fn sent_sorted(&self) -> Vec<(String, String)> {
            let mut sent = self.sent();
            sent.sort();
            sent
        }

        fn typing_in(&self, target: &str) -> bool {
            self.typing
                .lock()
                .unwrap()
                .iter()
                .any(|(t, rx)| t == target && rx.has_changed().is_ok())
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

        fn delivers_intermediate(&self) -> bool {
            self.deliver_intermediate
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
            if self.no_owner.load(Ordering::SeqCst) {
                return None;
            }
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

        fn start_typing(&self, target: String) -> tokio::sync::watch::Sender<()> {
            let (tx, rx) = tokio::sync::watch::channel(());
            self.typing.lock().unwrap().push((target, rx));
            tx
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

    fn session_output(
        conversation_id: &str,
        content: &str,
        is_final: bool,
    ) -> SessionResponseEvent {
        SessionResponseEvent {
            session_address: SessionAddress::from("external-group-7"),
            conversation_id: conversation_id.to_string(),
            content: content.to_string(),
            attachment: None,
            timestamp: chrono::Utc::now().naive_utc(),
            is_final,
        }
    }

    /// One turn with a tool call in the middle, as a chat sees it: the
    /// turn starts, the main agent and a group's session each write text
    /// before a tool call, then each sends its final reply. The turn is left
    /// open so the test can look at its typing indicator.
    async fn publish_turn_with_tool_call(handle: &BusHandle) {
        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();
        publisher
            .publish(
                endpoint(),
                TurnLifecycleEvent::Started {
                    correlation_id: "m1".to_string(),
                },
            )
            .await
            .unwrap();
        publisher
            .publish(
                endpoint(),
                IntermediateEvent {
                    correlation_id: "m1".to_string(),
                    content: "checking the build first".to_string(),
                },
            )
            .await
            .unwrap();
        publisher
            .publish(
                endpoint(),
                session_output("group-7", "looking that up", false),
            )
            .await
            .unwrap();
        publisher
            .publish(endpoint(), response("m1", "the build is green", None))
            .await
            .unwrap();
        publisher
            .publish(endpoint(), session_output("group-7", "it's Tuesday", true))
            .await
            .unwrap();
    }

    async fn end_turn(handle: &BusHandle) {
        handle
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                TurnLifecycleEvent::Ended {
                    correlation_id: "m1".to_string(),
                },
            )
            .await
            .unwrap();
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
            .publish(system(), NoticeEvent::new("Config reloaded"))
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

    // Paused clock: every event is in process, so `clock::elapse` returns
    // only once the loop has nothing left to handle.
    #[tokio::test(start_paused = true)]
    async fn by_default_the_chat_gets_final_replies_and_typing_through_the_turn() {
        let handle = crate::bus::spawn_broker();
        let mut chat = FakeChat::new(&handle);
        chat.conversations.push("group-7".to_string());
        start(&handle, chat.clone()).await;

        publish_turn_with_tool_call(&handle).await;
        wait_for_sends(&chat, 2).await;
        clock::elapse(Duration::from_millis(50)).await;
        assert_eq!(
            chat.sent_sorted(),
            vec![
                ("group-7".to_string(), "it's Tuesday".to_string()),
                ("owner-dm".to_string(), "the build is green".to_string()),
            ],
            "text written before a tool call stays out of the chat; final replies go out"
        );
        assert!(
            chat.typing_in("owner-dm"),
            "the typing indicator runs until the turn ends, not until text stops arriving"
        );

        end_turn(&handle).await;
        wait::until_true("the typing indicator to stop at turn end", || {
            !chat.typing_in("owner-dm")
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn deliver_intermediate_posts_text_written_before_a_tool_call() {
        let handle = crate::bus::spawn_broker();
        let mut chat = FakeChat::new(&handle);
        chat.conversations.push("group-7".to_string());
        chat.deliver_intermediate = true;
        start(&handle, chat.clone()).await;

        publish_turn_with_tool_call(&handle).await;
        wait_for_sends(&chat, 4).await;
        clock::elapse(Duration::from_millis(50)).await;
        assert_eq!(
            chat.sent_sorted(),
            vec![
                ("group-7".to_string(), "it's Tuesday".to_string()),
                ("group-7".to_string(), "looking that up".to_string()),
                (
                    "owner-dm".to_string(),
                    "checking the build first".to_string()
                ),
                ("owner-dm".to_string(), "the build is green".to_string()),
            ],
            "with the setting on, each piece of intermediate text is its own message"
        );
    }

    // Paused clock, as above. A failing chat proves an unsent intermediate
    // is not a failure: nothing is reported until the final reply fails.
    #[tokio::test(start_paused = true)]
    async fn an_unsent_intermediate_is_never_reported_as_a_failure() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
        let mut chat = FakeChat::new(&handle);
        chat.failing.push("owner-dm".to_string());
        chat.failing.push("group-7".to_string());
        chat.conversations.push("group-7".to_string());
        start(&handle, chat.clone()).await;
        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();

        publisher
            .publish(
                endpoint(),
                IntermediateEvent {
                    correlation_id: "m1".to_string(),
                    content: "working on it".to_string(),
                },
            )
            .await
            .unwrap();
        publisher
            .publish(
                endpoint(),
                session_output("group-7", "looking that up", false),
            )
            .await
            .unwrap();
        assert!(
            clock::within(Duration::from_millis(50), errors.recv())
                .await
                .is_none(),
            "an intermediate that was never sent must not be reported"
        );
        assert!(
            clock::within(Duration::from_millis(50), main.recv())
                .await
                .is_none(),
            "main must not hear about an intermediate that was never sent"
        );

        publisher
            .publish(endpoint(), response("m1", "done", None))
            .await
            .unwrap();
        expect_failing_replies_error(&mut errors).await;
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
    async fn undeliverable_addressed_message_becomes_a_web_error_not_a_chat_message() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
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

        // Refused by the platform: its fix in the message, its own words in
        // the details only.
        let refused = bus_wait::next_event("the refused-send error", &mut errors).await;
        assert!(
            refused.message.contains("group-7")
                && refused.message.contains("FakeChat")
                && refused.message.contains("Reconnect FakeChat.")
                && !refused.message.contains("refused"),
            "the error should name the place, the platform and the fix, without the raw reason: {}",
            refused.message
        );
        assert_eq!(refused.details.as_deref(), Some("the platform refused it"));
        let refused_note = bus_wait::next_event("the refused-send note to main", &mut main).await;
        assert!(
            refused_note.content.starts_with("[Delivery Failed]")
                && refused_note.content.contains("group-7")
                && refused_note.content.contains("the platform refused it"),
            "main should hear where and why: {}",
            refused_note.content
        );

        // An unknown conversation: the platform's reason is already plain.
        let unknown = bus_wait::next_event("the unknown-conversation error", &mut errors).await;
        assert!(
            unknown.message.contains("gone-1") && unknown.message.contains("no such conversation"),
            "the error should name the place and why: {}",
            unknown.message
        );
        assert_eq!(unknown.details, None);
        let unknown_note =
            bus_wait::next_event("the unknown-conversation note to main", &mut main).await;
        assert!(
            unknown_note.content.starts_with("[Delivery Failed]")
                && unknown_note.content.contains("gone-1")
        );

        assert!(
            chat.sent().is_empty(),
            "a delivery failure must not be posted to the owner's chat"
        );
    }

    /// The one event `sub` received before the first whose text names
    /// `marker`. A message addressed to an unknown conversation is reported
    /// every time, to the web UI and to main, so the tests use one as a marker
    /// that every event published before it has been handled.
    async fn only_event_before<E>(
        sub: &mut crate::bus::Subscriber<E>,
        marker: &str,
        text: impl Fn(&E) -> &str,
    ) -> E
    where
        E: Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let mut before = Vec::new();
        loop {
            let event = bus_wait::next_event(format!("an event naming {marker}"), sub).await;
            if text(&event).contains(marker) {
                break;
            }
            before.push(event);
        }
        let [event] = <[E; 1]>::try_from(before).unwrap_or_else(|before| {
            panic!("expected exactly one event before {marker}, got {before:?}")
        });
        event
    }

    /// Exactly one failing-replies error and one note to main came before
    /// the reports naming `marker`.
    async fn expect_one_report_before(
        errors: &mut crate::bus::Subscriber<ErrorEvent>,
        main: &mut crate::bus::Subscriber<MessageEvent>,
        marker: &str,
    ) {
        let note = only_event_before(main, marker, note_text).await;
        assert_failing_replies_note(&note.content);
        let error = only_event_before(errors, marker, error_text).await;
        assert_failing_replies_error(&error);
    }

    fn note_text(note: &MessageEvent) -> &str {
        &note.content
    }

    fn error_text(error: &ErrorEvent) -> &str {
        &error.message
    }

    /// Waits for the owner-facing error that says `FakeChat` replies are failing.
    async fn expect_failing_replies_error(errors: &mut crate::bus::Subscriber<ErrorEvent>) {
        let error = bus_wait::next_event("the failing-replies error", errors).await;
        assert_failing_replies_error(&error);
    }

    fn assert_failing_replies_error(error: &ErrorEvent) {
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
        chat.conversations.push("group-9".to_string());
        chat.failing.push("owner-dm".to_string());
        start(&handle, chat.clone()).await;
        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();
        let send = |id: &'static str, conversation: Option<&'static str>| {
            let publisher = publisher.clone();
            async move {
                publisher
                    .publish(endpoint(), response(id, "hello", conversation))
                    .await
                    .unwrap();
            }
        };
        // Three failed replies in a row are reported once.
        for id in ["m1", "m2", "m3"] {
            send(id, None).await;
        }
        send("fence-1", Some("gone-1")).await;
        expect_one_report_before(&mut errors, &mut main, "gone-1").await;

        // A reply that gets through ends the run, so the next failure is
        // reported again.
        send("ok", None).await;
        send("m4", None).await;
        send("fence-2", Some("gone-2")).await;
        expect_one_report_before(&mut errors, &mut main, "gone-2").await;

        // So does a message the agent addressed to a conversation.
        send("to-group-9", Some("group-9")).await;
        send("m5", None).await;
        send("fence-3", Some("gone-3")).await;
        expect_one_report_before(&mut errors, &mut main, "gone-3").await;

        assert_eq!(
            chat.sent(),
            vec![
                ("group-7".to_string(), "hello".to_string()),
                ("group-9".to_string(), "hello".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn output_with_no_owner_is_reported_once_until_something_is_delivered() {
        let handle = crate::bus::spawn_broker();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut main: crate::bus::Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let chat = FakeChat::new(&handle);
        chat.no_owner.store(true, Ordering::SeqCst);
        start(&handle, chat.clone()).await;
        let endpoint = || topics::Endpoint(EndpointName::from(ENDPOINT));
        let publisher = handle.publisher();
        let expect_no_owner_report = |note: &MessageEvent, notice: &NoticeEvent| {
            assert!(
                note.content.starts_with("[Delivery Failed]")
                    && note.content.contains("FakeChat")
                    && note.content.contains("hasn't messaged the bot"),
                "main should hear its output went nowhere: {}",
                note.content
            );
            assert!(
                notice.message.contains("FakeChat") && notice.message.contains("direct message"),
                "the owner should hear how to fix it: {}",
                notice.message
            );
        };

        for id in ["m1", "m2"] {
            publisher
                .publish(endpoint(), response(id, "hello", None))
                .await
                .unwrap();
        }
        publisher
            .publish(endpoint(), response("fence-1", "hi", Some("gone-1")))
            .await
            .unwrap();
        let first_note = only_event_before(&mut main, "gone-1", note_text).await;
        let first_notice = bus_wait::next_event("the no-owner notice", &mut notices).await;
        expect_no_owner_report(&first_note, &first_notice);

        // The owner messages the bot and a reply gets through; when the
        // owner is gone again, that is reported again.
        chat.no_owner.store(false, Ordering::SeqCst);
        publisher
            .publish(endpoint(), response("m3", "delivered", None))
            .await
            .unwrap();
        wait_for_sends(&chat, 1).await;
        chat.no_owner.store(true, Ordering::SeqCst);
        publisher
            .publish(endpoint(), response("m4", "hello", None))
            .await
            .unwrap();
        publisher
            .publish(endpoint(), response("fence-2", "hi", Some("gone-2")))
            .await
            .unwrap();
        let second_note = only_event_before(&mut main, "gone-2", note_text).await;
        let second_notice = bus_wait::next_event("the second no-owner notice", &mut notices).await;
        expect_no_owner_report(&second_note, &second_notice);
    }

    #[tokio::test]
    async fn a_failed_intermediate_message_is_reported() {
        let handle = crate::bus::spawn_broker();
        let (mut errors, mut main) = subscribe_to_reports(&handle).await;
        let mut chat = FakeChat::new(&handle);
        chat.failing.push("owner-dm".to_string());
        chat.deliver_intermediate = true;
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
        let note = bus_wait::next_event("the failing-replies note to main", &mut main).await;
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
        let note = bus_wait::next_event("the failing-replies note to main", &mut main).await;
        assert_failing_replies_note(&note.content);
    }
}

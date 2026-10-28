//! Slack bus subscriber — translates typed bus events to Slack messages.
//!
//! The mirror of `telegram/subscriber.rs` over the shared
//! `interfaces/outbound.rs` loop: same subscription set (conversation only —
//! agent replies and failed-turn report-backs reach the chat; system notices
//! and errors never do, enforced by what [`BaseSubscribers`] subscribes to),
//! same reply routing through the state's correlation map, same chunking
//! through the shared fence-aware chunker.
//!
//! Three Slack-specific policies:
//!
//! * chunks are converted to mrkdwn ([`super::format`]) one at a time, just
//!   before each send — the chunker splits the markdown the agent produced,
//!   and its fence closing/reopening keeps every chunk independently
//!   convertible;
//! * sends are awaited inline in the delivery loop, never queued: nothing
//!   buffers ahead of [`RateLimitedSender`], whose mutex spans its backoff
//!   sleeps — a slow send backpressures the loop instead of piling up an
//!   unbounded queue behind it;
//! * "working" feedback is a reaction: when a slack-originated turn starts,
//!   the agent's `working_reaction` emoji goes on the owner's message, and
//!   it comes off when the turn ends — the gateway publishes `Ended` after
//!   a successful turn and after a failed one alike. Slack's design guide
//!   names an emoji reaction as the lightweight "seen, working" signal; the
//!   native `agents.sessions.setStatus` indicator is thread-scoped and v1
//!   replies flat. A reaction posts no message, so — unlike a posted
//!   "Thinking…" — nothing is left looking like a reply if a turn goes
//!   wrong. Residual: a hub crash mid-turn never publishes `Ended`, so that
//!   one reaction stays until removed by hand.

use std::sync::Arc;

use async_trait::async_trait;

use crate::chunking::chunk_text;
use crate::outbound::ChatOutbound;
use residuum_bus::attachment::FileAttachment;

use super::api::{RateLimitedSender, SlackApi};
use super::format::markdown_to_mrkdwn;
use super::{SlackReply, SlackState, SlackTarget};

/// Maximum message length for Slack, beside Telegram's 4,096 and Discord's
/// 2,000 in their subscribers. Slack documents ~4,000 characters as the
/// `chat.postMessage` text budget this MVP holds itself to. `pub(super)`:
/// the module's end-to-end test sizes its chunk expectations from it.
pub(super) const SLACK_MAX_CHARS: usize = 4000;

/// The outbound side: the rate-limited sender plus the state shared with
/// the inbound handler, the working reaction, and the reactions in place.
struct SlackOutbound<A: SlackApi> {
    sender: Arc<RateLimitedSender<A>>,
    state: Arc<SlackState>,
    /// Emoji shortcode put on the owner's message while its turn runs;
    /// `None` when the agent's config turns the reaction off.
    working_reaction: Option<String>,
    /// Reactions currently on a message, by the correlation id of the turn
    /// they mark — so the turn's end removes exactly what its start added,
    /// even after the reply mapping is gone. A std mutex, like the
    /// reply-target map: critical sections never await.
    reacted: std::sync::Mutex<std::collections::HashMap<String, Reacted>>,
}

/// A working reaction in place on one message.
struct Reacted {
    channel: String,
    message_ts: String,
}

/// Receives events from the bus and delivers them to Slack channels.
///
/// The interface constructor builds the [`BaseSubscribers`] for the `slack`
/// endpoint (as Telegram's and Discord's constructors do) and passes the
/// sender it configured.
pub(super) async fn run_slack_subscriber<A: SlackApi>(
    subs: crate::BaseSubscribers,
    sender: Arc<RateLimitedSender<A>>,
    state: Arc<SlackState>,
    working_reaction: Option<String>,
) {
    crate::outbound::run(
        subs,
        SlackOutbound {
            sender,
            state,
            working_reaction,
            reacted: std::sync::Mutex::new(std::collections::HashMap::new()),
        },
    )
    .await;
}

#[async_trait]
impl<A: SlackApi> ChatOutbound for SlackOutbound<A> {
    /// A Slack channel id (`"C…"`, `"D…"`) plus, for a reply inside a
    /// thread, the thread's root `ts`.
    type Target = SlackTarget;

    fn name(&self) -> &'static str {
        "slack"
    }

    fn display_name(&self) -> &'static str {
        "Slack"
    }

    fn delivers_intermediate(&self) -> bool {
        self.state.deliver_intermediate
    }

    fn unknown_conversation_reason(&self) -> &'static str {
        "that is not a Slack channel ID"
    }

    fn publisher(&self) -> &residuum_bus::Publisher {
        &self.state.publisher
    }

    async fn reply_target(&self, correlation_id: &str) -> Option<SlackTarget> {
        self.state.target_for(correlation_id).await
    }

    fn release_reply(&self, correlation_id: &str) {
        self.state.reply_targets.release(correlation_id);
    }

    async fn conversation_target(&self, conversation_id: &str) -> Option<SlackTarget> {
        parse_channel_id(conversation_id).map(SlackTarget::top_level)
    }

    async fn describe(&self, conversation_id: &str, _target: &SlackTarget) -> String {
        self.state
            .store
            .conversation(conversation_id)
            .await
            .map_or_else(|| format!("channel {conversation_id}"), |chat| chat.label)
    }

    fn target_label(&self, target: &SlackTarget) -> String {
        match &target.thread_ts {
            Some(thread_ts) => format!("{} (thread {thread_ts})", target.channel),
            None => target.channel.clone(),
        }
    }

    /// Slack's Web API has no typing indicator a bot can send to a DM, so
    /// the working signal is the reaction ([`Self::turn_started`]) and this
    /// remains a stop channel with no loop behind it — dropping it is the
    /// whole contract.
    fn start_typing(&self, _target: SlackTarget) -> tokio::sync::watch::Sender<()> {
        tokio::sync::watch::channel(()).0
    }

    /// A turn started: when the correlation is one the Slack handler
    /// tracked — a slack-originated turn; the strict reply map, NOT
    /// [`SlackState::target_for`]'s owner-DM fallback, which would light up
    /// foreign turns (telegram, cron, …) — put the working reaction on the
    /// message that started it. A failed reaction logs and moves on: the
    /// reply itself is unaffected, and nothing is recorded to remove.
    async fn turn_started(&self, correlation_id: &str) {
        let Some(name) = self.working_reaction.as_deref() else {
            return;
        };
        let Some(SlackReply { target, message_ts }) = self.state.reply_targets.get(correlation_id)
        else {
            return;
        };
        match self.sender.react(&target.channel, &message_ts, name).await {
            Ok(()) => {
                self.reacted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        correlation_id.to_string(),
                        Reacted {
                            channel: target.channel,
                            message_ts,
                        },
                    );
            }
            Err(e) => tracing::warn!(
                correlation_id,
                reaction = name,
                error = %e,
                "could not add the slack working reaction; the reply is unaffected"
            ),
        }
    }

    /// A turn ended — successfully or not, the gateway publishes `Ended`
    /// either way — so take its working reaction off. The run loop drains a
    /// turn's output before its `Ended` (the biased `select!` in
    /// `outbound.rs`), so the reply is already posted by the time the
    /// reaction comes off. A failed removal logs: there is nothing else to
    /// fall back to.
    async fn turn_ended(&self, correlation_id: &str) {
        let Some(name) = self.working_reaction.as_deref() else {
            return;
        };
        let reacted = self
            .reacted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(correlation_id);
        let Some(reacted) = reacted else {
            return; // never reacted (not a slack turn, or the add failed)
        };
        if let Err(e) = self
            .sender
            .unreact(&reacted.channel, &reacted.message_ts, name)
            .await
        {
            tracing::warn!(
                correlation_id,
                reaction = name,
                error = %e,
                "could not remove the slack working reaction; it stays on the message"
            );
        }
    }

    async fn send(
        &self,
        target: &SlackTarget,
        content: &str,
        attachment: Option<&FileAttachment>,
    ) -> Result<(), String> {
        // Slack v1 cannot upload files; the text goes out with a note
        // naming where the file lives, the way Teams' subscriber handles it.
        let text = match attachment {
            Some(attachment) => attachment_fallback(content, attachment),
            None if content.is_empty() => return Ok(()),
            None => content.to_string(),
        };
        send_chunks(&self.sender, target, &text)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Parse a conversation id into a Slack channel id, or `None` if it isn't
/// one — Telegram and Discord parse a numeric chat/channel id here; Slack
/// ids are strings with a leading letter (`"C…"`, `"D…"`, `"G…"`) followed
/// by alphanumerics, so that shape is the check.
fn parse_channel_id(conversation_id: &str) -> Option<String> {
    let mut chars = conversation_id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => (),
        _ => return None,
    }
    if conversation_id.len() < 2 || !conversation_id.chars().all(char::is_alphanumeric) {
        return None;
    }
    Some(conversation_id.to_string())
}

/// Send `content` in mrkdwn-converted chunks to `target`, stopping at the
/// first failure — Telegram's `send_chunks` policy. Every chunk goes to the
/// same place: the thread the owner wrote in, or the top level.
async fn send_chunks<A: SlackApi>(
    sender: &RateLimitedSender<A>,
    target: &SlackTarget,
    content: &str,
) -> anyhow::Result<()> {
    for chunk in chunk_text(content, SLACK_MAX_CHARS) {
        let mrkdwn = markdown_to_mrkdwn(&chunk);
        sender
            .send(&target.channel, target.thread_ts.as_deref(), &mrkdwn)
            .await?;
    }
    Ok(())
}

/// Slack v1 has no file upload. The text the owner sees names the file and
/// where it was saved — the same fallback wording Teams uses.
fn attachment_fallback(content: &str, attachment: &FileAttachment) -> String {
    tracing::warn!(
        file = %attachment.path.display(),
        "slack v1 cannot deliver file attachments; sending the text with a note"
    );
    format!(
        "{content}\n\n_I made a file for you ({}), but I can't send files over Slack yet. \
         It's saved at `{}` and available in the web UI._",
        attachment.filename,
        attachment.path.display()
    )
    .trim_start()
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use crate::BaseSubscribers;
    use crate::chat_state::ChatStateStore;
    use crate::reply_targets::ReplyTargets;
    use residuum_bus::{
        BusHandle, EndpointName, ErrorEvent, IntermediateEvent, NoticeEvent, NotifyName,
        ResponseEvent, SYSTEM_CHANNEL, TurnLifecycleEvent, spawn_broker, topics,
    };
    use residuum_core::testing::{clock, wait};

    use super::super::ENDPOINT;
    use super::super::api::SlackApiError;

    /// One recorded Web API call. `method` is `post`, `react` or `unreact`;
    /// `detail` is a post's text or a reaction's `ts:name`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Call {
        method: &'static str,
        channel: String,
        thread_ts: Option<String>,
        detail: String,
    }

    impl Call {
        fn post(channel: &str, thread_ts: Option<&str>, text: &str) -> Self {
            Self {
                method: "post",
                channel: channel.to_string(),
                thread_ts: thread_ts.map(str::to_string),
                detail: text.to_string(),
            }
        }

        fn react(method: &'static str, channel: &str, ts: &str, name: &str) -> Self {
            Self {
                method,
                channel: channel.to_string(),
                thread_ts: None,
                detail: format!("{ts}:{name}"),
            }
        }
    }

    /// Records every call in order; reaction results are scriptable, for
    /// the add-failure path. The log is a std mutex so tests can poll it
    /// from `wait::until_true`'s sync closure, like `outbound.rs`'s
    /// `FakeChat`.
    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Arc<std::sync::Mutex<Vec<Call>>>,
        react_script: Arc<std::sync::Mutex<std::collections::VecDeque<Result<(), SlackApiError>>>>,
    }

    impl FakeApi {
        fn record(&self, call: Call) {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(call);
        }
    }

    #[async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(
            &self,
            channel: &str,
            thread_ts: Option<&str>,
            text: &str,
        ) -> Result<(), SlackApiError> {
            self.record(Call::post(channel, thread_ts, text));
            Ok(())
        }

        async fn add_reaction(
            &self,
            channel: &str,
            ts: &str,
            name: &str,
        ) -> Result<(), SlackApiError> {
            self.record(Call::react("react", channel, ts, name));
            self.react_script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Ok(()))
        }

        async fn remove_reaction(
            &self,
            channel: &str,
            ts: &str,
            name: &str,
        ) -> Result<(), SlackApiError> {
            self.record(Call::react("unreact", channel, ts, name));
            Ok(())
        }

        async fn open_connection(&self) -> anyhow::Result<String> {
            Ok("wss://fake.slack".to_string())
        }
    }

    /// A running subscriber on a real bus, with the state the handler would
    /// have shared. Correlations are tracked directly rather than by
    /// spawning the inbound handler — handler tests already prove dispatch
    /// records them.
    struct Harness {
        api: FakeApi,
        state: Arc<SlackState>,
        bus: BusHandle,
        _dir: tempfile::TempDir,
    }

    async fn harness_with(working_reaction: Option<&str>) -> Harness {
        harness_configured(working_reaction, false).await
    }

    async fn harness_configured(
        working_reaction: Option<&str>,
        deliver_intermediate: bool,
    ) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let (store, notice) = ChatStateStore::load(dir.path().join("slack_state.json"))
            .await
            .unwrap();
        assert!(notice.is_none(), "a fresh state file has nothing to report");
        let bus = spawn_broker();
        let api = FakeApi::default();
        let state = Arc::new(SlackState {
            store,
            reply_targets: ReplyTargets::default(),
            publisher: bus.publisher(),
            deliver_intermediate,
        });
        let subs = BaseSubscribers::new(&bus, EndpointName::from(ENDPOINT))
            .await
            .unwrap();
        // Zero minimum interval: delivery tests exercise routing and
        // conversion, not spacing — the sender's own tests own the clock.
        let sender = Arc::new(RateLimitedSender {
            api: api.clone(),
            min_interval: Duration::ZERO,
            last: tokio::sync::Mutex::new(None),
        });
        tokio::spawn(run_slack_subscriber(
            subs,
            sender,
            Arc::clone(&state),
            working_reaction.map(str::to_string),
        ));
        Harness {
            api,
            state,
            bus,
            _dir: dir,
        }
    }

    async fn harness() -> Harness {
        harness_with(Some("zap")).await
    }

    impl Harness {
        /// Track a top-level DM message `ts` in `channel` under
        /// `correlation_id`, as the handler would at dispatch.
        fn track(&self, correlation_id: &str, channel: &str, ts: &str) {
            self.track_reply(correlation_id, channel, None, ts);
        }

        fn track_reply(&self, correlation_id: &str, channel: &str, thread: Option<&str>, ts: &str) {
            self.state.reply_targets.track(
                correlation_id,
                SlackReply {
                    target: SlackTarget {
                        channel: channel.to_string(),
                        thread_ts: thread.map(str::to_string),
                    },
                    message_ts: ts.to_string(),
                },
            );
        }

        fn calls(&self) -> Vec<Call> {
            self.api
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn posts(&self) -> Vec<Call> {
            self.calls()
                .into_iter()
                .filter(|call| call.method == "post")
                .collect()
        }

        async fn wait_for_calls(&self, count: usize) {
            wait::until_true(format!("{count} slack call(s)"), || {
                self.calls().len() >= count
            })
            .await;
        }

        fn fail_next_reaction(&self, error: SlackApiError) {
            self.api
                .react_script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(Err(error));
        }

        async fn publish_turn_started(&self, correlation_id: &str) {
            self.bus
                .publisher()
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    TurnLifecycleEvent::Started {
                        correlation_id: correlation_id.to_string(),
                    },
                )
                .await
                .unwrap();
        }

        async fn publish_turn_ended(&self, correlation_id: &str) {
            self.bus
                .publisher()
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    TurnLifecycleEvent::Ended {
                        correlation_id: correlation_id.to_string(),
                    },
                )
                .await
                .unwrap();
        }

        async fn publish_response(&self, correlation_id: &str, content: &str) {
            self.bus
                .publisher()
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    ResponseEvent {
                        correlation_id: correlation_id.to_string(),
                        content: content.to_string(),
                        timestamp: chrono::Utc::now().naive_utc(),
                        attachment: None,
                        conversation: None,
                    },
                )
                .await
                .unwrap();
        }

        async fn publish_turn_error(&self, correlation_id: &str, message: &str) {
            self.bus
                .publisher()
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    ErrorEvent {
                        correlation_id: correlation_id.to_string(),
                        message: message.to_string(),
                        details: None,
                    },
                )
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn intermediate_text_is_posted_when_deliver_intermediate_is_on() {
        let h = harness_configured(None, true).await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.bus
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from(ENDPOINT)),
                IntermediateEvent {
                    correlation_id: "slack-D0001-1234.0001".to_string(),
                    content: "checking the calendar first".to_string(),
                },
            )
            .await
            .unwrap();

        h.wait_for_calls(1).await;
        assert_eq!(
            h.posts(),
            vec![Call::post("D0001", None, "checking the calendar first")]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn system_notices_and_errors_never_reach_slack() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");
        let publisher = h.bus.publisher();
        let system = || topics::Notification(NotifyName::from(SYSTEM_CHANNEL));

        publisher
            .publish(
                system(),
                NoticeEvent {
                    message: "Config reloaded".to_string(),
                    reload: None,
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
        h.publish_response("slack-D0001-1234.0001", "hello").await;

        h.wait_for_calls(1).await;
        clock::elapse(Duration::from_millis(50)).await;
        assert_eq!(
            h.calls(),
            vec![Call::post("D0001", None, "hello")],
            "only the agent's reply may go out; system output stays in the web UI"
        );
    }

    #[tokio::test]
    async fn agent_reply_reaches_the_reply_channel_converted_to_mrkdwn() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_response("slack-D0001-1234.0001", "**bold** and *italic* with `code`")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::post("D0001", None, "*bold* and _italic_ with `code`")],
            "the reply goes flat to the channel that started the turn, in mrkdwn"
        );
    }

    #[tokio::test]
    async fn long_replies_are_chunked_at_the_slack_limit() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");
        let content = "x".repeat(SLACK_MAX_CHARS * 2 + 500);

        h.publish_response("slack-D0001-1234.0001", &content).await;

        let expected_chunks = chunk_text(&content, SLACK_MAX_CHARS).len();
        h.wait_for_calls(expected_chunks).await;
        let posts = h.posts();
        assert_eq!(posts.len(), expected_chunks);
        assert!(
            posts
                .iter()
                .all(|call| call.detail.len() <= SLACK_MAX_CHARS)
        );
        assert_eq!(
            posts
                .iter()
                .map(|call| call.detail.as_str())
                .collect::<String>(),
            content,
            "chunks recombine to the reply exactly"
        );
        assert!(
            posts
                .iter()
                .all(|call| call.channel == "D0001" && call.thread_ts.is_none()),
            "every chunk goes flat to the reply channel"
        );
    }

    #[tokio::test]
    async fn a_reply_to_a_message_written_in_a_thread_goes_into_that_thread() {
        let h = harness().await;
        h.track_reply(
            "slack-D0001-1234.0002",
            "D0001",
            Some("1234.0001"),
            "1234.0002",
        );
        let content = "y".repeat(SLACK_MAX_CHARS + 10); // two chunks

        h.publish_response("slack-D0001-1234.0002", &content).await;

        h.wait_for_calls(2).await;
        assert!(
            h.posts()
                .iter()
                .all(|call| call.channel == "D0001"
                    && call.thread_ts.as_deref() == Some("1234.0001")),
            "every chunk lands in the thread the owner wrote in: {:?}",
            h.posts()
        );
    }

    #[tokio::test]
    async fn failed_turn_report_back_goes_to_the_channel_that_started_the_turn() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_turn_error("slack-D0001-1234.0001", "The model is unavailable.")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::post(
                "D0001",
                None,
                "*Error:* The model is unavailable."
            )],
            "the failure goes where the reply would have, converted like any other text"
        );
    }

    /// The whole life of the working reaction for a turn that replies: on
    /// the owner's message when the turn starts, reply posted, reaction
    /// off — in that order. The gateway publishes Response then Ended back
    /// to back, on separate subscriber channels; the run loop's biased
    /// `select!` (lifecycle last) is what keeps the reply ahead of the
    /// removal, so the check repeats over many turns.
    #[tokio::test]
    async fn the_working_reaction_spans_the_turn_and_comes_off_after_the_reply() {
        let h = harness().await;
        for i in 0..20 {
            let correlation_id = format!("slack-D0001-{i}.0001");
            let ts = format!("{i}.0001");
            h.track(&correlation_id, "D0001", &ts);
            let before = h.calls().len();

            h.publish_turn_started(&correlation_id).await;
            h.wait_for_calls(before + 1).await;
            h.publish_response(&correlation_id, "the answer").await;
            h.publish_turn_ended(&correlation_id).await;
            h.wait_for_calls(before + 3).await;

            assert_eq!(
                h.calls().split_off(before),
                [
                    Call::react("react", "D0001", &ts, "zap"),
                    Call::post("D0001", None, "the answer"),
                    Call::react("unreact", "D0001", &ts, "zap"),
                ],
                "turn {i}: reaction on, reply, reaction off"
            );
        }
    }

    #[tokio::test]
    async fn a_failed_turn_still_clears_its_reaction() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await;
        h.publish_turn_error("slack-D0001-1234.0001", "The model is unavailable.")
            .await;
        h.publish_turn_ended("slack-D0001-1234.0001").await;

        h.wait_for_calls(3).await;
        assert_eq!(
            h.calls(),
            vec![
                Call::react("react", "D0001", "1234.0001", "zap"),
                Call::post("D0001", None, "*Error:* The model is unavailable."),
                Call::react("unreact", "D0001", "1234.0001", "zap"),
            ],
            "a failure is reported and the reaction still comes off"
        );
    }

    #[tokio::test]
    async fn a_turn_with_no_reply_still_clears_its_reaction() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.publish_response("slack-D0001-1234.0001", "").await;
        h.publish_turn_ended("slack-D0001-1234.0001").await;

        h.wait_for_calls(2).await;
        assert_eq!(
            h.calls(),
            vec![
                Call::react("react", "D0001", "1234.0001", "zap"),
                Call::react("unreact", "D0001", "1234.0001", "zap"),
            ],
            "nothing posted, and no reaction is left behind"
        );
    }

    #[tokio::test]
    async fn the_reaction_marks_the_message_not_the_thread() {
        let h = harness().await;
        h.track_reply(
            "slack-D0001-1234.0002",
            "D0001",
            Some("1234.0001"),
            "1234.0002",
        );

        h.publish_turn_started("slack-D0001-1234.0002").await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::react("react", "D0001", "1234.0002", "zap")],
            "the reaction goes on the message the owner just wrote, not the thread root"
        );
    }

    #[tokio::test]
    async fn a_foreign_turn_never_reacts() {
        // An owner is bound, so the owner-DM fallback door exists — a
        // foreign turn (telegram, cron, …) must not light it up. The slack
        // turn started afterwards proves the loop had its chance to
        // mis-react for the foreign one.
        let h = harness().await;
        h.state
            .store
            .claim_owner(crate::chat_state::Owner {
                user_id: "U1".to_string(),
                name: "U1".to_string(),
                dm_conversation_id: "D0OWNER1".to_string(),
            })
            .await
            .unwrap();

        h.publish_turn_started("telegram-123-456").await;
        h.publish_turn_ended("telegram-123-456").await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::react("react", "D0001", "1234.0001", "zap")],
            "only the slack-originated turn reacts"
        );
    }

    #[tokio::test]
    async fn no_reaction_when_the_agent_turns_it_off() {
        let h = harness_with(None).await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.publish_response("slack-D0001-1234.0001", "the answer")
            .await;
        h.publish_turn_ended("slack-D0001-1234.0001").await;
        // Lifecycle events are processed in order, so once the next turn's
        // reply is out the first turn's start and end have both been handled.
        h.track("slack-D0001-1234.0002", "D0001", "1234.0002");
        h.publish_turn_started("slack-D0001-1234.0002").await;
        h.publish_response("slack-D0001-1234.0002", "second").await;

        h.wait_for_calls(2).await;
        assert_eq!(
            h.calls(),
            vec![
                Call::post("D0001", None, "the answer"),
                Call::post("D0001", None, "second"),
            ],
            "with working_reaction off, only the replies go out"
        );
    }

    #[tokio::test]
    async fn a_reaction_slack_refuses_never_blocks_the_reply() {
        // E.g. the app was not reinstalled after reactions:write was added.
        let h = harness().await;
        h.fail_next_reaction(SlackApiError::Fatal("missing_scope".to_string()));
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");

        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await; // the refused reaction attempt
        h.publish_response("slack-D0001-1234.0001", "the answer")
            .await;
        h.publish_turn_ended("slack-D0001-1234.0001").await;
        // The next turn's start is processed strictly after this turn's end
        // (one ordered lifecycle channel), so its reaction marks the point
        // by which any removal for the first turn would already be recorded.
        h.track("slack-D0001-1234.0002", "D0001", "1234.0002");
        h.publish_turn_started("slack-D0001-1234.0002").await;

        h.wait_for_calls(3).await;
        assert_eq!(
            h.calls(),
            vec![
                Call::react("react", "D0001", "1234.0001", "zap"),
                Call::post("D0001", None, "the answer"),
                Call::react("react", "D0001", "1234.0002", "zap"),
            ],
            "the reply still goes out, and nothing tries to remove a reaction that never landed"
        );
    }

    #[tokio::test]
    async fn foreign_correlations_never_post_until_an_owner_exists() {
        // No owner bound, nothing tracked: a non-Slack correlation must not
        // resolve to any channel. The Slack reply sent afterwards proves the
        // loop had its chance to mis-deliver the earlier event.
        let h = harness().await;
        h.publish_response("telegram-123-456", "wrong door").await;
        h.track("slack-D0001-1234.0001", "D0001", "1234.0001");
        h.publish_response("slack-D0001-1234.0001", "right door")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::post("D0001", None, "right door")],
            "a foreign correlation with no owner bound resolves to nothing"
        );
    }

    #[tokio::test]
    async fn untracked_correlations_fall_back_to_the_owner_dm() {
        // Telegram's `target_for` sends proactive output (a turn nothing in
        // this chat started) to the owner's DM; Slack mirrors it.
        let h = harness().await;
        h.state
            .store
            .claim_owner(crate::chat_state::Owner {
                user_id: "U1".to_string(),
                name: "U1".to_string(),
                dm_conversation_id: "D0OWNER1".to_string(),
            })
            .await
            .unwrap();

        h.publish_response("cron-nightly-9", "your nightly digest")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![Call::post("D0OWNER1", None, "your nightly digest")],
            "proactive output reaches the owner's DM, never a wrong channel"
        );
    }

    #[test]
    fn parse_channel_id_accepts_slack_shaped_ids() {
        assert_eq!(parse_channel_id("D0001").as_deref(), Some("D0001"));
        assert_eq!(parse_channel_id("C0123ABCD").as_deref(), Some("C0123ABCD"));
        assert_eq!(parse_channel_id("G01XYZ9z").as_deref(), Some("G01XYZ9z"));
    }

    #[test]
    fn parse_channel_id_rejects_non_channel_ids() {
        assert_eq!(parse_channel_id(""), None);
        assert_eq!(parse_channel_id("not-an-id"), None);
        assert_eq!(parse_channel_id("123456789"), None, "never digit-first");
        // A correlation id is not a conversation id.
        assert_eq!(parse_channel_id("slack-D0001-1"), None);
    }

    #[test]
    fn slack_chunk_limit_splits_at_4000() {
        let exactly = "x".repeat(SLACK_MAX_CHARS);
        assert_eq!(
            chunk_text(&exactly, SLACK_MAX_CHARS).len(),
            1,
            "a reply at exactly the limit is one chunk"
        );
        let over = "x".repeat(SLACK_MAX_CHARS + 1);
        let chunks = chunk_text(&over, SLACK_MAX_CHARS);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|chunk| chunk.len() <= SLACK_MAX_CHARS));
        assert_eq!(chunks.concat(), over);
    }

    #[test]
    fn attachment_fallback_names_the_file_and_its_path() {
        let attachment = FileAttachment {
            path: "/inbox/report.pdf".into(),
            filename: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 1024,
        };
        let text = attachment_fallback("here it is", &attachment);
        assert!(text.starts_with("here it is"));
        assert!(text.contains("report.pdf"));
        assert!(text.contains("/inbox/report.pdf"));
        assert!(text.contains("web UI"));
    }
}

//! Slack bus subscriber — translates typed bus events to Slack messages.
//!
//! The mirror of `telegram/subscriber.rs` over the shared
//! `interfaces/outbound.rs` loop: same subscription set (conversation only —
//! agent replies and failed-turn report-backs reach the chat; system notices
//! and errors never do, enforced by what [`BaseSubscribers`] subscribes to),
//! same reply routing through the state's correlation→channel map, same
//! chunking through the shared fence-aware chunker.
//!
//! Three Slack-specific policies:
//!
//! * chunks are converted to mrkdwn ([`super::format`]) one at a time, just
//!   before each send — the chunker splits the markdown the agent produced,
//!   and its fence closing/reopening keeps every chunk independently
//!   convertible;
//! * sends are awaited inline in the delivery loop, never queued: nothing
//!   buffers ahead of [`RateLimitedSender`], whose mutex spans its backoff
//!   sleeps (Task 3 review note) — a slow send backpressures the loop
//!   instead of piling up an unbounded queue behind it;
//! * typing feedback is the placeholder pattern: Slack exposes no typing
//!   signal a bot can send, so a slack-originated turn's start posts
//!   "Thinking…", the first text the turn sends (reply chunk, failure
//!   report-back, …) edits that message in place, and a turn that ends
//!   without sending anything sweeps it — every write through the same
//!   rate-limited sender as replies. Ordering — reply before `Ended`, so
//!   the sweep never beats the reply — is the run loop's biased `select!`
//!   guarantee (see `outbound.rs`); one residual: an intermediate message
//!   queued at the same instant as its turn's `Started` can post before
//!   the placeholder, take nothing, and leave the placeholder for the
//!   final reply — one extra message, never a false one.

use std::sync::Arc;

use async_trait::async_trait;

use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::chunking::chunk_text;
use crate::interfaces::outbound::ChatOutbound;

use super::SlackState;
use super::api::{RateLimitedSender, SlackApi};
use super::format::markdown_to_mrkdwn;

/// Maximum message length for Slack, beside Telegram's 4,096 and Discord's
/// 2,000 in their subscribers. Slack documents ~4,000 characters as the
/// `chat.postMessage` text budget this MVP holds itself to. `pub(super)`:
/// the module's end-to-end test sizes its chunk expectations from it.
pub(super) const SLACK_MAX_CHARS: usize = 4000;

/// Placeholder copy posted while a slack-originated turn runs — the typing
/// feedback Slack's API cannot give a bot (task 10 brief: constant copy).
/// `pub(super)`: the module's end-to-end test asserts it.
pub(super) const PLACEHOLDER_TEXT: &str = "Thinking…";

/// What a turn-end sweep leaves in place of a placeholder no text ever
/// consumed — the placeholder must not survive as a stale "Thinking…"
/// (task 10 brief, requirement 4), and `chat.update` is this MVP's only
/// editing surface, so the sweep rewrites rather than deletes.
const TURN_ENDED_TEXT: &str = "_Turn ended without a reply._";

/// The outbound side: the rate-limited sender plus the state shared with
/// the inbound handler, and the turn placeholders.
struct SlackOutbound<A: SlackApi> {
    sender: Arc<RateLimitedSender<A>>,
    state: Arc<SlackState>,
    /// Live placeholders for in-flight turns, oldest first, as
    /// `(correlation id, placeholder)`. A `Vec`, not a map: in-flight turns
    /// are few, the turn-end sweep looks entries up by correlation, and the
    /// send path consumes by channel in posted order (FIFO) when the bus
    /// event that triggered it no longer names the turn (see
    /// [`SlackOutbound::take_placeholder_for`]). A std mutex, like the
    /// reply-target map it mirrors: critical sections never await.
    placeholders: std::sync::Mutex<Vec<(String, Placeholder)>>,
}

/// One posted "Thinking…" awaiting its turn's first text.
struct Placeholder {
    /// Channel the placeholder was posted to.
    channel: String,
    /// `ts` Slack assigned the placeholder — the handle `chat.update` edits.
    ts: String,
}

/// Receives events from the bus and delivers them to Slack channels.
///
/// Task 7's interface constructor builds the [`BaseSubscribers`] for the
/// `slack` endpoint (as Telegram's and Discord's constructors do) and passes
/// the sender it configured.
pub(super) async fn run_slack_subscriber<A: SlackApi>(
    subs: crate::interfaces::BaseSubscribers,
    sender: Arc<RateLimitedSender<A>>,
    state: Arc<SlackState>,
) {
    crate::interfaces::outbound::run(
        subs,
        SlackOutbound {
            sender,
            state,
            placeholders: std::sync::Mutex::new(Vec::new()),
        },
    )
    .await;
}

#[async_trait]
impl<A: SlackApi> ChatOutbound for SlackOutbound<A> {
    /// A Slack channel id (`"C…"`, `"D…"`), the string `post_message` takes.
    type Target = String;

    fn name(&self) -> &'static str {
        "slack"
    }

    fn unknown_conversation_reason(&self) -> &'static str {
        "that is not a Slack channel ID"
    }

    fn publisher(&self) -> &crate::bus::Publisher {
        &self.state.publisher
    }

    async fn reply_target(&self, correlation_id: &str) -> Option<String> {
        self.state.target_for(correlation_id).await
    }

    fn release_reply(&self, correlation_id: &str) {
        self.state.reply_targets.release(correlation_id);
    }

    async fn conversation_target(&self, conversation_id: &str) -> Option<String> {
        parse_channel_id(conversation_id)
    }

    async fn describe(&self, conversation_id: &str, _target: &String) -> String {
        self.state
            .store
            .conversation(conversation_id)
            .await
            .map_or_else(|| format!("channel {conversation_id}"), |chat| chat.label)
    }

    fn target_label(&self, target: &String) -> String {
        target.clone()
    }

    /// Slack's Web API has no typing indicator to send from the bot's side,
    /// so typing feedback is the placeholder pattern instead (see the module
    /// docs) and this remains a stop channel with no loop behind it —
    /// dropping it is the whole contract.
    fn start_typing(&self, _target: String) -> tokio::sync::watch::Sender<()> {
        tokio::sync::watch::channel(()).0
    }

    /// A turn started: when the correlation is one the Slack handler
    /// tracked — a slack-originated turn; the strict reply-target map, NOT
    /// [`SlackState::target_for`]'s owner-DM fallback, which would light up
    /// foreign turns (telegram, cron, …) — post the placeholder and
    /// remember its `ts` for the first text the turn sends. The post rides
    /// the same rate-limited sender as every reply, awaited inline in the
    /// delivery loop so it strictly precedes anything the turn produces.
    ///
    /// A post that fails logs and moves on: there is simply no placeholder
    /// to edit, and the turn's text posts fresh.
    async fn turn_started(&self, correlation_id: &str) {
        let Some(channel) = self.state.reply_targets.get(correlation_id) else {
            return;
        };
        match self.sender.send(&channel, PLACEHOLDER_TEXT).await {
            Ok(Some(ts)) => self
                .placeholders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((correlation_id.to_string(), Placeholder { channel, ts })),
            // The placeholder posted, but Slack did not report a ts, so
            // there is nothing to edit — the turn's text posts fresh.
            Ok(None) => tracing::warn!(
                correlation_id,
                channel,
                "slack did not report a ts for the placeholder; the reply will post fresh"
            ),
            Err(e) => tracing::warn!(
                correlation_id,
                error = %e,
                "could not post the slack placeholder; the reply will post fresh"
            ),
        }
    }

    /// A turn ended: if no text ever reached the channel (an empty reply,
    /// or one addressed elsewhere), sweep the placeholder so no stale
    /// "Thinking…" survives — best-effort, like the rest of the pattern.
    /// The sweep rewrites rather than deletes because `chat.update` is this
    /// MVP's only editing surface.
    ///
    /// Ordering: the run loop's biased `select!` polls the output topics
    /// before this hook's `turn_lifecycle` arm, and the gateway publishes a
    /// turn's reply (or error) before its `Ended` — so by the time this
    /// runs, the turn's text has already consumed the placeholder whenever
    /// there was one. `a_turns_reply_is_edited_in_before_the_sweep` holds
    /// that invariant.
    async fn turn_ended(&self, correlation_id: &str) {
        let placeholder = {
            // Find and remove under one guard — a separate find and remove
            // would race any future second mutator into an out-of-range
            // panic, the shape `take_placeholder_for` already avoids.
            let mut placeholders = self
                .placeholders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            placeholders
                .iter()
                .position(|(id, _)| id == correlation_id)
                .map(|index| placeholders.remove(index).1)
        };
        let Some(placeholder) = placeholder else {
            return; // the turn's text already consumed its placeholder
        };
        if let Err(e) = self
            .sender
            .edit(&placeholder.channel, &placeholder.ts, TURN_ENDED_TEXT)
            .await
        {
            tracing::warn!(
                correlation_id,
                error = %e,
                "could not sweep the slack placeholder; it stays as last sent"
            );
        }
    }

    async fn send(
        &self,
        channel: &String,
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
        // The first text a channel sees takes that channel's placeholder —
        // reply chunk, failure report-back, or intermediate output alike:
        // whatever turns "Thinking…" into substance in the owner's eyes.
        // Taken before the chunks are built so an empty chunk list (all
        // content, no chunks — unreachable past the early return above)
        // could never strand it.
        let placeholder = self.take_placeholder_for(channel);
        send_chunks(&self.sender, channel, &text, placeholder.as_ref())
            .await
            .map_err(|e| e.to_string())
    }
}

impl<A: SlackApi> SlackOutbound<A> {
    /// Take the oldest placeholder posted to `channel`, if any. The send
    /// path knows only the channel it is sending to — the bus event that
    /// triggered it carries the correlation, but [`ChatOutbound::send`]
    /// does not — so consumption is by channel, in posted order. Two
    /// concurrent turns in one channel each get a placeholder consumed by
    /// one text of that channel, in order; Slack's DM-only MVP has one
    /// conversation per channel anyway.
    fn take_placeholder_for(&self, channel: &str) -> Option<Placeholder> {
        let mut placeholders = self
            .placeholders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = placeholders
            .iter()
            .position(|(_, placeholder)| placeholder.channel == channel);
        index.map(|index| placeholders.remove(index).1)
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

/// Send `content` in mrkdwn-converted chunks, stopping at the first failure
/// — Telegram's `send_chunks` policy verbatim. When the turn left a
/// placeholder, the FIRST chunk edits it in place (by its remembered `ts`);
/// every later chunk posts fresh, in order. An edit that fails — the owner
/// deleted the placeholder, the workspace's edit window closed, a transient
/// transport error — falls back to posting that chunk as a fresh message
/// rather than losing the reply: the placeholder is already spent, so
/// nothing else would deliver it.
async fn send_chunks<A: SlackApi>(
    sender: &RateLimitedSender<A>,
    channel: &str,
    content: &str,
    placeholder: Option<&Placeholder>,
) -> anyhow::Result<()> {
    let mut chunks = chunk_text(content, SLACK_MAX_CHARS).into_iter();
    if let Some(chunk) = chunks.next() {
        let mrkdwn = markdown_to_mrkdwn(&chunk);
        match placeholder {
            Some(placeholder) => {
                if let Err(e) = sender.edit(channel, &placeholder.ts, &mrkdwn).await {
                    tracing::warn!(
                        channel,
                        ts = %placeholder.ts,
                        error = %e,
                        "chat.update failed; posting the chunk as a fresh message instead"
                    );
                    sender.send(channel, &mrkdwn).await?;
                }
            }
            None => {
                sender.send(channel, &mrkdwn).await?;
            }
        }
    }
    for chunk in chunks {
        let mrkdwn = markdown_to_mrkdwn(&chunk);
        sender.send(channel, &mrkdwn).await?;
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

    use crate::bus::{
        BusHandle, EndpointName, ErrorEvent, NoticeEvent, NotifyName, ResponseEvent,
        SYSTEM_CHANNEL, TurnLifecycleEvent, spawn_broker, topics,
    };
    use crate::interfaces::BaseSubscribers;
    use crate::interfaces::chat_state::ChatStateStore;
    use crate::interfaces::reply_targets::ReplyTargets;
    use crate::testing::{clock, wait};

    use super::super::ENDPOINT;
    use super::super::api::SlackApiError;

    /// Records every `post_message` and `chat_update` call — the fake from
    /// `api.rs`'s tests, minus the post scripting (sender behavior is
    /// tested there; here every post succeeds). A post returns a synthetic
    /// ts derived from the call count, so a test can assert which ts an
    /// edit targeted; `chat_update` results are scriptable, for the
    /// edit-failure fallback. The logs are std mutexes so tests can poll
    /// them from `wait::until_true`'s sync closure, like `outbound.rs`'s
    /// `FakeChat`.
    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Arc<std::sync::Mutex<Vec<(String, String)>>>,
        updates: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
        update_script: Arc<std::sync::Mutex<std::collections::VecDeque<Result<(), SlackApiError>>>>,
    }

    #[async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(
            &self,
            channel: &str,
            text: &str,
        ) -> Result<Option<String>, SlackApiError> {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.push((channel.to_string(), text.to_string()));
            Ok(Some(format!("ts.{}", calls.len())))
        }

        async fn chat_update(
            &self,
            channel: &str,
            ts: &str,
            text: &str,
        ) -> Result<(), SlackApiError> {
            self.updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((channel.to_string(), ts.to_string(), text.to_string()));
            self.update_script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Ok(()))
        }

        async fn open_connection(&self) -> anyhow::Result<String> {
            Ok("wss://fake.slack".to_string())
        }
    }

    /// A running subscriber on a real bus, with the state the handler would
    /// have shared. Correlations are tracked directly rather than by
    /// spawning the inbound handler — handler tests already prove dispatch
    /// records them (`dispatch_records_the_reply_target_and_the_conversation`).
    struct Harness {
        api: FakeApi,
        state: Arc<SlackState>,
        bus: BusHandle,
        _dir: tempfile::TempDir,
    }

    async fn harness() -> Harness {
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
        tokio::spawn(crate::interfaces::outbound::run(
            subs,
            SlackOutbound {
                sender,
                state: Arc::clone(&state),
                placeholders: std::sync::Mutex::new(Vec::new()),
            },
        ));
        Harness {
            api,
            state,
            bus,
            _dir: dir,
        }
    }

    impl Harness {
        fn track(&self, correlation_id: &str, channel: &str) {
            self.state
                .reply_targets
                .track(correlation_id, channel.to_string());
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.api
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn updates(&self) -> Vec<(String, String, String)> {
            self.api
                .updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        async fn wait_for_calls(&self, count: usize) {
            wait::until_true(format!("{count} slack call(s)"), || {
                self.calls().len() >= count
            })
            .await;
        }

        async fn wait_for_updates(&self, count: usize) {
            wait::until_true(format!("{count} slack edit(s)"), || {
                self.updates().len() >= count
            })
            .await;
        }

        async fn publish_lifecycle(&self, event: TurnLifecycleEvent) {
            self.bus
                .publisher()
                .publish(topics::Endpoint(EndpointName::from(ENDPOINT)), event)
                .await
                .unwrap();
        }

        async fn publish_turn_started(&self, correlation_id: &str) {
            self.publish_lifecycle(TurnLifecycleEvent::Started {
                correlation_id: correlation_id.to_string(),
            })
            .await;
        }

        async fn publish_turn_ended(&self, correlation_id: &str) {
            self.publish_lifecycle(TurnLifecycleEvent::Ended {
                correlation_id: correlation_id.to_string(),
            })
            .await;
        }

        /// Make the next `chat_update` fail with `error`.
        fn fail_next_update(&self, error: SlackApiError) {
            self.api
                .update_script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(Err(error));
        }

        fn response(correlation_id: &str, content: &str) -> ResponseEvent {
            ResponseEvent {
                correlation_id: correlation_id.to_string(),
                content: content.to_string(),
                timestamp: chrono::Utc::now().naive_utc(),
                attachment: None,
                conversation: None,
            }
        }

        async fn publish_response(&self, correlation_id: &str, content: &str) {
            self.bus
                .publisher()
                .publish(
                    topics::Endpoint(EndpointName::from(ENDPOINT)),
                    Self::response(correlation_id, content),
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

    #[tokio::test(start_paused = true)]
    async fn system_notices_and_errors_never_reach_slack() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        let publisher = h.bus.publisher();
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
        h.publish_response("slack-D0001-1234.0001", "hello").await;

        h.wait_for_calls(1).await;
        clock::elapse(Duration::from_millis(50)).await;
        assert_eq!(
            h.calls(),
            vec![("D0001".to_string(), "hello".to_string())],
            "only the agent's reply may go out; system output stays in the web UI"
        );
    }

    #[tokio::test]
    async fn agent_reply_reaches_the_reply_channel_converted_to_mrkdwn() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");

        h.publish_response("slack-D0001-1234.0001", "**bold** and *italic* with `code`")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![(
                "D0001".to_string(),
                "*bold* and _italic_ with `code`".to_string()
            )],
            "the reply goes to the channel that started the turn, in mrkdwn"
        );
    }

    #[tokio::test]
    async fn long_replies_are_chunked_at_the_slack_limit() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        let content = "x".repeat(SLACK_MAX_CHARS * 2 + 500);

        h.publish_response("slack-D0001-1234.0001", &content).await;

        let expected_chunks = chunk_text(&content, SLACK_MAX_CHARS).len();
        h.wait_for_calls(expected_chunks).await;
        let calls = h.calls();
        assert_eq!(calls.len(), expected_chunks);
        assert!(calls.iter().all(|(_, text)| text.len() <= SLACK_MAX_CHARS));
        assert_eq!(
            calls
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<String>(),
            content,
            "chunks recombine to the reply exactly"
        );
        assert!(
            calls.iter().all(|(channel, _)| channel == "D0001"),
            "every chunk goes to the reply channel"
        );
    }

    #[tokio::test]
    async fn failed_turn_report_back_goes_to_the_channel_that_started_the_turn() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");

        h.publish_turn_error("slack-D0001-1234.0001", "The model is unavailable.")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![(
                "D0001".to_string(),
                "*Error:* The model is unavailable.".to_string()
            )],
            "the failure goes where the reply would have, converted like any other text"
        );
    }

    #[tokio::test]
    async fn a_slack_turn_start_posts_the_thinking_placeholder() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");

        h.publish_turn_started("slack-D0001-1234.0001").await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![("D0001".to_string(), PLACEHOLDER_TEXT.to_string())],
            "a tracked (slack-originated) turn start posts the placeholder to its channel"
        );
        assert!(
            h.updates().is_empty(),
            "the placeholder is a fresh post, not an edit"
        );
    }

    #[tokio::test]
    async fn a_foreign_turn_start_never_posts_a_placeholder() {
        // An owner is bound, so the owner-DM fallback door exists — a
        // foreign turn (telegram, cron, …) must not light it up. The slack
        // turn started afterwards proves the loop had its chance to
        // mis-post for the foreign one.
        let h = harness().await;
        h.state
            .store
            .claim_owner(crate::interfaces::chat_state::Owner {
                user_id: "U1".to_string(),
                name: "U1".to_string(),
                dm_conversation_id: "D0OWNER1".to_string(),
            })
            .await
            .unwrap();

        h.publish_turn_started("telegram-123-456").await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![("D0001".to_string(), PLACEHOLDER_TEXT.to_string())],
            "only the slack-originated turn posts a placeholder; the owner DM stays quiet"
        );
    }

    #[tokio::test]
    async fn the_first_reply_chunk_edits_the_placeholder_and_the_rest_post_fresh() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await; // the placeholder, with ts "ts.1"

        let content = "x".repeat(SLACK_MAX_CHARS * 2 + 500);
        h.publish_response("slack-D0001-1234.0001", &content).await;

        let chunks = chunk_text(&content, SLACK_MAX_CHARS);
        // Placeholder post + every chunk after the first.
        h.wait_for_calls(1 + chunks.len() - 1).await;
        h.wait_for_updates(1).await;

        let first_chunk = chunks.first().expect("a multi-chunk reply has chunks");
        assert_eq!(
            h.updates(),
            vec![("D0001".to_string(), "ts.1".to_string(), first_chunk.clone())],
            "the first chunk edits the placeholder in place, by its remembered ts"
        );
        // The placeholder was calls[0] (posted before the reply was even
        // published); everything after it is the reply's fresh chunks.
        let mut sent = vec![first_chunk.clone()];
        sent.extend(h.calls().iter().skip(1).map(|(_, text)| text.clone()));
        assert_eq!(
            sent.iter().map(String::as_str).collect::<String>(),
            content,
            "edit + fresh posts recombine to the reply exactly, in order"
        );
        assert!(
            h.calls()
                .iter()
                .skip(1)
                .all(|(channel, _)| channel == "D0001"),
            "every later chunk goes to the reply channel"
        );
    }

    #[tokio::test]
    async fn a_failed_turn_edits_the_placeholder_with_the_failure_text() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await; // the placeholder, with ts "ts.1"

        h.publish_turn_error("slack-D0001-1234.0001", "The model is unavailable.")
            .await;

        h.wait_for_updates(1).await;
        assert_eq!(
            h.updates(),
            vec![(
                "D0001".to_string(),
                "ts.1".to_string(),
                "*Error:* The model is unavailable.".to_string()
            )],
            "the report-back edits the placeholder, never leaves a stale Thinking…"
        );
        assert_eq!(
            h.calls().len(),
            1,
            "only the placeholder posted; the failure is the edit"
        );
    }

    /// The gateway publishes a turn's reply and its `Ended` back to back
    /// (`turns.rs`'s `publish_turn_outcome`), on separate subscriber
    /// channels — so delivery order is the run loop's to keep. The loop's
    /// `select!` is biased with `turn_lifecycle` polled last, which makes
    /// "reply first, sweep finds nothing" an invariant rather than a coin
    /// flip (review C-1: the unbiased loop swept ~67% of turns). Many
    /// turns, because each turn under a broken ordering is only a
    /// probabilistic catch; twenty makes a miss astronomically unlikely
    /// while staying fast at zero spacing.
    #[tokio::test]
    async fn a_turns_reply_is_edited_in_before_the_sweep() {
        let h = harness().await;
        let turns = 20;
        for i in 0..turns {
            let correlation_id = format!("slack-D0001-{i}.0001");
            h.track(&correlation_id, "D0001");
            h.publish_turn_started(&correlation_id).await;
            h.wait_for_calls(i + 1).await; // this turn's placeholder

            // The gateway's exact shape: Response then Ended, back to back.
            h.publish_response(&correlation_id, "the answer").await;
            h.publish_turn_ended(&correlation_id).await;

            h.wait_for_updates(i + 1).await;
            let updates = h.updates();
            let edit = updates.last().expect("the placeholder was edited");
            assert_eq!(
                edit.2, "the answer",
                "turn {i}: the reply must be the placeholder's edit, not the sweep"
            );
            assert!(
                updates.iter().all(|(_, _, text)| text != TURN_ENDED_TEXT),
                "turn {i}: the sweep must never fire on a turn that replied"
            );
        }
        assert_eq!(
            h.calls().len(),
            turns,
            "only the placeholders were ever posted: every reply arrived as an edit"
        );
    }

    /// The failure path has the same back-to-back shape — `ErrorEvent` then
    /// `Ended` — and the same guarantee: the error text edits the
    /// placeholder, the sweep finds nothing.
    #[tokio::test]
    async fn a_failed_turns_error_is_edited_in_before_the_sweep() {
        let h = harness().await;
        let turns = 20;
        for i in 0..turns {
            let correlation_id = format!("slack-D0001-{i}.0001");
            h.track(&correlation_id, "D0001");
            h.publish_turn_started(&correlation_id).await;
            h.wait_for_calls(i + 1).await; // this turn's placeholder

            h.publish_turn_error(&correlation_id, "The model is unavailable.")
                .await;
            h.publish_turn_ended(&correlation_id).await;

            h.wait_for_updates(i + 1).await;
            let updates = h.updates();
            let edit = updates.last().expect("the placeholder was edited");
            assert_eq!(
                edit.2, "*Error:* The model is unavailable.",
                "turn {i}: the error must be the placeholder's edit, not the sweep"
            );
            assert!(
                updates.iter().all(|(_, _, text)| text != TURN_ENDED_TEXT),
                "turn {i}: the sweep must never fire on a turn that reported its failure"
            );
        }
        assert_eq!(
            h.calls().len(),
            turns,
            "only the placeholders were ever posted: every failure arrived as an edit"
        );
    }

    /// A failed edit (the owner deleted the placeholder, the workspace edit
    /// window closed, a transient error) must not lose the reply: the chunk
    /// posts fresh, and the rest of the reply continues (review I-1).
    #[tokio::test]
    async fn a_failed_edit_falls_back_to_posting_the_chunk_fresh() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await; // the placeholder, with ts "ts.1"
        h.fail_next_update(SlackApiError::Fatal("message_not_found".to_string()));

        let content = "x".repeat(SLACK_MAX_CHARS * 2 + 500); // three chunks
        h.publish_response("slack-D0001-1234.0001", &content).await;

        // The placeholder plus every chunk as a fresh post — the edit
        // failed, so nothing was lost and the loop ran to the end.
        let chunks = chunk_text(&content, SLACK_MAX_CHARS);
        h.wait_for_calls(1 + chunks.len()).await;
        assert_eq!(h.updates().len(), 1, "the edit was attempted exactly once");
        let sent = h
            .calls()
            .iter()
            .skip(1) // the placeholder
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            sent.iter().map(String::as_str).collect::<String>(),
            content,
            "every chunk was delivered despite the failed edit"
        );
        assert!(
            h.calls()
                .iter()
                .skip(1)
                .all(|(channel, _)| channel == "D0001"),
            "the fallback posts go to the reply channel"
        );
    }

    #[tokio::test]
    async fn a_turn_that_ends_without_text_sweeps_the_placeholder() {
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_turn_started("slack-D0001-1234.0001").await;
        h.wait_for_calls(1).await; // the placeholder, with ts "ts.1"

        // An empty reply sends nothing at all; the turn then ends.
        h.publish_response("slack-D0001-1234.0001", "").await;
        h.publish_turn_ended("slack-D0001-1234.0001").await;

        h.wait_for_updates(1).await;
        assert_eq!(
            h.updates(),
            vec![(
                "D0001".to_string(),
                "ts.1".to_string(),
                TURN_ENDED_TEXT.to_string()
            )],
            "a turn that produced no text sweeps its placeholder"
        );

        // And a later turn's reply posts fresh: the placeholder is gone,
        // not reused. (A new correlation, tracked as the handler would for
        // a new turn — the old turn's mapping was released at its end.)
        h.track("slack-D0001-9999.000900", "D0001");
        h.publish_response("slack-D0001-9999.000900", "later").await;
        h.wait_for_calls(2).await;
        assert_eq!(
            h.calls().last(),
            Some(&("D0001".to_string(), "later".to_string())),
            "after the sweep the next text posts fresh"
        );
        assert_eq!(h.updates().len(), 1, "nothing further is edited");
    }

    #[tokio::test]
    async fn a_reply_without_a_turn_start_posts_fresh_as_before() {
        // The placeholder is opt-in per turn: no turn-start signal, no
        // placeholder to edit — the pre-task-10 behavior, unchanged.
        let h = harness().await;
        h.track("slack-D0001-1234.0001", "D0001");

        h.publish_response("slack-D0001-1234.0001", "hello").await;

        h.wait_for_calls(1).await;
        assert_eq!(h.calls(), vec![("D0001".to_string(), "hello".to_string())]);
        assert!(h.updates().is_empty(), "nothing to edit, nothing edited");
    }

    #[tokio::test]
    async fn foreign_correlations_never_post_until_an_owner_exists() {
        // No owner bound, nothing tracked: a non-Slack correlation must not
        // resolve to any channel. The Slack reply sent afterwards proves the
        // loop had its chance to mis-deliver the earlier event.
        let h = harness().await;
        h.publish_response("telegram-123-456", "wrong door").await;
        h.track("slack-D0001-1234.0001", "D0001");
        h.publish_response("slack-D0001-1234.0001", "right door")
            .await;

        h.wait_for_calls(1).await;
        assert_eq!(
            h.calls(),
            vec![("D0001".to_string(), "right door".to_string())],
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
            .claim_owner(crate::interfaces::chat_state::Owner {
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
            vec![("D0OWNER1".to_string(), "your nightly digest".to_string())],
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

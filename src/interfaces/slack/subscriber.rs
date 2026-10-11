//! Slack bus subscriber — translates typed bus events to Slack messages.
//!
//! The mirror of `telegram/subscriber.rs` over the shared
//! `interfaces/outbound.rs` loop: same subscription set (conversation only —
//! agent replies and failed-turn report-backs reach the chat; system notices
//! and errors never do, enforced by what [`BaseSubscribers`] subscribes to),
//! same reply routing through the state's correlation→channel map, same
//! chunking through the shared fence-aware chunker.
//!
//! Two Slack-specific policies:
//!
//! * chunks are converted to mrkdwn ([`super::format`]) one at a time, just
//!   before each send — the chunker splits the markdown the agent produced,
//!   and its fence closing/reopening keeps every chunk independently
//!   convertible;
//! * sends are awaited inline in the delivery loop, never queued: nothing
//!   buffers ahead of [`RateLimitedSender`], whose mutex spans its backoff
//!   sleeps (Task 3 review note) — a slow send backpressures the loop
//!   instead of piling up an unbounded queue behind it.

// Staged build: the only consumer (the interface constructor, plan Task 7)
// lands in a later commit; until then the lib target sees these items as
// dead. REMOVE this allow when Task 7 wires the interface.
#![allow(
    dead_code,
    reason = "staged build: started by the Task 7 interface constructor; remove when Task 7 wires the interface"
)]

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
/// `chat.postMessage` text budget this MVP holds itself to.
const SLACK_MAX_CHARS: usize = 4000;

/// The outbound side: the rate-limited sender plus the state shared with
/// the inbound handler.
struct SlackOutbound<A: SlackApi> {
    sender: Arc<RateLimitedSender<A>>,
    state: Arc<SlackState>,
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
    crate::interfaces::outbound::run(subs, SlackOutbound { sender, state }).await;
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

    /// Slack's Web API has no typing indicator to send from the bot's side
    /// (and v1's surface is `chat.postMessage` only), so this is a stop
    /// channel with no loop behind it — dropping it is the whole contract.
    fn start_typing(&self, _target: String) -> tokio::sync::watch::Sender<()> {
        tokio::sync::watch::channel(()).0
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
        send_chunks(&self.sender, channel, &text)
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

/// Send `content` in mrkdwn-converted chunks, stopping at the first failure
/// — Telegram's `send_chunks` policy verbatim.
async fn send_chunks<A: SlackApi>(
    sender: &RateLimitedSender<A>,
    channel: &str,
    content: &str,
) -> anyhow::Result<()> {
    for chunk in chunk_text(content, SLACK_MAX_CHARS) {
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
        SYSTEM_CHANNEL, spawn_broker, topics,
    };
    use crate::interfaces::BaseSubscribers;
    use crate::interfaces::chat_state::ChatStateStore;
    use crate::interfaces::reply_targets::ReplyTargets;
    use crate::testing::{clock, wait};

    use super::super::ENDPOINT;
    use super::super::api::SlackApiError;

    /// Records every `post_message` call — the fake from `api.rs`'s tests,
    /// minus the scripted results (sender behavior is tested there; here
    /// every call succeeds). The log is a std mutex so tests can poll it
    /// from `wait::until_true`'s sync closure, like `outbound.rs`'s
    /// `FakeChat`.
    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl SlackApi for FakeApi {
        async fn post_message(&self, channel: &str, text: &str) -> Result<(), SlackApiError> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((channel.to_string(), text.to_string()));
            Ok(())
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

        async fn wait_for_calls(&self, count: usize) {
            wait::until_true(format!("{count} slack call(s)"), || {
                self.calls().len() >= count
            })
            .await;
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

//! Inbound Slack envelope handler — filter chain, owner binding, dedup, dispatch.
//!
//! The socket client ([`super::socket::run_socket`]) invokes its callback
//! SYNCHRONOUSLY, right after each envelope's ack frame is written. Processing an
//! envelope inline there would backpressure every subsequent ack (Slack
//! redelivers un-acked envelopes and eventually drops the connection), so
//! the contract here is the opposite of inline: [`SlackInbound::forwarder`]
//! hands the socket a callback that only `try_send`s into a bounded channel
//! — never blocks, drops on full — and a dedicated task owns every piece of
//! inbound machinery:
//!
//! 1. **Structural filter** — [`DmMessageEvent::parse`] keeps only plain
//!    messages in DM channels; everything else is logged and skipped.
//! 2. **Dedup** — Slack redelivers envelopes (deliberately, after a slow or
//!    missing ack). A redelivery may reuse the envelope id, but Slack's
//!    retry of an un-acked event arrives under a NEW one (the PR #498 live
//!    probe saw 4x/7x duplicate turns that way), so [`EnvelopeDedup`] keys
//!    on the envelope id AND [`MessageDedup`] on the message identity
//!    (channel, ts), which is stable across retries.
//! 3. **Bot exclusion** — a message authored by any bot (our own echoes
//!    included) is dropped before it can bind the owner or publish.
//! 4. **Owner binding** — the first human DM claims ownership, persisted in
//!    `slack_state.json` exactly the way Telegram persists
//!    `telegram_state.json` (see [`crate::chat_state`]): first
//!    claim wins, survives restarts, and is recorded even when
//!    `respond_to_others` is on.
//! 5. **Owner gate** — [`should_process`]: the owner always passes, anyone
//!    else passes only when `respond_to_others` is set.
//! 6. **Dispatch** — the DM channel is recorded in the shared
//!    [`SlackState`] store (Telegram records every addressed chat the same
//!    way, so `list_conversations` and delivery labels work), the
//!    correlation→reply mapping (channel, the thread the owner wrote in, and
//!    the message's own ts for the working reaction) is tracked for the
//!    outbound subscriber (Telegram's `reply_targets.track`), and a `UserMessage`
//!    bus event built like Telegram's (`telegram/handler.rs`'s
//!    `dispatch_message`) is published to [`residuum_bus::topics::UserMessage`];
//!    a publish failure is logged and the loop continues.
//!
//! Task 7 wires this into the interface constructor: load the state store
//! from `WorkspaceLayout::slack_state_json`, call [`SlackInbound::spawn`],
//! and pass [`SlackInbound::forwarder`]'s callback to `run_socket`.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::events::{DmMessageEvent, Envelope};
use super::{ENDPOINT, SlackReply, SlackState, SlackTarget};
use crate::chat_state::{ChatRef, Owner};
use residuum_bus::origin::{ConversationContext, ConversationKind, MessageOrigin};
use residuum_bus::topics;
use residuum_inference::MessageSender;

/// How many envelope ids [`EnvelopeDedup`] remembers.
const DEDUP_CAPACITY: usize = 256;

/// How many envelopes may sit queued between the socket callback and the
/// handler task. Sized like the dedup ring; when it is full the callback
/// drops the envelope rather than block — backpressuring the socket's acks
/// would risk redelivery storms (design, Lifecycle §4).
const ENVELOPE_QUEUE_CAPACITY: usize = DEDUP_CAPACITY;

/// The inbound policy gate, pure and total.
///
/// * Any bot-authored message (`bot_id` set — our own echoes and other
///   bots alike) is excluded structurally, regardless of everything else.
/// * The owner (by user id) always passes.
/// * Anyone else passes only when `respond_to_others` is on; with it off,
///   a stranger is refused whether or not an owner is bound yet.
pub(super) fn should_process(
    ev: &DmMessageEvent,
    owner: Option<&str>,
    respond_to_others: bool,
) -> bool {
    if ev.bot_id.is_some() {
        return false;
    }
    match owner {
        Some(owner) => ev.user == owner || respond_to_others,
        None => respond_to_others,
    }
}

/// A fixed-capacity ring of envelope ids already handled, absorbing Slack's
/// redeliveries. Remembers the most recent [`DEDUP_CAPACITY`] ids; an id
/// pushed out of the ring is treated as fresh again (a redelivery older
/// than 256 envelopes is indistinguishable from, and as harmless as, a new
/// message — the ack already succeeded, so Slack is not resending it).
#[derive(Default)]
pub(super) struct EnvelopeDedup {
    seen: std::collections::VecDeque<String>,
}

impl EnvelopeDedup {
    /// Whether `id` was already seen: `false` the first time (and the id is
    /// remembered), `true` on every repeat.
    pub(super) fn seen(&mut self, id: &str) -> bool {
        if self.seen.iter().any(|seen| seen == id) {
            return true;
        }
        if self.seen.len() == DEDUP_CAPACITY {
            self.seen.pop_front();
        }
        self.seen.push_back(id.to_string());
        false
    }
}

/// A fixed-capacity ring of message identities — (channel, ts) — already
/// handled. Slack's retries of one event carry fresh envelope ids but the
/// same message, so this is the guard that actually holds across retries.
/// Same ring discipline (and capacity) as [`EnvelopeDedup`].
#[derive(Default)]
pub(super) struct MessageDedup {
    ids: EnvelopeDedup,
}

impl MessageDedup {
    /// Whether the message `ts` in `channel` was already seen: `false` the
    /// first time (and it is remembered), `true` on every repeat. A Slack
    /// `ts` is unique only within its channel, hence the pair.
    pub(super) fn seen(&mut self, channel: &str, ts: &str) -> bool {
        // NUL cannot occur in a Slack channel id, so the joined key is
        // unambiguous.
        self.ids.seen(&format!("{channel}\0{ts}"))
    }
}

/// Everything the handler task owns: the dedup rings, the config flag, and
/// the state shared with the outbound subscriber.
struct InboundHandler {
    dedup: EnvelopeDedup,
    message_dedup: MessageDedup,
    /// Owner, DM channel references, reply mappings, and the bus publisher,
    /// shared with the outbound subscriber (`subscriber.rs`).
    state: Arc<SlackState>,
    respond_to_others: bool,
    tz: chrono_tz::Tz,
}

impl InboundHandler {
    /// Run one envelope through the whole chain: parse → dedup → bot
    /// exclusion → owner binding → gate → publish. Never fails — every
    /// problem is logged and the loop moves on to the next envelope.
    async fn handle(&mut self, envelope: Envelope) {
        let Some(event) = envelope.event().and_then(DmMessageEvent::parse) else {
            tracing::debug!(
                envelope_id = %envelope.id,
                "slack handler: not a plain DM message — skipping"
            );
            return;
        };
        // A repeat of an envelope id we already handled (e.g. a reconnect
        // replaying the same envelope) is absorbed here. Slack's retries of
        // an UN-ACKED event do NOT reuse the id — they arrive under a NEW
        // envelope id (PR #498 live probe, 2026-10-12) — so they pass this
        // check and are caught by the (channel, ts) message dedup below. An
        // empty id (only hello/disconnect lack one) cannot be deduped here,
        // so it falls through to the message dedup as-is.
        if !envelope.id.is_empty() && self.dedup.seen(&envelope.id) {
            tracing::debug!(
                envelope_id = %envelope.id,
                "slack handler: redelivered envelope — dedup absorbs it"
            );
            return;
        }
        // Slack's retry of an un-acked event arrives under a new envelope
        // id; the message identity is what stays the same.
        if self.message_dedup.seen(&event.channel, &event.ts) {
            tracing::debug!(
                envelope_id = %envelope.id,
                channel = %event.channel,
                ts = %event.ts,
                "slack handler: message already handled under another envelope — dedup absorbs it"
            );
            return;
        }
        // Bot-authored messages — our own echoes and other bots alike — are
        // excluded before anything else, so a bot can never bind the owner.
        if event.bot_id.is_some() {
            tracing::debug!(
                bot_id = ?event.bot_id,
                envelope_id = %envelope.id,
                "slack handler: bot-authored message — skipping"
            );
            return;
        }
        self.remember_conversation(&event).await;
        self.claim_owner_if_unset(&event).await;
        let owner = self.state.store.owner().await;
        let owner_id = owner.as_ref().map(|owner| owner.user_id.as_str());
        if !should_process(&event, owner_id, self.respond_to_others) {
            tracing::info!(
                user = %event.user,
                "slack message from someone other than the owner; respond_to_others is off"
            );
            return;
        }
        // An empty text (a DM event with no content v1 can use) still bound
        // the owner above, exactly as an empty Telegram DM does — but there
        // is nothing to publish.
        if event.text.is_empty() {
            tracing::debug!(user = %event.user, "slack message had no content, dropping");
            return;
        }
        let is_owner = owner
            .as_ref()
            .is_some_and(|owner| owner.user_id == event.user);
        self.publish_message(&event, is_owner).await;
    }

    /// Record the DM channel in the shared store — Telegram's
    /// `addressed_to_agent` records every addressed chat before its owner
    /// gate, so `list_conversations`, `send_message` target checks, and the
    /// subscriber's delivery labels all work. Slack's MVP is DM-only, so
    /// every admitted-shape human DM records one channel.
    async fn remember_conversation(&self, ev: &DmMessageEvent) {
        let reference = ChatRef::direct_message(&ev.user);
        if let Err(e) = self.state.store.remember(&ev.channel, reference).await {
            tracing::warn!(
                error = %e,
                channel = %ev.channel,
                "failed to save slack conversation"
            );
        }
    }

    /// Make the sender the owner if nobody is yet; only called for messages
    /// that already cleared the bot exclusion, mirroring Telegram's
    /// `claim_owner_if_unset`. Slack v1 has no user-lookup call, so the raw
    /// user id doubles as the display name.
    async fn claim_owner_if_unset(&self, ev: &DmMessageEvent) {
        let owner = Owner {
            user_id: ev.user.clone(),
            name: ev.user.clone(),
            dm_conversation_id: ev.channel.clone(),
        };
        match self.state.store.claim_owner(owner).await {
            Ok(true) => {
                tracing::info!(user = %ev.user, "slack owner set from first direct message");
            }
            Ok(false) => {}
            Err(e) => tracing::error!(error = %e, "failed to save slack owner"),
        }
    }

    /// Build the `UserMessage` event for an admitted DM and publish it,
    /// logging and continuing on failure — the same construction (and
    /// failure wording) as Telegram's `dispatch_message`.
    async fn publish_message(&self, ev: &DmMessageEvent, is_owner: bool) {
        let origin = MessageOrigin {
            endpoint: ENDPOINT.to_string(),
            sender: Some(MessageSender {
                name: ev.user.clone(),
                id: ev.user.clone(),
                interface: ENDPOINT.to_string(),
                location: Some("direct message".to_string()),
            }),
            conversation: Some(ConversationContext {
                id: ev.channel.clone(),
                kind: ConversationKind::Personal,
                is_owner,
            }),
            agent_sender: None,
        };
        // A Slack message ts is unique within its channel, so channel + ts
        // identifies the message the way Telegram's chat id + message id does.
        let correlation_id = format!("slack-{}-{}", ev.channel, ev.ts);
        // Where the turn this message starts should reply — Telegram tracks
        // the chat at this exact spot in `dispatch_message`; the outbound
        // subscriber reads the mapping when the turn's output arrives. A
        // message written inside a thread is answered in that thread (it
        // still reaches main — threads are not separate conversations in
        // v1); a top-level one is answered at the top level.
        self.state.reply_targets.track(
            &correlation_id,
            SlackReply {
                target: SlackTarget {
                    channel: ev.channel.clone(),
                    thread_ts: ev.thread_ts.clone(),
                },
                message_ts: ev.ts.clone(),
            },
        );
        let msg_event = residuum_bus::MessageEvent {
            id: correlation_id,
            content: ev.text.clone(),
            origin,
            timestamp: residuum_core::time::now_local(self.tz),
            images: Vec::new(),
            attachments: Vec::new(),
            context: None,
        };
        if let Err(e) = self
            .state
            .publisher
            .publish(topics::UserMessage, msg_event)
            .await
        {
            tracing::error!(
                user = %ev.user,
                channel = %ev.channel,
                error = %e,
                "failed to publish slack message to bus"
            );
        }
    }
}

/// The running inbound handler task plus the sender half that feeds it.
///
/// Dropping this aborts the task (the guard pattern of Telegram's
/// `OutboundTask`), so a restarted interface never leaves a second handler
/// deduplicating against a dead store. The task also ends on its own once
/// every sender — the socket's forwarder included — is gone.
pub(super) struct SlackInbound {
    tx: mpsc::Sender<Envelope>,
    task: tokio::task::JoinHandle<()>,
}

impl SlackInbound {
    /// Spawn the handler task. The caller (Task 7's interface constructor)
    /// loads the state store from `WorkspaceLayout::slack_state_json` —
    /// surfacing a corrupt-file notice the way Telegram's `start` does —
    /// builds the shared [`SlackState`] around it, and passes
    /// `cfg.respond_to_others` from the resolved [`residuum_config::SlackConfig`].
    /// The same state object goes to `subscriber::run_slack_subscriber`, so
    /// the reply mappings recorded here reach the outbound side.
    pub(super) fn spawn(
        respond_to_others: bool,
        state: Arc<SlackState>,
        tz: chrono_tz::Tz,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel(ENVELOPE_QUEUE_CAPACITY);
        let mut handler = InboundHandler {
            dedup: EnvelopeDedup::default(),
            message_dedup: MessageDedup::default(),
            state,
            respond_to_others,
            tz,
        };
        let task = residuum_core::util::spawn_in_span(async move {
            while let Some(envelope) = rx.recv().await {
                handler.handle(envelope).await;
            }
            tracing::debug!("slack inbound handler: envelope channel closed — task exiting");
        });
        Self { tx, task }
    }

    /// The `on_envelope` callback for [`super::socket::run_socket`].
    ///
    /// Only ever `try_send`s: the socket invokes it synchronously between
    /// ack. A full queue drops the envelope instead (warned) — preferable
    /// to stalling the connection over one message.
    ///
    /// Known v1 limit: the socket has ALREADY acked the envelope by the time
    /// it reaches this callback, so Slack will not retry it — a dropped
    /// envelope's message is lost, not deferred.
    pub(super) fn forwarder(&self) -> impl FnMut(Envelope) + Send + 'static {
        let tx = self.tx.clone();
        move |envelope| match tx.try_send(envelope) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(envelope)) => {
                tracing::warn!(
                    envelope_id = %envelope.id,
                    "slack handler: envelope queue full — dropping an envelope that was ALREADY ACKED; Slack will not retry it, so this message is lost"
                );
            }
            Err(mpsc::error::TrySendError::Closed(envelope)) => {
                tracing::warn!(
                    envelope_id = %envelope.id,
                    "slack handler: envelope channel closed — dropping an envelope that was already acked; Slack will not retry it, so this message is lost"
                );
            }
        }
    }
}

impl Drop for SlackInbound {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    use crate::chat_state::ChatStateStore;
    use crate::reply_targets::ReplyTargets;
    use residuum_bus::{MessageEvent, Subscriber, spawn_broker, topics};
    use residuum_core::testing::wait;

    /// A test harness: a spawned handler on a real bus and a real state
    /// file under a tempdir, fed through the same forwarder the socket
    /// callback will use.
    struct Harness {
        inbound: SlackInbound,
        state: Arc<SlackState>,
        user_messages: Subscriber<MessageEvent>,
        bus: residuum_bus::BusHandle,
        state_path: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    async fn harness(respond_to_others: bool) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("slack_state.json");
        let (store, notice) = ChatStateStore::load(state_path.clone()).await.unwrap();
        assert!(notice.is_none(), "a fresh state file has nothing to report");
        let bus = spawn_broker();
        let user_messages = bus.subscribe(topics::UserMessage).await.unwrap();
        let state = Arc::new(SlackState {
            store,
            reply_targets: ReplyTargets::default(),
            publisher: bus.publisher(),
            deliver_intermediate: false,
        });
        let inbound = SlackInbound::spawn(respond_to_others, Arc::clone(&state), chrono_tz::UTC);
        Harness {
            inbound,
            state,
            user_messages,
            bus,
            state_path,
            _dir: dir,
        }
    }

    impl Harness {
        /// Feed one envelope exactly as the socket callback would.
        fn feed(&self, envelope: Envelope) {
            (self.inbound.forwarder())(envelope);
        }

        /// The next published `UserMessage`, failing fast if the handler
        /// task dies instead of publishing.
        async fn next_user_message(&mut self, what: &str) -> MessageEvent {
            let inbound = &mut self.inbound;
            let user_messages = &mut self.user_messages;
            tokio::select! {
                biased;
                event = wait::guarded(what, user_messages.recv()) => {
                    event
                        .unwrap_or_else(|e| panic!("bus error while waiting for {what}: {e}"))
                        .unwrap_or_else(|| panic!("the bus went away while waiting for {what}"))
                }
                result = &mut inbound.task => {
                    panic!("slack handler task exited while waiting for {what}: {result:?}")
                }
            }
        }
    }

    /// An `events_api` envelope carrying a plain DM message.
    fn dm_envelope(id: &str, user: &str, text: &str, ts: &str) -> Envelope {
        Envelope {
            id: id.to_string(),
            kind: super::super::events::EnvelopeKind::EventsApi,
            payload: Some(serde_json::json!({
                "event": {
                    "type": "message",
                    "channel_type": "im",
                    "channel": "D0001",
                    "user": user,
                    "text": text,
                    "ts": ts,
                }
            })),
        }
    }

    /// The same envelope, authored by a bot instead of a human.
    fn bot_dm_envelope(id: &str, user: &str, text: &str, ts: &str) -> Envelope {
        let mut envelope = dm_envelope(id, user, text, ts);
        envelope
            .payload
            .as_mut()
            .unwrap()
            .get_mut("event")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("bot_id".to_string(), serde_json::json!("B061"));
        envelope
    }

    /// An `events_api` envelope whose event is not a plain DM message.
    fn non_dm_envelope(id: &str) -> Envelope {
        Envelope {
            id: id.to_string(),
            kind: super::super::events::EnvelopeKind::EventsApi,
            payload: Some(serde_json::json!({
                "event": {
                    "type": "message",
                    "channel_type": "channel",
                    "channel": "C0001",
                    "user": "U0001",
                    "text": "hello",
                    "ts": "1",
                }
            })),
        }
    }

    fn dm_event(user: &str, text: &str) -> DmMessageEvent {
        DmMessageEvent {
            channel: "D0001".to_string(),
            user: user.to_string(),
            text: text.to_string(),
            ts: "1".to_string(),
            thread_ts: None,
            bot_id: None,
        }
    }

    #[test]
    fn should_process_truth_table_covers_owner_strangers_and_bots() {
        let owner_msg = dm_event("U1", "hi");
        let stranger_msg = dm_event("U2", "hi");
        let mut bot_msg = dm_event("U1", "beep");
        bot_msg.bot_id = Some("B061".to_string());

        // Owner passes, stranger blocked, when respond_to_others is off.
        assert!(should_process(&owner_msg, Some("U1"), false));
        assert!(!should_process(&stranger_msg, Some("U1"), false));
        // respond_to_others on: the stranger passes alongside the owner.
        assert!(should_process(&stranger_msg, Some("U1"), true));
        // Bot-authored is structurally excluded every which way.
        assert!(!should_process(&bot_msg, Some("U1"), false));
        assert!(!should_process(&bot_msg, Some("U1"), true));
        assert!(!should_process(&bot_msg, None, true));
        // No owner yet: everyone waits for respond_to_others.
        assert!(!should_process(&stranger_msg, None, false));
        assert!(should_process(&stranger_msg, None, true));
    }

    #[test]
    fn dedup_absorbs_redelivery() {
        let mut d = EnvelopeDedup::default();
        assert!(!d.seen("e1"));
        assert!(d.seen("e1"));
        assert!(!d.seen("e2"));
    }

    #[tokio::test]
    async fn dispatch_records_the_reply_target_and_the_conversation() {
        let mut h = harness(false).await;
        h.feed(dm_envelope("env-1", "U1", "hello from slack", "1234.0001"));

        let event = h.next_user_message("the DM published").await;

        // The correlation the subscriber will see on the bus resolves back
        // to the channel the message arrived in.
        assert_eq!(
            h.state.reply_targets.get(&event.id),
            Some(SlackReply {
                target: SlackTarget::top_level("D0001"),
                message_ts: "1234.0001".to_string(),
            }),
            "the handler must track correlation -> top-level reply and the message's ts"
        );
        // The channel itself is recorded, so list_conversations and the
        // subscriber's delivery labels know it.
        let known = h.state.store.known_conversations().await;
        let [only] = known.as_slice() else {
            panic!("exactly the DM channel is recorded: {known:?}")
        };
        assert_eq!(only.id, "D0001");
        assert_eq!(only.label, "direct message with U1");
    }

    #[tokio::test]
    async fn a_message_written_in_a_thread_is_answered_in_that_thread() {
        let mut h = harness(false).await;
        let mut envelope = dm_envelope("env-1", "U1", "in the thread", "1234.0002");
        envelope
            .payload
            .as_mut()
            .unwrap()
            .get_mut("event")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("thread_ts".to_string(), serde_json::json!("1234.0001"));
        h.feed(envelope);

        let event = h.next_user_message("the thread reply published").await;

        assert_eq!(
            h.state.reply_targets.get(&event.id),
            Some(SlackReply {
                target: SlackTarget {
                    channel: "D0001".to_string(),
                    thread_ts: Some("1234.0001".to_string()),
                },
                message_ts: "1234.0002".to_string(),
            }),
            "the reply goes into the thread; the reaction goes on the message itself"
        );
        assert!(
            event.origin.belongs_to_main(),
            "v1: a thread message from the owner still reaches main"
        );
    }

    #[test]
    fn dedup_wraps_around_at_capacity() {
        let mut d = EnvelopeDedup::default();
        for n in 0..DEDUP_CAPACITY {
            assert!(!d.seen(&format!("id-{n}")), "id-{n} is fresh");
        }
        // The ring is exactly full: everything remembered is a repeat.
        assert!(d.seen("id-0"));
        // Pushing one more evicts the oldest, which reads as fresh again.
        assert!(!d.seen("id-new"));
        assert!(!d.seen("id-0"), "id-0 was pushed out of the ring");
        assert!(d.seen("id-2"), "id-2 is still remembered");
        assert!(d.seen("id-new"));
    }

    #[tokio::test]
    async fn first_dm_binds_the_owner_and_persists_across_reload() {
        let mut h = harness(false).await;
        h.feed(dm_envelope("env-1", "U1", "hello from slack", "1234.0001"));

        let event = h.next_user_message("the first DM published").await;
        assert_eq!(event.content, "hello from slack");
        assert_eq!(event.id, "slack-D0001-1234.0001");
        assert_eq!(event.origin.endpoint, "slack");
        let sender = event.origin.sender.expect("sender info present");
        assert_eq!(sender.id, "U1");
        assert_eq!(sender.interface, "slack");
        assert_eq!(sender.location.as_deref(), Some("direct message"));
        let conversation = event
            .origin
            .conversation
            .expect("conversation info present");
        assert_eq!(conversation.id, "D0001");
        assert_eq!(conversation.kind, ConversationKind::Personal);
        assert!(conversation.is_owner, "the first DM sender is the owner");

        // The binding is on disk and survives a reload.
        wait::until_true("the state file is written", || h.state_path.exists()).await;
        let (reloaded, _) = ChatStateStore::<ChatRef>::load(h.state_path.clone())
            .await
            .unwrap();
        let owner = reloaded.owner().await.expect("owner was persisted");
        assert_eq!(owner.user_id, "U1");
        assert_eq!(owner.dm_conversation_id, "D0001");
    }

    #[tokio::test]
    async fn strangers_are_refused_until_respond_to_others_is_on() {
        // Owner-only: U1 binds, then U2's DM is dropped without an event.
        // Channel order proves the refusal: U2 is handled before the U1
        // follow-up that does publish, so a published U2 event would have
        // arrived first.
        let mut h = harness(false).await;
        h.feed(dm_envelope("env-1", "U1", "hi", "1"));
        let first = h.next_user_message("the owner's DM published").await;
        assert!(first.origin.conversation.unwrap().is_owner);

        h.feed(dm_envelope("env-2", "U2", "let me in", "2"));
        h.feed(dm_envelope("env-3", "U1", "still here", "3"));
        let second = h
            .next_user_message("the follow-up that proves U2 was handled")
            .await;
        assert_eq!(second.content, "still here");
        residuum_bus::testing::bus_barrier(&h.bus).await;
        assert!(
            h.user_messages.drain().is_empty(),
            "U2's DM published nothing"
        );
        drop(h);

        // respond_to_others on: U2's DM publishes as a non-owner.
        let mut h_open = harness(true).await;
        h_open.feed(dm_envelope("env-1", "U1", "hi", "1"));
        h_open.feed(dm_envelope("env-2", "U2", "me too", "2"));
        let owner_event = h_open.next_user_message("the owner's DM published").await;
        assert!(owner_event.origin.conversation.unwrap().is_owner);
        let other_event = h_open
            .next_user_message("the stranger's DM published")
            .await;
        assert_eq!(other_event.content, "me too");
        assert!(!other_event.origin.conversation.unwrap().is_owner);

        // The first human still claimed the binding, even with the gate open.
        let (store, _) = ChatStateStore::<ChatRef>::load(h_open.state_path.clone())
            .await
            .unwrap();
        assert_eq!(
            store.owner().await.map(|o| o.user_id),
            Some("U1".to_string())
        );
    }

    #[tokio::test]
    async fn bot_authored_messages_never_bind_the_owner_or_publish() {
        let mut h = harness(false).await;
        h.feed(bot_dm_envelope("env-bot", "U9", "beep", "1"));
        h.feed(dm_envelope("env-1", "U1", "human here", "2"));

        let event = h.next_user_message("the human DM published").await;
        assert_eq!(event.content, "human here");
        residuum_bus::testing::bus_barrier(&h.bus).await;
        assert!(
            h.user_messages.drain().is_empty(),
            "the bot-authored DM published nothing"
        );

        // The bot's message did not claim the owner binding either.
        let (store, _) = ChatStateStore::<ChatRef>::load(h.state_path.clone())
            .await
            .unwrap();
        assert_eq!(
            store.owner().await.map(|o| o.user_id),
            Some("U1".to_string()),
            "the first human, not the bot, owns the binding"
        );
    }

    #[tokio::test]
    async fn non_dm_and_empty_envelopes_are_skipped() {
        let mut h = harness(false).await;
        h.feed(non_dm_envelope("env-channel"));
        h.feed(dm_envelope("env-empty", "U1", "", "1"));
        h.feed(dm_envelope("env-1", "U1", "real message", "2"));

        let event = h.next_user_message("the real DM published").await;
        assert_eq!(event.content, "real message");
        residuum_bus::testing::bus_barrier(&h.bus).await;
        assert!(
            h.user_messages.drain().is_empty(),
            "non-DM and empty-text envelopes publish nothing"
        );
    }

    /// The live-probe defect (PR #498, 2026-10-12): Slack retries an
    /// un-acked event under a NEW `envelope_id`, so envelope-id dedup is
    /// blind to it. The message identity (channel, ts) is stable across
    /// retries — the same message under two envelope ids publishes once.
    #[tokio::test]
    async fn the_same_message_under_a_new_envelope_id_publishes_once() {
        let mut h = harness(false).await;
        h.feed(dm_envelope(
            "env-first-try",
            "U1",
            "hello",
            "1728558000.000100",
        ));
        // Slack's retry: same event, fresh envelope id.
        h.feed(dm_envelope(
            "env-retry-1",
            "U1",
            "hello",
            "1728558000.000100",
        ));
        h.feed(dm_envelope(
            "env-retry-2",
            "U1",
            "hello",
            "1728558000.000100",
        ));
        // A genuinely new message (new ts) in the same channel still flows.
        h.feed(dm_envelope("env-next", "U1", "again", "1728558001.000200"));

        let first = h.next_user_message("the first delivery published").await;
        assert_eq!(first.content, "hello");
        assert_eq!(first.id, "slack-D0001-1728558000.000100");
        let second = h.next_user_message("the next message published").await;
        assert_eq!(second.content, "again");
        residuum_bus::testing::bus_barrier(&h.bus).await;
        assert!(
            h.user_messages.drain().is_empty(),
            "the retries under new envelope ids published nothing"
        );
    }

    #[test]
    fn message_dedup_keys_on_channel_and_ts() {
        let mut d = MessageDedup::default();
        assert!(!d.seen("D0001", "1.0001"));
        assert!(d.seen("D0001", "1.0001"), "same channel + ts is a repeat");
        assert!(
            !d.seen("D0002", "1.0001"),
            "a ts is unique only within its channel"
        );
        assert!(!d.seen("D0001", "1.0002"), "a new ts is a new message");
    }

    #[tokio::test]
    async fn redelivered_envelopes_publish_once() {
        let mut h = harness(false).await;
        h.feed(dm_envelope("env-1", "U1", "hello", "1"));
        // Slack redelivers with the same envelope id.
        h.feed(dm_envelope("env-1", "U1", "hello", "1"));
        h.feed(dm_envelope("env-2", "U1", "again", "2"));

        let first = h.next_user_message("the first delivery published").await;
        assert_eq!(first.content, "hello");
        let second = h.next_user_message("the distinct envelope published").await;
        assert_eq!(second.content, "again");
        residuum_bus::testing::bus_barrier(&h.bus).await;
        assert!(
            h.user_messages.drain().is_empty(),
            "the redelivery published nothing"
        );
    }
}

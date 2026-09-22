//! Agent messaging: routes a `message_agent` call to its target by address.
//!
//! Delivery depends on the target's lifecycle state:
//! - **running/idle session** — delivered through the registry's interrupt
//!   channel: an interrupt at the target's next tool-call boundary while
//!   running, or the input for a new turn while idle (the runtime on the
//!   other end decides which).
//! - **completing session** — the target's current run no longer accepts
//!   input but hasn't yet left the registry (its completion pipeline —
//!   memory merge, transcript write — is still running). Delivery is handed
//!   to a detached task that waits for the run to clear and then resumes the
//!   session, so the sender's call returns immediately rather than blocking
//!   on however long that pipeline takes.
//! - **completed session** — a resume pointer is looked up in the registry
//!   and a fresh `SpawnRequestEvent` is published for it, starting a new run
//!   at the same address through the ordinary spawn-listener path.
//! - **main** — reuses the existing `MessageEvent`/`UserMessage` bus path,
//!   which already implements interrupt-if-running/new-turn-if-idle for the
//!   main agent.
//! - **unknown** — neither a live session nor a resume point exists.
//!
//! Every send is checked against the configured hop-count limits before
//! dispatch: at or above the soft limit the delivered message carries a note
//! asking the receiver to reply only if needed, and at or above the hard
//! limit delivery is refused outright to bound message loops.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::agent::hop::HopLimits;
use crate::agent::interrupt::Interrupt;
use crate::bus::{
    AgentMessageEvent, MessageEvent, Publisher, SessionAddress, SessionEventKind, topics,
};
use crate::config::BackgroundModelTier;
use crate::inference::Message;
use crate::interfaces::types::InboundMessage;

use super::events::publish_session_event;
use super::registry::{DeliverOutcome, MAIN_ADDRESS, ResumePoint, SessionRegistry};
use super::store::SessionStore;

/// What happened when a message was sent to an address.
#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    /// Delivered to the main agent.
    Main,
    /// Delivered to a live (running or idle) session.
    Live(SessionAddress),
    /// The session had completed; a new run was started at the same address.
    Resumed(SessionAddress),
    /// The session's current run is completing (its completion pipeline is
    /// still running). Delivery was handed to a background task that will
    /// resume the session as a new run once that run leaves the registry —
    /// the caller does not wait for that to happen.
    Queued(SessionAddress),
    /// No live session, and no record of this address ever having run.
    Unknown,
}

/// A `message_agent` send could not be completed.
#[derive(Debug, Clone)]
pub enum SendError {
    /// The target's interrupt channel is full. The caller should retry
    /// shortly rather than fall back to resuming a duplicate run — a full
    /// channel means the target is live and actively draining it.
    Busy(SessionAddress),
    /// Publishing the delivery (to main, or as a resume spawn request)
    /// failed at the bus.
    PublishFailed(String),
    /// The message's hop count reached the configured hard limit — this
    /// looks like a message loop, and delivery was refused.
    HopLimitExceeded {
        /// The hop count the message carried.
        hop_count: u32,
        /// The configured hard limit it met or exceeded.
        limit: u32,
    },
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(address) => write!(f, "agent {address} is busy, try again shortly"),
            Self::PublishFailed(reason) => write!(f, "{reason}"),
            Self::HopLimitExceeded { hop_count, limit } => write!(
                f,
                "message loop limit reached ({hop_count} hops, limit {limit}); this looks like \
                 a message loop between agents, so delivery was refused — stop replying and \
                 report back to your spawner or the user instead"
            ),
        }
    }
}

impl std::error::Error for SendError {}

/// Routes agent-to-agent messages by address.
pub struct AgentMessenger {
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    store: Arc<SessionStore>,
    hop_limits: HopLimits,
    /// Hop count of an agent message delivered to main, keyed by the
    /// `MessageEvent.id` it was published under. Main has no interrupt
    /// channel of its own the way a session does — delivery reuses the
    /// generic `MessageEvent`/`UserMessage` bus path — so this is how the
    /// gateway event loop recovers a specific inbound message's hop count
    /// (via [`Self::take_main_hop`]) without threading a new field through
    /// that shared, interface-facing event type. Entries are removed on
    /// read; anything never looked up (a message main never actually
    /// consumed as a turn's kickoff or mid-turn interrupt) is harmless
    /// clutter, not a leak that grows unbounded in practice.
    pending_main_hops: Mutex<HashMap<String, u32>>,
}

impl AgentMessenger {
    /// Create a new messenger over the given registry, bus publisher, and
    /// session store (used to record a best-effort note in a session's
    /// transcript when a hop-limit refusal involves it), enforcing `hop_limits`.
    #[must_use]
    pub(crate) fn new(
        registry: Arc<SessionRegistry>,
        publisher: Publisher,
        store: Arc<SessionStore>,
        hop_limits: HopLimits,
    ) -> Self {
        Self {
            registry,
            publisher,
            store,
            hop_limits,
            pending_main_hops: Mutex::new(HashMap::new()),
        }
    }

    /// The bus publisher this messenger delivers with, for a caller (e.g.
    /// [`super::conversation_router::ConversationRouter`]) that needs to
    /// publish a notice of its own alongside an ordinary delivery.
    #[must_use]
    pub(crate) fn publisher(&self) -> Publisher {
        self.publisher.clone()
    }

    /// Recover the hop count of an agent message delivered to main, given
    /// the `MessageEvent.id` it arrived under. Removes the entry on read.
    /// Returns `0` for any id this messenger never published under — a
    /// genuinely external-origin message (a user message, an interface
    /// message, a subconscious correction) — which is exactly hop `0` per
    /// the design.
    #[must_use]
    pub fn take_main_hop(&self, message_id: &str) -> u32 {
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(message_id)
            .unwrap_or(0)
    }

    /// Send `content` from `from` (identified by address and category) to
    /// `to`, carrying `hop_count`. `to` may be `"main"`, a live session's
    /// address, or a completed (or completing) session's address.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::HopLimitExceeded`] if `hop_count` has reached the
    /// configured hard limit — delivery is refused outright and never
    /// reaches `to`. Returns [`SendError::Busy`] if the target is live but
    /// its interrupt channel is saturated, and [`SendError::PublishFailed`]
    /// if delivering the message (to main, or as a resume spawn request)
    /// failed at the bus. All three are real delivery failures the caller
    /// must not treat as success.
    pub async fn send(
        &self,
        to: &str,
        from: SessionAddress,
        from_category: String,
        content: String,
        hop_count: u32,
    ) -> Result<DeliveryOutcome, SendError> {
        if hop_count >= self.hop_limits.hard {
            self.refuse_hop_limit(&from, to, hop_count).await;
            return Err(SendError::HopLimitExceeded {
                hop_count,
                limit: self.hop_limits.hard,
            });
        }
        let content = self.apply_soft_note(hop_count, content);

        if to == MAIN_ADDRESS {
            self.deliver_to_main(from, from_category, content, hop_count)
                .await?;
            return Ok(DeliveryOutcome::Main);
        }

        let address = SessionAddress::from(to);
        let message = AgentMessageEvent {
            from: from.clone(),
            from_category: from_category.clone(),
            content: content.clone(),
            hop_count,
        };
        match self
            .registry
            .deliver(&address, Interrupt::AgentMessage(message.clone()))
        {
            DeliverOutcome::Delivered => return Ok(DeliveryOutcome::Live(address)),
            DeliverOutcome::Full => return Err(SendError::Busy(address)),
            DeliverOutcome::Completing => {
                // The target's completion pipeline (memory merge, transcript
                // write) may still be running — potentially including an LLM
                // call — so waiting for it here would block the sender's
                // tool call for however long that takes. Hand the wait off
                // to a detached task instead: `send` returns as soon as the
                // task is spawned, and the task itself resumes the session
                // once the run actually clears (see `deferred_resume`). The
                // resume point isn't looked up until then, since it may not
                // exist yet at this exact moment — `finish_run` records it
                // only partway through the pipeline, before the run leaves
                // the registry.
                let registry = Arc::clone(&self.registry);
                let publisher = self.publisher.clone();
                let store = Arc::clone(&self.store);
                let deferred_address = address.clone();
                tokio::spawn(async move {
                    deferred_resume(registry, publisher, store, deferred_address, message).await;
                });
                return Ok(DeliveryOutcome::Queued(address));
            }
            DeliverOutcome::NotLive => {}
        }

        let Some(point) = self.registry.resume_point(&address) else {
            return Ok(DeliveryOutcome::Unknown);
        };
        self.resume(&address, &point, message).await?;
        Ok(DeliveryOutcome::Resumed(address))
    }

    /// Append a hop-count-note to `content` when `hop_count` has reached the
    /// configured soft limit, so the receiver knows this is a long-running
    /// exchange and should only reply if a reply is actually needed.
    fn apply_soft_note(&self, hop_count: u32, content: String) -> String {
        if hop_count >= self.hop_limits.soft {
            format!(
                "{content}\n\n[This exchange has reached {hop_count} hops. Reply only if a \
                 reply is actually needed.]"
            )
        } else {
            content
        }
    }

    /// Log the hop-limit refusal and record a best-effort note in the
    /// transcript of whichever side (`from`, `to`) is a live, addressable
    /// session — main has no comparable transcript to write into, so it's
    /// skipped there and covered by the log alone.
    async fn refuse_hop_limit(&self, from: &SessionAddress, to: &str, hop_count: u32) {
        tracing::warn!(
            from = %from,
            to = %to,
            hop_count,
            limit = self.hop_limits.hard,
            "refusing to deliver agent message: hop limit reached, likely a message loop"
        );
        let note = format!(
            "[Message Loop Limit] a message from {from} to {to} reached the hop limit \
             ({hop_count} >= {}); it was not delivered.",
            self.hop_limits.hard
        );
        let sinks = NoteSinks {
            registry: &self.registry,
            store: &self.store,
            publisher: &self.publisher,
        };
        record_note_if_live_session(sinks, from, &note).await;
        record_note_if_live_session(sinks, &SessionAddress::from(to), &note).await;
    }

    /// Deliver to the main agent by publishing a `MessageEvent` on the
    /// `UserMessage` topic, exactly like a relayed session result does today
    /// — the gateway's own turn loop already treats an inbound message on
    /// this topic as an interrupt when a turn is running, and as fresh input
    /// for a new turn when it's idle. The hop count is recorded under the
    /// event's id (see [`Self::take_main_hop`]) rather than carried on
    /// `MessageEvent` itself.
    async fn deliver_to_main(
        &self,
        from: SessionAddress,
        from_category: String,
        content: String,
        hop_count: u32,
    ) -> Result<(), SendError> {
        let msg = AgentMessageEvent {
            from,
            from_category,
            content,
            hop_count,
        };
        let event = MessageEvent::from_background(msg.format_for_agent());
        let message_id = event.id.clone();
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(message_id.clone(), hop_count);
        if let Err(e) = self.publisher.publish(topics::UserMessage, event).await {
            // The message never reached main, so this hop count will never
            // be looked up via `take_main_hop` — remove it rather than
            // leaving a permanent entry behind for an id nothing will ever
            // read.
            self.pending_main_hops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&message_id);
            tracing::warn!(error = %e, "failed to deliver agent message to main");
            return Err(SendError::PublishFailed(
                "failed to deliver message to main".to_string(),
            ));
        }
        Ok(())
    }

    /// Resume a completed session as a new run at the same address, by
    /// publishing a fresh `SpawnRequestEvent` for the spawn listener to pick
    /// up — the same path any other session fork takes. The new run's
    /// prompt is the delivered message; its context carries a pointer back
    /// to the previous run's episode (or its run id, if it produced none),
    /// so the resumed session can retrieve it with `memory_get`. The new
    /// run's hop count is the delivered message's hop count directly (that
    /// message *is* the run's kickoff input).
    async fn resume(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        msg: AgentMessageEvent,
    ) -> Result<(), SendError> {
        let hop_count = msg.hop_count;
        publish_resume(
            &self.publisher,
            address,
            point,
            msg.format_for_agent(),
            hop_count,
        )
        .await
    }

    /// Resume a session as a new run, combining several buffered messages
    /// (agent messages, conversation messages, or a mix — anything that can
    /// land in a session's interrupt channel) into a single kickoff prompt.
    /// The resumed run's hop count is the highest hop count among the
    /// combined messages — together they're the inputs driving its first
    /// turn.
    ///
    /// Used when a run's own interrupt channel still holds messages at the
    /// moment its teardown drains it (see
    /// `crate::background::runtime::finish_run`): by then there is no live
    /// turn left to deliver them into individually, so they all become the
    /// resumed run's opening input instead.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::PublishFailed`] if publishing the resume spawn
    /// request fails.
    pub(crate) async fn resume_with_pending(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        pending: &[PendingInput],
    ) -> Result<(), SendError> {
        let combined = pending
            .iter()
            .map(PendingInput::render_for_resume)
            .collect::<Vec<_>>()
            .join("\n\n");
        let hop_count = pending
            .iter()
            .map(PendingInput::hop_count)
            .max()
            .unwrap_or(0);
        publish_resume(&self.publisher, address, point, combined, hop_count).await
    }

    /// Deliver an inbound conversation message to its session by its
    /// deterministic address, per the delivery rules: an interrupt at the
    /// next tool-call boundary if it's running, a new turn if it's idle, a
    /// deferred resume if its current run is completing, and — unlike
    /// [`Self::send`], which requires the target to already exist — a fresh
    /// spawn if no run has ever executed at this address (the first message
    /// in a brand-new conversation).
    ///
    /// Inbound conversation messages are external input, so unlike
    /// [`Self::send`] this carries no hop-count check: they are always hop
    /// `0`, below both limits by construction.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Busy`] if the target is live but its interrupt
    /// channel is saturated, and [`SendError::PublishFailed`] if starting or
    /// resuming the session failed at the bus.
    pub(crate) async fn deliver_conversation(
        &self,
        address: &SessionAddress,
        inbound: InboundMessage,
        spawn: ConversationSpawn,
    ) -> Result<ConversationDeliveryOutcome, SendError> {
        match self
            .registry
            .deliver(address, Interrupt::UserMessage(inbound.clone()))
        {
            DeliverOutcome::Delivered => {
                return Ok(ConversationDeliveryOutcome::Live(address.clone()));
            }
            DeliverOutcome::Full => return Err(SendError::Busy(address.clone())),
            DeliverOutcome::Completing => {
                // Mirrors `send`'s own `Completing` branch: hand the wait
                // off to a detached task rather than blocking the caller
                // (an interface's inbound handler) on however long the
                // completing run's own teardown takes.
                let registry = Arc::clone(&self.registry);
                let publisher = self.publisher.clone();
                let deferred_address = address.clone();
                tokio::spawn(async move {
                    deferred_conversation_resume(
                        registry,
                        publisher,
                        deferred_address,
                        inbound,
                        spawn,
                    )
                    .await;
                });
                return Ok(ConversationDeliveryOutcome::Queued(address.clone()));
            }
            DeliverOutcome::NotLive => {}
        }

        if let Some(point) = self.registry.resume_point(address) {
            publish_conversation_resume(&self.publisher, address, &point, &inbound).await?;
            Ok(ConversationDeliveryOutcome::Resumed(address.clone()))
        } else {
            publish_conversation_spawn(&self.publisher, address.clone(), inbound, spawn).await?;
            Ok(ConversationDeliveryOutcome::Started(address.clone()))
        }
    }
}

/// A message that can arrive in a session's interrupt channel, held together
/// only for [`AgentMessenger::resume_with_pending`]'s combined-kickoff
/// rendering when a run's teardown drains a mix of both kinds.
#[derive(Debug, Clone)]
pub(crate) enum PendingInput {
    /// Another agent addressed this session (`message_agent`).
    Agent(AgentMessageEvent),
    /// A new message arrived in this session's own conversation.
    External(InboundMessage),
}

impl PendingInput {
    fn hop_count(&self) -> u32 {
        match self {
            Self::Agent(m) => m.hop_count,
            // Inbound conversation messages are external input: hop 0.
            Self::External(_) => 0,
        }
    }

    /// Render this input as it would read in a resumed run's opening prompt.
    fn render_for_resume(&self) -> String {
        match self {
            Self::Agent(m) => m.format_for_agent(),
            Self::External(m) => {
                let mut parts = Vec::new();
                if let Some(ctx) = &m.context {
                    parts.push(ctx.clone());
                }
                // Reuses `Message::attributed_content`'s formatting rather
                // than hand-rolling the `[From: …]` prefix a second time, so
                // the two stay in sync if that format ever changes.
                let attributed = Message::user(m.content.clone())
                    .with_sender(m.origin.sender.clone())
                    .attributed_content()
                    .into_owned();
                parts.push(attributed);
                parts.join("\n\n")
            }
        }
    }
}

/// Parameters for starting a brand-new conversation session when no run has
/// ever executed at its deterministic address.
#[derive(Debug, Clone)]
pub(crate) struct ConversationSpawn {
    /// Human-readable source label (e.g. `"discord:#builds"`).
    pub(crate) source_label: String,
    /// Model tier to run the session at.
    pub(crate) model_tier: BackgroundModelTier,
}

/// Outcome of delivering an inbound conversation message to its session.
#[derive(Debug, Clone)]
pub(crate) enum ConversationDeliveryOutcome {
    /// Delivered into a live (running or idle) session.
    Live(SessionAddress),
    /// No run had ever executed at this address; a fresh one was started.
    Started(SessionAddress),
    /// The session had completed a previous run; a new run was started,
    /// picking up from where it left off.
    Resumed(SessionAddress),
    /// The session's current run is completing; delivery was handed to a
    /// detached task that starts or resumes the session once that run
    /// clears the registry.
    Queued(SessionAddress),
}

/// Build the `SpawnRequestEvent` that starts a brand-new conversation
/// session, and publish it for the spawn listener to pick up.
async fn publish_conversation_spawn(
    publisher: &Publisher,
    address: SessionAddress,
    inbound: InboundMessage,
    spawn: ConversationSpawn,
) -> Result<(), SendError> {
    let Some(conversation) = inbound.origin.conversation.as_ref() else {
        // The router only ever calls this for messages it has already
        // classified as conversation input (see
        // `MessageOrigin::belongs_to_main`), so this is an internal
        // inconsistency, not a real-world input this needs to tolerate.
        tracing::error!(
            address = %address,
            "conversation spawn requested for an inbound message with no conversation context"
        );
        return Err(SendError::PublishFailed(
            "internal error: no conversation context on an external message".to_string(),
        ));
    };
    let conversation_id = conversation.id.clone();
    let original_inbound = inbound.clone();
    let event = crate::bus::SpawnRequestEvent {
        address,
        skill: None,
        source_label: spawn.source_label,
        prompt: inbound.content,
        context: inbound.context,
        source: crate::bus::EventTrigger::Conversation,
        model_tier: spawn.model_tier,
        spawner: None,
        depth: crate::background::registry::MAIN_DEPTH + 1,
        hop_count: 0,
        sender: inbound.origin.sender,
        conversation: Some(crate::bus::ConversationTarget {
            endpoint: inbound.origin.endpoint,
            conversation_id,
        }),
        inbound: Some(original_inbound),
    };
    publisher
        .publish(topics::Background, event)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to publish conversation spawn request");
            SendError::PublishFailed("failed to start a conversation session".to_string())
        })
}

/// Build the `SpawnRequestEvent` that resumes a completed conversation
/// session as a new run, carrying the previous run's pointer alongside the
/// new inbound message.
async fn publish_conversation_resume(
    publisher: &Publisher,
    address: &SessionAddress,
    point: &ResumePoint,
    inbound: &InboundMessage,
) -> Result<(), SendError> {
    let context = match &inbound.context {
        Some(ctx) => format!("{}\n\n{ctx}", pointer_note(point)),
        None => pointer_note(point),
    };
    let event = crate::bus::SpawnRequestEvent {
        address: address.clone(),
        skill: point.agent_skill.clone(),
        source_label: point.source_label.clone(),
        prompt: inbound.content.clone(),
        context: Some(context),
        source: point.trigger.clone(),
        model_tier: point.model_tier,
        spawner: point.spawner.clone(),
        depth: point.depth,
        hop_count: 0,
        sender: inbound.origin.sender.clone(),
        conversation: point.conversation_target.clone(),
        inbound: Some(inbound.clone()),
    };
    publisher.publish(topics::Background, event).await.map_err(|e| {
        tracing::error!(error = %e, address = %address, "failed to publish conversation resume");
        SendError::PublishFailed(format!("failed to resume conversation session {address}"))
    })
}

/// Wait for `address` to leave the registry, then re-run the same
/// live-vs-start-vs-resume decision [`AgentMessenger::deliver_conversation`]
/// makes for a fresh call — the deferred half of its `Completing` branch, run
/// on its own detached task so the original caller never blocks on it.
async fn deferred_conversation_resume(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    address: SessionAddress,
    inbound: InboundMessage,
    spawn: ConversationSpawn,
) {
    loop {
        registry.wait_until_clear(&address).await;
        let done = resume_or_start_conversation_after_clear(
            &registry, &publisher, &address, &inbound, &spawn,
        )
        .await;
        if done {
            return;
        }
        // Falling through loops back to `wait_until_clear` again — another
        // resume raced in and is already tearing down once more.
    }
}

/// The decision `deferred_conversation_resume` makes once `address` has been
/// observed clear: deliver into a run that already won the race, resume from
/// its resume point, or start a fresh run if it never had one. Returns
/// `false` only for [`DeliverOutcome::Completing`] (another resume raced in
/// and is *already* tearing down again), telling the caller to wait and
/// retry — every other outcome, success or failure, is final.
async fn resume_or_start_conversation_after_clear(
    registry: &SessionRegistry,
    publisher: &Publisher,
    address: &SessionAddress,
    inbound: &InboundMessage,
    spawn: &ConversationSpawn,
) -> bool {
    match registry.deliver(address, Interrupt::UserMessage(inbound.clone())) {
        DeliverOutcome::Delivered => true,
        DeliverOutcome::Completing => false,
        DeliverOutcome::Full => {
            tracing::error!(
                address = %address,
                "deferred conversation delivery failed: interrupt channel saturated; message dropped"
            );
            true
        }
        DeliverOutcome::NotLive => {
            let result = match registry.resume_point(address) {
                Some(point) => {
                    publish_conversation_resume(publisher, address, &point, inbound).await
                }
                None => {
                    publish_conversation_spawn(
                        publisher,
                        address.clone(),
                        inbound.clone(),
                        spawn.clone(),
                    )
                    .await
                }
            };
            if let Err(e) = result {
                tracing::error!(error = %e, address = %address, "failed to deliver deferred conversation message");
            }
            true
        }
    }
}

/// Best-effort: append `note` to `address`'s live transcript sidecar and
/// publish it as an error event on that session's stream (so the web UI
/// shows it on the affected session), if `address` names a
/// currently-registered session. A no-op for `main` (which has no
/// session-store transcript or session stream) and for an address with no
/// live entry (nothing to append into that would actually surface). Free
/// function (not a method) so it can be shared between
/// [`AgentMessenger::refuse_hop_limit`] and [`deferred_resume`], which runs
/// on a detached task with no `&AgentMessenger` to call through.
async fn record_note_if_live_session(sinks: NoteSinks<'_>, address: &SessionAddress, note: &str) {
    if address.as_ref() == MAIN_ADDRESS {
        return;
    }
    if let Some(info) = sinks.registry.get(address) {
        sinks
            .store
            .append_note(&info.run_id, info.started_at, note)
            .await;
        publish_session_event(
            sinks.publisher,
            address,
            &info.run_id,
            SessionEventKind::Error {
                message: note.to_string(),
            },
        )
        .await;
    }
}

/// Where [`record_note_if_live_session`] records a note: the registry to
/// find the session's live run, the store holding its transcript, and the
/// publisher for its event stream.
#[derive(Clone, Copy)]
struct NoteSinks<'a> {
    registry: &'a SessionRegistry,
    store: &'a SessionStore,
    publisher: &'a Publisher,
}

/// Wait for `address` to leave the registry, then re-run the same
/// live-vs-resume decision [`AgentMessenger::send`] makes for a fresh call —
/// the deferred half of `send`'s `Completing` branch, run on its own
/// detached task so the original sender never blocks on it.
///
/// By the time the wait ends, another deferred message queued at the same
/// address may already have resumed it (a live run now exists to deliver
/// into), so re-checking liveness here — rather than blindly publishing
/// another resume — is what keeps two messages queued to one completing
/// session from producing two runs, with the second silently dropped once
/// the spawn listener's own liveness guard refuses it. A `Completing` result
/// here (another resume raced in and is *already* tearing down again) waits
/// once more rather than giving up.
async fn deferred_resume(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    store: Arc<SessionStore>,
    address: SessionAddress,
    msg: AgentMessageEvent,
) {
    loop {
        registry.wait_until_clear(&address).await;
        let done =
            resume_or_deliver_after_clear(&registry, &publisher, &store, &address, &msg).await;
        if done {
            return;
        }
        // Falling through loops back to `wait_until_clear` again — another
        // resume raced in and is already tearing down once more.
    }
}

/// The decision `deferred_resume` makes once `address` has been observed
/// clear: if a run is now live there (another deferred message queued at the
/// same address already resumed it before this one got a chance to check),
/// deliver `msg` into it directly instead of blindly publishing a second
/// resume; otherwise resume it. Returns `false` only for
/// [`DeliverOutcome::Completing`] (another resume raced in and is *already*
/// tearing down again), telling the caller to wait and retry — every other
/// outcome, success or failure, is final.
async fn resume_or_deliver_after_clear(
    registry: &SessionRegistry,
    publisher: &Publisher,
    store: &SessionStore,
    address: &SessionAddress,
    msg: &AgentMessageEvent,
) -> bool {
    match registry.deliver(address, Interrupt::AgentMessage(msg.clone())) {
        DeliverOutcome::Delivered => true,
        DeliverOutcome::Completing => false,
        DeliverOutcome::Full => {
            let reason = "the resumed session's interrupt channel is saturated";
            tracing::error!(address = %address, from = %msg.from, reason, "deferred delivery failed");
            record_note_if_live_session(
                NoteSinks {
                    registry,
                    store,
                    publisher,
                },
                &msg.from,
                &format!(
                    "[Deferred Delivery Failed] your message to {address} could not be \
                     delivered: {reason}"
                ),
            )
            .await;
            true
        }
        DeliverOutcome::NotLive => {
            // By the time the entry clears, a resume point for this exact
            // run is guaranteed to exist (`finish_run` always records one
            // before removing the entry — see its doc comment), so a
            // missing point here would be an internal inconsistency, not a
            // genuinely unknown address.
            let Some(point) = registry.resume_point(address) else {
                tracing::error!(
                    address = %address,
                    "session left the registry with no resume point recorded; deferred message not delivered"
                );
                record_note_if_live_session(
                    NoteSinks {
                        registry,
                        store,
                        publisher,
                    },
                    &msg.from,
                    &format!(
                        "[Deferred Delivery Failed] your message to {address} could not be \
                         delivered: no resume point recorded"
                    ),
                )
                .await;
                return true;
            };
            let hop_count = msg.hop_count;
            if let Err(e) = publish_resume(
                publisher,
                address,
                &point,
                msg.format_for_agent(),
                hop_count,
            )
            .await
            {
                tracing::error!(error = %e, address = %address, "failed to deliver deferred resume");
                record_note_if_live_session(
                    NoteSinks {
                        registry,
                        store,
                        publisher,
                    },
                    &msg.from,
                    &format!(
                        "[Deferred Delivery Failed] your message to {address} could not be \
                         delivered: {e}"
                    ),
                )
                .await;
            }
            true
        }
    }
}

/// Build and publish the `SpawnRequestEvent` that resumes `address` from
/// `point`, with `prompt` as the new run's opening input and `hop_count` as
/// its first turn's hop count. Free function (not a method) so it can be
/// shared between [`AgentMessenger`]'s own resume paths and [`deferred_resume`],
/// which runs on a detached task with no `&AgentMessenger` to call through.
async fn publish_resume(
    publisher: &Publisher,
    address: &SessionAddress,
    point: &ResumePoint,
    prompt: String,
    hop_count: u32,
) -> Result<(), SendError> {
    let event = crate::bus::SpawnRequestEvent {
        address: address.clone(),
        skill: point.agent_skill.clone(),
        source_label: point.source_label.clone(),
        prompt,
        context: Some(pointer_note(point)),
        source: point.trigger.clone(),
        model_tier: point.model_tier,
        // A resume keeps the session's original spawner and depth, rather
        // than resetting it to depth 1 with no spawner — which would let a
        // resumed session evade the nesting cap.
        spawner: point.spawner.clone(),
        depth: point.depth,
        hop_count,
        // An agent message carries no sender attribution; a conversation
        // session's own conversation target is still carried over, though,
        // so a resume triggered by `message_agent` doesn't strand it.
        sender: None,
        conversation: point.conversation_target.clone(),
        // This resume was triggered by a plain agent message
        // (`message_agent`/`resume_with_pending`), never an inbound
        // conversation message — even when `point.trigger` is
        // `Conversation` (a previously conversation-triggered session,
        // resumed by an agent addressing it directly). A race-guard delivery
        // for this request must fall back to `Interrupt::AgentMessage`, not
        // fabricate a `UserMessage` with no real sender.
        inbound: None,
    };

    publisher
        .publish(topics::Background, event)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address = %address, "failed to publish resume spawn request");
            SendError::PublishFailed(format!("failed to resume session {address}"))
        })
}

/// Build the note telling a resumed session how to retrieve what its
/// previous run produced.
fn pointer_note(point: &ResumePoint) -> String {
    match &point.previous_episode_id {
        Some(episode_id) => format!(
            "[Resumed session] Your previous run ({}) was merged as episode {episode_id} — \
             retrieve it with memory_get if useful.",
            point.previous_run_id
        ),
        None => format!(
            "[Resumed session] Your previous run ({}) produced no episode — retrieve its \
             transcript with memory_get using that run id if useful.",
            point.previous_run_id
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{EventTrigger, SkillName, Subscriber};
    use tokio_util::sync::CancellationToken;

    const NO_LIMIT: HopLimits = HopLimits { soft: 8, hard: 32 };

    fn messenger() -> (AgentMessenger, Arc<SessionRegistry>, crate::bus::BusHandle) {
        messenger_with_limits(NO_LIMIT)
    }

    fn messenger_with_limits(
        limits: HopLimits,
    ) -> (AgentMessenger, Arc<SessionRegistry>, crate::bus::BusHandle) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger =
            AgentMessenger::new(Arc::clone(&registry), bus_handle.publisher(), store, limits);
        (messenger, registry, bus_handle)
    }

    fn sample_resume_point(
        run_id: &str,
        model_tier: crate::config::BackgroundModelTier,
    ) -> ResumePoint {
        ResumePoint {
            previous_run_id: run_id.to_string(),
            previous_episode_id: Some("ep-42".to_string()),
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            agent_skill: Some(SkillName::from("researcher")),
            model_tier,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            conversation_target: None,
        }
    }

    fn sample_live_info(
        address: &str,
        state: crate::background::registry::SessionState,
    ) -> crate::background::registry::SessionInfo {
        crate::background::registry::SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            category: crate::background::registry::SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: None,
            started_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn send_to_main_publishes_a_user_message_naming_the_sender() {
        let (messenger, _registry, bus_handle) = messenger();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        let outcome = messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-3f9a"),
                "spawned".to_string(),
                "found the answer".to_string(),
                0,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Main));

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.content.contains("spawned-researcher-3f9a"));
        assert!(event.content.contains("spawned"));
        assert!(event.content.contains("found the answer"));
        assert_eq!(event.origin.endpoint, "background");
        assert_eq!(messenger.take_main_hop(&event.id), 0);
    }

    #[tokio::test]
    async fn take_main_hop_recovers_the_hop_count_and_removes_the_entry() {
        let (messenger, _registry, bus_handle) = messenger();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-3f9b"),
                "spawned".to_string(),
                "an update".to_string(),
                4,
            )
            .await
            .unwrap();
        let event = sub.recv().await.unwrap().unwrap();

        assert_eq!(messenger.take_main_hop(&event.id), 4);
        assert_eq!(
            messenger.take_main_hop(&event.id),
            0,
            "a second read must not find the same entry again"
        );
    }

    #[tokio::test]
    async fn take_main_hop_on_an_unknown_id_is_hop_zero() {
        let (messenger, _registry, _bus_handle) = messenger();
        assert_eq!(messenger.take_main_hop("never-sent"), 0);
    }

    #[tokio::test]
    async fn send_to_live_session_delivers_via_registry() {
        let (messenger, registry, _bus_handle) = messenger();
        let info = sample_live_info(
            "spawned-researcher-0001",
            crate::background::registry::SessionState::Idle,
        );
        let mut rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let outcome = messenger
            .send(
                info.address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "how's it going?".to_string(),
                0,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Live(addr) if addr == info.address));

        let received = rx.try_recv().expect("message should be queued");
        assert!(matches!(received, Interrupt::AgentMessage(_)));
    }

    #[tokio::test]
    async fn send_to_unknown_address_reports_unknown() {
        let (messenger, _registry, _bus_handle) = messenger();
        let outcome = messenger
            .send(
                "spawned-ghost-0000",
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "hello?".to_string(),
                0,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Unknown));
    }

    #[tokio::test]
    async fn send_to_a_full_channel_reports_busy_and_never_resumes() {
        let (messenger, registry, bus_handle) = messenger();
        let mut spawn_sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "spawned-researcher-000f",
            crate::background::registry::SessionState::Idle,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        // A resume point existing here would be wrong to use — a busy
        // channel must error, never fall through to a resume.
        registry.record_resume_point(
            &info.address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        for _ in 0..crate::background::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(
                messenger
                    .send(
                        info.address.as_ref(),
                        SessionAddress::from(MAIN_ADDRESS),
                        "main".to_string(),
                        "filler".to_string(),
                        0,
                    )
                    .await
                    .is_ok()
            );
        }

        let err = messenger
            .send(
                info.address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "one more?".to_string(),
                0,
            )
            .await
            .expect_err("a saturated channel must error rather than silently resume");
        assert!(matches!(&err, SendError::Busy(addr) if *addr == info.address));
        assert!(
            err.to_string().contains("busy"),
            "error message should be actionable: {err}"
        );

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), spawn_sub.recv())
                .await
                .is_err(),
            "a busy channel must never trigger a duplicate resume spawn"
        );
    }

    #[tokio::test]
    async fn send_to_a_completing_session_returns_immediately_and_resumes_once_it_clears() {
        // The whole point of the non-blocking redesign: `send` must not
        // await the completing run's own teardown (which can include a slow
        // memory-merge LLM call) — it hands the wait off to a detached task
        // and returns right away.
        let (messenger, registry, bus_handle) = messenger();
        let mut spawn_sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "spawned-researcher-000c",
            crate::background::registry::SessionState::Completing,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry.record_resume_point(
            &info.address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        let address = info.address.clone();
        let run_id = info.run_id.clone();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            messenger.send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "any updates?".to_string(),
                0,
            ),
        )
        .await
        .expect("send must return promptly for a completing target, not block on its teardown")
        .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Queued(addr) if addr == address));

        // Nothing should be published yet: the deferred task is still
        // waiting for the entry to clear.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), spawn_sub.recv())
                .await
                .is_err(),
            "the resume must wait for the completing run to clear before publishing"
        );

        registry.remove(&address, &run_id);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), spawn_sub.recv())
            .await
            .expect("the deferred resume should publish once the entry clears")
            .unwrap()
            .unwrap();
        assert_eq!(event.address, address);
        assert!(event.prompt.contains("any updates?"));
        assert_eq!(event.model_tier, crate::config::BackgroundModelTier::Large);
    }

    #[tokio::test]
    async fn resume_or_deliver_after_clear_delivers_into_a_run_that_already_won_the_race() {
        // The exact "loop-defeat" scenario item 3 exists to prevent: two
        // messages queued to one completing session must never produce two
        // runs. `wait_until_clear` only tells the caller the address *was*
        // empty; a completely separate run may already be live there by the
        // time this decision actually runs (e.g. another deferred message
        // won the race and got itself resumed first) — this must deliver
        // into that live run rather than blindly publishing a second resume.
        let bus_handle = crate::bus::spawn_broker();
        let mut spawn_sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let publisher = bus_handle.publisher();

        let winner = sample_live_info(
            "spawned-researcher-race1",
            crate::background::registry::SessionState::Idle,
        );
        let mut winner_rx = registry.register(winner, CancellationToken::new()).unwrap();

        let msg = AgentMessageEvent {
            from: SessionAddress::from(MAIN_ADDRESS),
            from_category: "main".to_string(),
            content: "second message".to_string(),
            hop_count: 0,
        };
        let done = resume_or_deliver_after_clear(
            &registry,
            &publisher,
            &store,
            &SessionAddress::from("spawned-researcher-race1"),
            &msg,
        )
        .await;
        assert!(done, "delivering into a live run is a final outcome");

        let delivered = winner_rx
            .try_recv()
            .expect("the message should be delivered into the run that already won");
        match delivered {
            Interrupt::AgentMessage(m) => assert!(m.content.contains("second message")),
            Interrupt::UserMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!("expected an agent message delivered into the winning run")
            }
        }

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), spawn_sub.recv())
                .await
                .is_err(),
            "no resume should ever be published once the address is already live"
        );
    }

    #[tokio::test]
    async fn resume_or_deliver_after_clear_resumes_when_the_address_is_genuinely_free() {
        let bus_handle = crate::bus::spawn_broker();
        let mut spawn_sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let publisher = bus_handle.publisher();
        let address = SessionAddress::from("spawned-researcher-race2");
        registry.record_resume_point(
            &address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        let msg = AgentMessageEvent {
            from: SessionAddress::from(MAIN_ADDRESS),
            from_category: "main".to_string(),
            content: "still there?".to_string(),
            hop_count: 0,
        };
        let done =
            resume_or_deliver_after_clear(&registry, &publisher, &store, &address, &msg).await;
        assert!(done, "resuming is a final outcome");

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), spawn_sub.recv())
            .await
            .expect("a genuinely free address must be resumed")
            .unwrap()
            .unwrap();
        assert_eq!(event.address, address);
        assert!(event.prompt.contains("still there?"));
    }

    #[tokio::test]
    async fn send_to_completed_session_resumes_via_spawn_request() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0002");
        registry.record_resume_point(
            &address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        let outcome = messenger
            .send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "any updates?".to_string(),
                3,
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Resumed(addr) if addr == address));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        assert_eq!(event.skill.as_ref().map(AsRef::as_ref), Some("researcher"));
        assert!(event.prompt.contains("any updates?"));
        assert_eq!(
            event.model_tier,
            crate::config::BackgroundModelTier::Large,
            "a resumed run must carry the previous run's model tier, not default to Medium"
        );
        assert_eq!(
            event.spawner,
            Some(SessionAddress::from(MAIN_ADDRESS)),
            "a resumed run must keep its original spawner, not lose it"
        );
        assert_eq!(
            event.depth, 1,
            "a resumed run must keep its original depth, not reset to a fresh depth-1 session"
        );
        assert_eq!(
            event.hop_count, 3,
            "the resumed run's hop count must be the delivered message's own hop count"
        );
        let context = event.context.expect("resume should carry a pointer note");
        assert!(context.contains("ep-42"));
        assert!(context.contains("memory_get"));
    }

    #[tokio::test]
    async fn send_to_completed_session_without_episode_points_at_run_id() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0003");
        registry.record_resume_point(
            &address,
            ResumePoint {
                previous_run_id: "run-quiet".to_string(),
                previous_episode_id: None,
                trigger: EventTrigger::Agent,
                source_label: "agent:researcher".to_string(),
                agent_skill: None,
                model_tier: crate::config::BackgroundModelTier::Small,
                spawner: None,
                depth: 1,
                conversation_target: None,
            },
        );

        messenger
            .send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "hi".to_string(),
                0,
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        let context = event.context.unwrap();
        assert!(context.contains("run-quiet"));
        assert!(context.contains("produced no episode"));
    }

    #[tokio::test]
    async fn resume_with_pending_combines_all_drained_agent_messages_into_one_prompt() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0004");
        let point = sample_resume_point("run-old", crate::config::BackgroundModelTier::Small);
        registry.record_resume_point(&address, point.clone());

        let messages = vec![
            PendingInput::Agent(AgentMessageEvent {
                from: SessionAddress::from(MAIN_ADDRESS),
                from_category: "main".to_string(),
                content: "first".to_string(),
                hop_count: 1,
            }),
            PendingInput::Agent(AgentMessageEvent {
                from: SessionAddress::from("spawned-other-0001"),
                from_category: "spawned".to_string(),
                content: "second".to_string(),
                hop_count: 5,
            }),
        ];

        messenger
            .resume_with_pending(&address, &point, &messages)
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.prompt.contains("first"));
        assert!(event.prompt.contains("second"));
        assert!(event.prompt.contains("main"));
        assert!(event.prompt.contains("spawned-other-0001"));
        assert_eq!(
            event.hop_count, 5,
            "the resumed run's hop count must be the highest among the combined messages"
        );
    }

    fn sample_inbound(content: &str, buffered: Option<&str>) -> InboundMessage {
        InboundMessage {
            id: "conv-1".to_string(),
            content: content.to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: Some(crate::inference::MessageSender {
                    name: "Jane".to_string(),
                    id: "discord-jane".to_string(),
                    interface: "discord".to_string(),
                    location: Some("#builds".to_string()),
                }),
                conversation: Some(crate::interfaces::types::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: crate::interfaces::types::ConversationKind::Channel,
                    is_owner: false,
                }),
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: buffered.map(str::to_string),
        }
    }

    fn conversation_spawn() -> ConversationSpawn {
        ConversationSpawn {
            source_label: "discord:#builds".to_string(),
            model_tier: crate::config::BackgroundModelTier::Medium,
        }
    }

    #[tokio::test]
    async fn resume_with_pending_combines_agent_and_conversation_messages() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("external-discord-0004");
        let point = sample_resume_point("run-old", crate::config::BackgroundModelTier::Small);
        registry.record_resume_point(&address, point.clone());

        let pending = vec![
            PendingInput::Agent(AgentMessageEvent {
                from: SessionAddress::from(MAIN_ADDRESS),
                from_category: "main".to_string(),
                content: "checking in".to_string(),
                hop_count: 2,
            }),
            PendingInput::External(sample_inbound("any updates?", None)),
        ];

        messenger
            .resume_with_pending(&address, &point, &pending)
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.prompt.contains("checking in"));
        assert!(event.prompt.contains("any updates?"));
        assert!(event.prompt.contains("Jane"), "{}", event.prompt);
        assert_eq!(
            event.hop_count, 2,
            "the agent message's hop count is higher than the conversation message's (always 0)"
        );
    }

    #[tokio::test]
    async fn deliver_conversation_to_a_live_session_delivers_via_registry() {
        let (messenger, registry, _bus_handle) = messenger();
        let info = sample_live_info(
            "external-discord-0001",
            crate::background::registry::SessionState::Idle,
        );
        let mut rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let outcome = messenger
            .deliver_conversation(
                &info.address,
                sample_inbound("hi again", None),
                conversation_spawn(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ConversationDeliveryOutcome::Live(addr) if addr == info.address));

        let received = rx.try_recv().expect("message should be queued");
        assert!(matches!(received, Interrupt::UserMessage(_)));
    }

    #[tokio::test]
    async fn deliver_conversation_with_no_prior_run_starts_a_fresh_session() {
        let (messenger, _registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("external-discord-brand-new");
        let outcome = messenger
            .deliver_conversation(
                &address,
                sample_inbound(
                    "can you check the build?",
                    Some("[14:00] Sam: build is red"),
                ),
                conversation_spawn(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ConversationDeliveryOutcome::Started(addr) if addr == address));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        assert_eq!(event.prompt, "can you check the build?");
        assert_eq!(event.context.as_deref(), Some("[14:00] Sam: build is red"));
        assert_eq!(event.hop_count, 0);
        assert!(matches!(event.source, EventTrigger::Conversation));
        assert_eq!(event.spawner, None);
        assert_eq!(event.sender.as_ref().map(|s| s.name.as_str()), Some("Jane"));
        let target = event.conversation.expect("conversation target must be set");
        assert_eq!(target.endpoint, "discord");
        assert_eq!(target.conversation_id, "chan-1");
    }

    #[tokio::test]
    async fn deliver_conversation_to_a_completed_session_resumes_it() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("external-discord-0002");
        let mut point = sample_resume_point("run-old", crate::config::BackgroundModelTier::Large);
        point.conversation_target = Some(crate::bus::ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "chan-1".to_string(),
        });
        registry.record_resume_point(&address, point);

        let outcome = messenger
            .deliver_conversation(
                &address,
                sample_inbound("still there?", None),
                conversation_spawn(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ConversationDeliveryOutcome::Resumed(addr) if addr == address));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        assert!(event.prompt.contains("still there?"));
        assert_eq!(
            event.model_tier,
            crate::config::BackgroundModelTier::Large,
            "a resumed conversation session must keep its previous model tier"
        );
        let target = event
            .conversation
            .expect("conversation target must carry over");
        assert_eq!(target.conversation_id, "chan-1");
    }

    #[tokio::test]
    async fn deliver_conversation_to_a_completing_session_queues_and_resumes_once_clear() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "external-discord-0003",
            crate::background::registry::SessionState::Completing,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry.record_resume_point(
            &info.address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        let address = info.address.clone();
        let run_id = info.run_id.clone();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            messenger.deliver_conversation(
                &address,
                sample_inbound("any updates?", None),
                conversation_spawn(),
            ),
        )
        .await
        .expect("deliver_conversation must return promptly for a completing target")
        .unwrap();
        assert!(matches!(outcome, ConversationDeliveryOutcome::Queued(addr) if addr == address));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv())
                .await
                .is_err(),
            "the resume must wait for the completing run to clear before publishing"
        );

        registry.remove(&address, &run_id);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("the deferred resume should publish once the entry clears")
            .unwrap()
            .unwrap();
        assert_eq!(event.address, address);
        assert!(event.prompt.contains("any updates?"));
    }

    #[tokio::test]
    async fn publish_failure_is_reported_as_an_error_not_success() {
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger =
            AgentMessenger::new(Arc::clone(&registry), Publisher::noop(), store, NO_LIMIT);

        let err = messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-0005"),
                "spawned".to_string(),
                "hello".to_string(),
                0,
            )
            .await
            .expect_err("a noop publisher must surface as a delivery failure");
        assert!(matches!(err, SendError::PublishFailed(_)));
    }

    #[tokio::test]
    async fn deliver_to_main_does_not_leak_a_pending_hop_entry_when_publish_fails() {
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger = AgentMessenger::new(registry, Publisher::noop(), store, NO_LIMIT);

        messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-leak"),
                "spawned".to_string(),
                "hello".to_string(),
                3,
            )
            .await
            .expect_err("a noop publisher must surface as a delivery failure");

        assert!(
            messenger.pending_main_hops.lock().unwrap().is_empty(),
            "a failed publish must not leave a stale hop-count entry behind for an id \
             main will never actually receive and look up"
        );
    }

    #[tokio::test]
    async fn hop_count_at_or_above_soft_limit_appends_a_reply_only_if_needed_note() {
        let (messenger, _registry, bus_handle) =
            messenger_with_limits(HopLimits { soft: 5, hard: 32 });
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-0006"),
                "spawned".to_string(),
                "still going".to_string(),
                5,
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(
            event.content.contains("Reply only if"),
            "content should carry the soft-limit note, got: {}",
            event.content
        );
    }

    #[tokio::test]
    async fn hop_count_below_soft_limit_carries_no_note() {
        let (messenger, _registry, bus_handle) =
            messenger_with_limits(HopLimits { soft: 5, hard: 32 });
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-0007"),
                "spawned".to_string(),
                "just starting".to_string(),
                1,
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(!event.content.contains("Reply only if"));
    }

    #[tokio::test]
    async fn hop_count_at_hard_limit_is_refused_and_logged() {
        let (messenger, _registry, bus_handle) =
            messenger_with_limits(HopLimits { soft: 5, hard: 10 });
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        let err = messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-0008"),
                "spawned".to_string(),
                "are we there yet".to_string(),
                10,
            )
            .await
            .expect_err("hop count at the hard limit must be refused");
        assert!(matches!(
            err,
            SendError::HopLimitExceeded {
                hop_count: 10,
                limit: 10
            }
        ));
        assert!(err.to_string().contains("loop"));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv())
                .await
                .is_err(),
            "a refused message must never actually reach the target"
        );
    }

    #[tokio::test]
    async fn hop_limit_refusal_records_a_note_in_a_live_sender_and_receiver_transcript() {
        let registry = Arc::new(SessionRegistry::new());
        let sender_info = sample_live_info(
            "spawned-sender-0001",
            crate::background::registry::SessionState::Running,
        );
        let receiver_info = sample_live_info(
            "spawned-receiver-0001",
            crate::background::registry::SessionState::Idle,
        );
        let _sender_rx = registry
            .register(sender_info.clone(), CancellationToken::new())
            .unwrap();
        let _receiver_rx = registry
            .register(receiver_info.clone(), CancellationToken::new())
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger = AgentMessenger::new(
            Arc::clone(&registry),
            crate::bus::Publisher::noop(),
            Arc::clone(&store),
            HopLimits { soft: 5, hard: 3 },
        );
        let outcome = messenger
            .send(
                receiver_info.address.as_ref(),
                sender_info.address.clone(),
                "spawned".to_string(),
                "loop!".to_string(),
                3,
            )
            .await;
        assert!(
            matches!(outcome, Err(SendError::HopLimitExceeded { .. })),
            "the send itself must still report the refusal"
        );

        let sender_transcript = store
            .read_incremental_transcript(&sender_info.run_id, sender_info.started_at)
            .await;
        assert!(
            sender_transcript
                .iter()
                .any(|m| m.content.contains("Message Loop Limit")),
            "sender transcript should record the refusal, got {sender_transcript:?}"
        );

        let receiver_transcript = store
            .read_incremental_transcript(&receiver_info.run_id, receiver_info.started_at)
            .await;
        assert!(
            receiver_transcript
                .iter()
                .any(|m| m.content.contains("Message Loop Limit")),
            "receiver transcript should record the refusal, got {receiver_transcript:?}"
        );
    }

    #[tokio::test]
    async fn hop_limit_refusal_is_an_error_event_on_both_live_sessions() {
        let bus_handle = crate::bus::spawn_broker();
        let mut events: crate::bus::Subscriber<crate::bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let sender_info = sample_live_info(
            "spawned-sender-0002",
            crate::background::registry::SessionState::Running,
        );
        let receiver_info = sample_live_info(
            "spawned-receiver-0002",
            crate::background::registry::SessionState::Idle,
        );
        let _sender_rx = registry
            .register(sender_info.clone(), CancellationToken::new())
            .unwrap();
        let _receiver_rx = registry
            .register(receiver_info.clone(), CancellationToken::new())
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let messenger = AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::new(SessionStore::new(dir.path().to_path_buf())),
            HopLimits { soft: 5, hard: 3 },
        );

        let outcome = messenger
            .send(
                receiver_info.address.as_ref(),
                sender_info.address.clone(),
                "spawned".to_string(),
                "loop!".to_string(),
                3,
            )
            .await;
        assert!(matches!(outcome, Err(SendError::HopLimitExceeded { .. })));

        let mut tagged = Vec::new();
        for _ in 0..2 {
            let event = events.recv().await.unwrap().unwrap();
            assert!(
                matches!(&event.kind, SessionEventKind::Error { message } if message.contains("Message Loop Limit")),
                "a refusal must surface as an error event, got {:?}",
                event.kind
            );
            tagged.push((event.address.to_string(), event.run_id));
        }
        assert!(
            tagged.contains(&(sender_info.address.to_string(), sender_info.run_id.clone())),
            "the sender's stream must show the refusal, got {tagged:?}"
        );
        assert!(
            tagged.contains(&(
                receiver_info.address.to_string(),
                receiver_info.run_id.clone()
            )),
            "the receiver's stream must show the refusal, got {tagged:?}"
        );
    }
}

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::team::TeamLink;
use residuum_agent::hop::HopLimits;
use residuum_bus::interrupt::Interrupt;
use residuum_bus::{
    AgentMessageEvent, MessageEvent, Publisher, SessionAddress, SessionEventKind, SkillName,
    SpawnRequestEvent, topics,
};
use residuum_config::ModelSelector;
use residuum_inference::{ImageData, Message};

use super::store::SessionStore;
use crate::registry::{DeliverOutcome, ResumePoint, SessionRegistry};
use residuum_bus::session::publish_session_event;
use residuum_bus::session::{MAIN_ADDRESS, TEAMMATE_SENDER_CATEGORY};

#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    Main,
    Live(SessionAddress),
    Resumed(SessionAddress),
    Queued(SessionAddress),
    Unknown,
}

#[derive(Debug, Clone)]
pub enum SendError {
    Busy(SessionAddress),
    PublishFailed(String),
    HopLimitExceeded { hop_count: u32, limit: u32 },
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
                 report back to your spawner or the user instead. Raise the `hop_hard_limit` \
                 setting under `[background]` in config.toml to allow more hops"
            ),
        }
    }
}

impl std::error::Error for SendError {}

pub struct AgentMessenger {
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    store: Arc<SessionStore>,
    hop_limits: HopLimits,
    team: TeamLink,
    pending_main_hops: Mutex<HashMap<String, u32>>,
    mid_turn_hops: Mutex<HashMap<String, u32>>,
}

impl AgentMessenger {
    #[must_use]
    pub fn new(
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
            team: TeamLink::alone("agent"),
            pending_main_hops: Mutex::new(HashMap::new()),
            mid_turn_hops: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn with_team(mut self, team: TeamLink) -> Self {
        self.team = team;
        self
    }

    #[must_use]
    pub fn team(&self) -> &TeamLink {
        &self.team
    }

    #[must_use]
    pub fn publisher(&self) -> &Publisher {
        &self.publisher
    }

    pub(crate) fn store(&self) -> &Arc<SessionStore> {
        &self.store
    }

    #[must_use]
    pub fn take_main_hop(&self, message_id: &str) -> u32 {
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(message_id)
            .unwrap_or(0)
    }

    pub fn restore_main_hop(&self, message_id: &str, hop_count: u32) {
        if hop_count == 0 {
            return;
        }
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(message_id.to_string(), hop_count);
    }

    pub fn note_mid_turn_hop(&self, message_id: &str, hop_count: u32) {
        if hop_count == 0 {
            return;
        }
        self.mid_turn_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(message_id.to_string(), hop_count);
    }

    #[must_use]
    pub fn mid_turn_hop(&self, message_id: &str) -> u32 {
        self.mid_turn_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(message_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn clear_mid_turn_hops(&self) {
        self.mid_turn_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn forget_main_hop(&self, message_id: &str) {
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(message_id);
    }

    pub async fn send(
        &self,
        to: &str,
        mut message: AgentMessageEvent,
    ) -> Result<DeliveryOutcome, SendError> {
        let hop_count = message.hop_count;
        if hop_count >= self.hop_limits.hard {
            self.refuse_hop_limit(&message.from, to, hop_count).await;
            return Err(SendError::HopLimitExceeded {
                hop_count,
                limit: self.hop_limits.hard,
            });
        }
        message.content = self.apply_soft_note(hop_count, std::mem::take(&mut message.content));

        if to == MAIN_ADDRESS {
            let AgentMessageEvent {
                from,
                from_category,
                content,
                ..
            } = message;
            self.deliver_to_main(from, from_category, content, hop_count)
                .await?;
            return Ok(DeliveryOutcome::Main);
        }

        let address = SessionAddress::from(to);
        match self
            .registry
            .deliver(&address, Interrupt::AgentMessage(message.clone()))
        {
            DeliverOutcome::Delivered => return Ok(DeliveryOutcome::Live(address)),
            DeliverOutcome::Full => return Err(SendError::Busy(address)),
            DeliverOutcome::Completing => {
                let registry = Arc::clone(&self.registry);
                let publisher = self.publisher.clone();
                let store = Arc::clone(&self.store);
                let deferred_address = address.clone();
                residuum_core::util::spawn_in_span(async move {
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

    async fn refuse_hop_limit(&self, from: &SessionAddress, to: &str, hop_count: u32) {
        tracing::warn!(
            from = %from,
            to = %to,
            hop_count,
            limit = self.hop_limits.hard,
            "refusing to deliver agent message: hop limit reached, likely a message loop"
        );
        let note = self.hop_refusal_note(from, to, hop_count);
        let sinks = self.note_sinks();
        record_note_if_live_session(sinks, from, &note).await;
        record_note_if_live_session(sinks, &SessionAddress::from(to), &note).await;
    }

    fn hop_refusal_note(&self, from: &SessionAddress, to: &str, hop_count: u32) -> String {
        format!(
            "[Message Loop Limit] a message from {from} to {to} reached the hop limit \
             ({hop_count} >= {}); it was not delivered.",
            self.hop_limits.hard
        )
    }

    fn note_sinks(&self) -> NoteSinks<'_> {
        NoteSinks {
            registry: &self.registry,
            store: &self.store,
            publisher: &self.publisher,
        }
    }

    pub(crate) async fn note_refused_send(&self, from: &SessionAddress, to: &str, hop_count: u32) {
        let note = self.hop_refusal_note(from, to, hop_count);
        record_note_if_live_session(self.note_sinks(), from, &note).await;
    }

    async fn deliver_to_main(
        &self,
        from: SessionAddress,
        from_category: String,
        content: String,
        hop_count: u32,
    ) -> Result<(), SendError> {
        let sender_is_teammate = from_category == TEAMMATE_SENDER_CATEGORY;
        let msg = AgentMessageEvent::new(from, from_category, content, hop_count);
        let mut event = MessageEvent::from_agent(&msg);
        if !sender_is_teammate && let Some(sender) = event.origin.agent_sender.as_deref_mut() {
            sender.run_id = self.sending_run_id(&msg.from);
        }
        let message_id = event.id.clone();
        self.pending_main_hops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(message_id.clone(), hop_count);
        if let Err(e) = self.publisher.publish(topics::UserMessage, event).await {
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

    fn sending_run_id(&self, sender: &SessionAddress) -> Option<String> {
        let run_id = self.registry.get(sender).map(|info| info.run_id);
        if run_id.is_none() {
            tracing::warn!(
                sender = %sender,
                "message to main came from an address with no live run; delivering it without a run id"
            );
        }
        run_id
    }

    async fn resume(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        msg: AgentMessageEvent,
    ) -> Result<(), SendError> {
        publish_resume(&self.publisher, address, point, ResumeOpening::agent(msg)).await
    }

    pub(crate) async fn resume_with_pending(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        pending: &[PendingInput],
    ) -> Result<(), SendError> {
        let combined = pending
            .iter()
            .filter_map(PendingInput::conversation_text_for_resume)
            .collect::<Vec<_>>()
            .join("\n\n");
        let hop_count = pending
            .iter()
            .map(PendingInput::hop_count)
            .max()
            .unwrap_or(0);
        let images = pending.iter().flat_map(PendingInput::images).collect();
        let mut agent_messages = Vec::new();
        let mut chat_messages = Vec::new();
        for input in pending {
            match input {
                PendingInput::Agent(msg) => agent_messages.push(msg.clone()),
                PendingInput::External(msg) => chat_messages.push(msg.clone()),
            }
        }
        let opening = ResumeOpening {
            prompt: combined,
            hop_count,
            images,
            agent_messages,
            chat_messages,
        };
        publish_resume(&self.publisher, address, point, opening).await
    }

    pub async fn deliver_conversation(
        &self,
        address: &SessionAddress,
        inbound: MessageEvent,
        spawn: ConversationSpawn,
    ) -> Result<ConversationDeliveryOutcome, SendError> {
        match self
            .registry
            .deliver(address, Interrupt::UserMessage(inbound.clone()))
        {
            DeliverOutcome::Delivered => {
                return Ok(ConversationDeliveryOutcome::Live(address.clone()));
            }
            DeliverOutcome::Full | DeliverOutcome::Completing => {
                let registry = Arc::clone(&self.registry);
                let publisher = self.publisher.clone();
                let deferred_address = address.clone();
                residuum_core::util::spawn_in_span(async move {
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

#[derive(Debug, Clone)]
pub(crate) enum PendingInput {
    Agent(AgentMessageEvent),
    External(MessageEvent),
}

impl PendingInput {
    fn hop_count(&self) -> u32 {
        match self {
            Self::Agent(m) => m.hop_count,
            Self::External(_) => 0,
        }
    }

    fn images(&self) -> Vec<ImageData> {
        match self {
            Self::Agent(_) => Vec::new(),
            Self::External(m) => m.images.clone(),
        }
    }

    fn conversation_text_for_resume(&self) -> Option<String> {
        let Self::External(m) = self else {
            return None;
        };
        let mut parts = Vec::new();
        if let Some(ctx) = &m.context {
            parts.push(ctx.clone());
        }
        let attributed = Message::user(m.content.clone())
            .with_sender(m.origin.sender.clone())
            .attributed_content()
            .into_owned();
        parts.push(attributed);
        Some(parts.join("\n\n"))
    }
}

#[derive(Debug, Clone)]
pub struct ConversationSpawn {
    pub source_label: String,
    pub model: ModelSelector,
    pub skill: Option<SkillName>,
}

#[derive(Debug, Clone)]
pub enum ConversationDeliveryOutcome {
    Live(SessionAddress),
    Started(SessionAddress),
    Resumed(SessionAddress),
    Queued(SessionAddress),
}

async fn publish_conversation_spawn(
    publisher: &Publisher,
    address: SessionAddress,
    inbound: MessageEvent,
    spawn: ConversationSpawn,
) -> Result<(), SendError> {
    let Some(conversation) = inbound.origin.conversation.as_ref() else {
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
    let event = residuum_bus::SpawnRequestEvent {
        address,
        skill: spawn.skill,
        source_label: spawn.source_label,
        prompt: inbound.content,
        context: inbound.context,
        source: residuum_bus::EventTrigger::Conversation,
        model: spawn.model,
        spawner: None,
        depth: residuum_bus::session::MAIN_DEPTH + 1,
        hop_count: 0,
        sender: inbound.origin.sender,
        conversation: Some(residuum_bus::ConversationTarget {
            endpoint: inbound.origin.endpoint,
            conversation_id,
        }),
        images: original_inbound.images.clone(),
        inbound: Some(original_inbound),
        overlap: None,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: Vec::new(),
        delivered_chat_messages: Vec::new(),
    };
    publisher
        .publish(topics::Background, event)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to publish conversation spawn request");
            SendError::PublishFailed("failed to start a conversation session".to_string())
        })
}

async fn publish_conversation_resume(
    publisher: &Publisher,
    address: &SessionAddress,
    point: &ResumePoint,
    inbound: &MessageEvent,
) -> Result<(), SendError> {
    let context = match &inbound.context {
        Some(ctx) => format!("{}\n\n{ctx}", pointer_note(point)),
        None => pointer_note(point),
    };
    let event = residuum_bus::SpawnRequestEvent {
        address: address.clone(),
        skill: point.agent_skill.clone(),
        source_label: point.source_label.clone(),
        prompt: inbound.content.clone(),
        context: Some(context),
        source: point.trigger.clone(),
        model: point.model.clone(),
        spawner: point.spawner.clone(),
        depth: point.depth,
        hop_count: 0,
        sender: inbound.origin.sender.clone(),
        conversation: point.conversation_target.clone(),
        images: inbound.images.clone(),
        inbound: Some(inbound.clone()),
        overlap: None,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: Vec::new(),
        delivered_chat_messages: Vec::new(),
    };
    publisher.publish(topics::Background, event).await.map_err(|e| {
        tracing::error!(error = %e, address = %address, "failed to publish conversation resume");
        SendError::PublishFailed(format!("failed to resume conversation session {address}"))
    })
}

pub(crate) const USER_MESSAGE_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(100);

pub(crate) const USER_MESSAGE_WARN_AFTER_ATTEMPTS: u32 = 20;

pub(crate) async fn retry_race_guard_user_message(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    address: SessionAddress,
    inbound: MessageEvent,
    spawn_event: SpawnRequestEvent,
) {
    let mut attempts: u32 = 0;
    loop {
        match registry.deliver(&address, Interrupt::UserMessage(inbound.clone())) {
            DeliverOutcome::Delivered => return,
            DeliverOutcome::Full => {
                attempts += 1;
                if attempts == USER_MESSAGE_WARN_AFTER_ATTEMPTS {
                    tracing::warn!(
                        address = %address,
                        attempts,
                        "race-guard user message still waiting on a saturated interrupt channel"
                    );
                }
                tokio::time::sleep(USER_MESSAGE_RETRY_DELAY).await;
            }
            DeliverOutcome::Completing => {
                registry.wait_until_clear(&address).await;
            }
            DeliverOutcome::NotLive => {
                if let Err(e) = publisher.publish(topics::Background, spawn_event).await {
                    tracing::error!(
                        error = %e,
                        address = %address,
                        "failed to republish a race-guard user message after its target \
                         finished; input dropped"
                    );
                }
                return;
            }
        }
    }
}

pub(crate) fn deliver_chat_messages_in_order(
    registry: &Arc<SessionRegistry>,
    publisher: &Publisher,
    address: &SessionAddress,
    chat_messages: Vec<MessageEvent>,
    spawn: ConversationSpawn,
) {
    let mut chat_messages = chat_messages.into_iter();
    while let Some(msg) = chat_messages.next() {
        let outcome = registry.deliver(address, Interrupt::UserMessage(msg.clone()));
        if matches!(outcome, DeliverOutcome::Delivered) {
            continue;
        }
        let undelivered: Vec<MessageEvent> = std::iter::once(msg).chain(chat_messages).collect();
        tracing::warn!(
            address = %address,
            ?outcome,
            count = undelivered.len(),
            "a losing resume's chat message(s) could not be delivered immediately; delivering \
             them on a detached task rather than dropping them"
        );
        let registry = Arc::clone(registry);
        let publisher = publisher.clone();
        let address = address.clone();
        residuum_core::util::spawn_in_span(async move {
            deferred_conversation_resumes(registry, publisher, address, undelivered, spawn).await;
        });
        return;
    }
}

pub(crate) fn defer_losing_resume(
    registry: &Arc<SessionRegistry>,
    publisher: &Publisher,
    store: &Arc<SessionStore>,
    address: &SessionAddress,
    agent_messages: Vec<AgentMessageEvent>,
    chat_messages: Vec<MessageEvent>,
    spawn: ConversationSpawn,
) {
    let registry = Arc::clone(registry);
    let publisher = publisher.clone();
    let store = Arc::clone(store);
    let address = address.clone();
    residuum_core::util::spawn_in_span(async move {
        for msg in agent_messages {
            deferred_resume(
                Arc::clone(&registry),
                publisher.clone(),
                Arc::clone(&store),
                address.clone(),
                msg,
            )
            .await;
        }
        deferred_conversation_resumes(registry, publisher, address, chat_messages, spawn).await;
    });
}

async fn deferred_conversation_resumes(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    address: SessionAddress,
    chat_messages: Vec<MessageEvent>,
    spawn: ConversationSpawn,
) {
    for chat_message in chat_messages {
        deferred_conversation_resume(
            Arc::clone(&registry),
            publisher.clone(),
            address.clone(),
            chat_message,
            spawn.clone(),
        )
        .await;
    }
}

async fn deferred_conversation_resume(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    address: SessionAddress,
    inbound: MessageEvent,
    spawn: ConversationSpawn,
) {
    loop {
        let done = resume_or_start_conversation_after_clear(
            &registry, &publisher, &address, &inbound, &spawn,
        )
        .await;
        if done {
            return;
        }
        registry.wait_until_clear(&address).await;
    }
}

async fn resume_or_start_conversation_after_clear(
    registry: &SessionRegistry,
    publisher: &Publisher,
    address: &SessionAddress,
    inbound: &MessageEvent,
    spawn: &ConversationSpawn,
) -> bool {
    let mut attempts: u32 = 0;
    loop {
        match registry.deliver(address, Interrupt::UserMessage(inbound.clone())) {
            DeliverOutcome::Delivered => return true,
            DeliverOutcome::Completing => return false,
            DeliverOutcome::Full => {
                attempts += 1;
                if attempts == USER_MESSAGE_WARN_AFTER_ATTEMPTS {
                    tracing::warn!(
                        address = %address,
                        attempts,
                        "user message still waiting on a saturated interrupt channel"
                    );
                }
                tokio::time::sleep(USER_MESSAGE_RETRY_DELAY).await;
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
                return true;
            }
        }
    }
}

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
                details: None,
            },
        )
        .await;
    }
}

#[derive(Clone, Copy)]
struct NoteSinks<'a> {
    registry: &'a SessionRegistry,
    store: &'a SessionStore,
    publisher: &'a Publisher,
}

async fn deferred_resume(
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
    store: Arc<SessionStore>,
    address: SessionAddress,
    msg: AgentMessageEvent,
) {
    loop {
        let done = deliver_or_resume_deferred(&registry, &publisher, &store, &address, &msg).await;
        if done {
            return;
        }
        registry.wait_until_clear(&address).await;
    }
}

async fn deliver_or_resume_deferred(
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
            if let Err(e) = publish_resume(
                publisher,
                address,
                &point,
                ResumeOpening::agent(msg.clone()),
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

struct ResumeOpening {
    prompt: String,
    hop_count: u32,
    images: Vec<ImageData>,
    agent_messages: Vec<AgentMessageEvent>,
    chat_messages: Vec<MessageEvent>,
}

impl ResumeOpening {
    fn agent(msg: AgentMessageEvent) -> Self {
        Self {
            prompt: String::new(),
            hop_count: msg.hop_count,
            images: Vec::new(),
            agent_messages: vec![msg],
            chat_messages: Vec::new(),
        }
    }
}

async fn publish_resume(
    publisher: &Publisher,
    address: &SessionAddress,
    point: &ResumePoint,
    opening: ResumeOpening,
) -> Result<(), SendError> {
    let ResumeOpening {
        prompt,
        hop_count,
        images,
        agent_messages,
        chat_messages,
    } = opening;
    let event = residuum_bus::SpawnRequestEvent {
        address: address.clone(),
        skill: point.agent_skill.clone(),
        source_label: point.source_label.clone(),
        prompt,
        context: Some(pointer_note(point)),
        source: point.trigger.clone(),
        model: point.model.clone(),
        spawner: point.spawner.clone(),
        depth: point.depth,
        hop_count,
        sender: None,
        conversation: point.conversation_target.clone(),
        inbound: None,
        images,
        overlap: None,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: agent_messages,
        delivered_chat_messages: chat_messages,
    };

    publisher
        .publish(topics::Background, event)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, address = %address, "failed to publish resume spawn request");
            SendError::PublishFailed(format!("failed to resume session {address}"))
        })
}

fn pointer_note(point: &ResumePoint) -> String {
    format!(
        "[Resumed session] Your previous run ({}) has ended. Retrieve its transcript \
         with memory_get using that run id if useful.",
        point.previous_run_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_bus::{EventTrigger, SkillName, Subscriber};
    use residuum_core::testing::wait;
    use tokio_util::sync::CancellationToken;

    const NO_LIMIT: HopLimits = HopLimits { soft: 8, hard: 32 };

    fn messenger() -> (
        AgentMessenger,
        Arc<SessionRegistry>,
        residuum_bus::BusHandle,
    ) {
        messenger_with_limits(NO_LIMIT)
    }

    fn messenger_with_limits(
        limits: HopLimits,
    ) -> (
        AgentMessenger,
        Arc<SessionRegistry>,
        residuum_bus::BusHandle,
    ) {
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger =
            AgentMessenger::new(Arc::clone(&registry), bus_handle.publisher(), store, limits);
        (messenger, registry, bus_handle)
    }

    fn sample_resume_point(run_id: &str, model: residuum_config::ModelSelector) -> ResumePoint {
        ResumePoint {
            previous_run_id: run_id.to_string(),
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            agent_skill: Some(SkillName::from("researcher")),
            model,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            conversation_target: None,
            recorded_at: chrono::Utc::now(),
        }
    }

    fn sample_live_info(
        address: &str,
        state: residuum_bus::session::SessionState,
    ) -> residuum_bus::session::SessionInfo {
        residuum_bus::session::SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            category: residuum_bus::session::SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: chrono::Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-3f9a"),
                    "spawned",
                    "found the answer",
                    0,
                ),
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
    async fn send_to_main_names_the_sending_sessions_run_on_the_origin() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();
        let sender = sample_live_info(
            "spawned-researcher-3f9c",
            residuum_bus::session::SessionState::Running,
        );
        let run_id = sender.run_id.clone();
        let _rx = registry.register(sender, CancellationToken::new()).unwrap();

        messenger
            .send(
                MAIN_ADDRESS,
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-3f9c"),
                    "spawned",
                    "found the answer",
                    0,
                ),
            )
            .await
            .unwrap();

        let event = next_event("the message to main", &mut sub).await;
        let expected = residuum_inference::AgentSender {
            address: "spawned-researcher-3f9c".to_string(),
            category: "spawned".to_string(),
            run_id: Some(run_id),
        };
        assert_eq!(event.origin.agent_sender.as_deref(), Some(&expected));
        let history = event.into_history_messages();
        assert_eq!(
            history.last().and_then(|m| m.agent_sender.as_ref()),
            Some(&expected),
            "main's history entry keeps the run too"
        );
    }

    #[tokio::test]
    async fn send_to_main_gives_a_teammate_or_a_sender_with_no_live_run_no_run_id() {
        let (messenger, _registry, bus_handle) = messenger();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        for (from, category) in [
            ("agent:scout", TEAMMATE_SENDER_CATEGORY),
            ("spawned-gone-3f9d", "spawned"),
        ] {
            messenger
                .send(
                    MAIN_ADDRESS,
                    AgentMessageEvent::new(SessionAddress::from(from), category, "status?", 0),
                )
                .await
                .unwrap();
            let event = next_event("the message to main", &mut sub).await;
            let sender = event.origin.agent_sender.as_deref().unwrap();
            assert_eq!(sender.address, from);
            assert_eq!(
                sender.run_id, None,
                "{from} has no run in this agent's registry"
            );
        }
    }

    #[tokio::test]
    async fn take_main_hop_recovers_the_hop_count_and_removes_the_entry() {
        let (messenger, _registry, bus_handle) = messenger();
        let mut sub: Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();

        messenger
            .send(
                MAIN_ADDRESS,
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-3f9b"),
                    "spawned",
                    "an update",
                    4,
                ),
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
            residuum_bus::session::SessionState::Idle,
        );
        let mut rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let outcome = messenger
            .send(
                info.address.as_ref(),
                AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "how's it going?",
                    0,
                ),
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
                AgentMessageEvent::new(SessionAddress::from(MAIN_ADDRESS), "main", "hello?", 0),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Unknown));
    }

    #[tokio::test]
    async fn send_to_a_full_channel_reports_busy_and_never_resumes() {
        let (messenger, registry, bus_handle) = messenger();
        let mut spawn_sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "spawned-researcher-000f",
            residuum_bus::session::SessionState::Idle,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry
            .record_resume_point(
                &info.address,
                sample_resume_point("run-old", residuum_config::ModelSelector::LARGE),
            )
            .await;

        for _ in 0..crate::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(
                messenger
                    .send(
                        info.address.as_ref(),
                        AgentMessageEvent::new(
                            SessionAddress::from(MAIN_ADDRESS),
                            "main",
                            "filler",
                            0,
                        ),
                    )
                    .await
                    .is_ok()
            );
        }

        let err = messenger
            .send(
                info.address.as_ref(),
                AgentMessageEvent::new(SessionAddress::from(MAIN_ADDRESS), "main", "one more?", 0),
            )
            .await
            .expect_err("a saturated channel must error rather than silently resume");
        assert!(matches!(&err, SendError::Busy(addr) if *addr == info.address));
        assert!(
            err.to_string().contains("busy"),
            "error message should be actionable: {err}"
        );

        bus_barrier(&bus_handle).await;
        assert!(
            spawn_sub.drain().is_empty(),
            "a busy channel must never trigger a duplicate resume spawn"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn send_to_a_completing_session_returns_immediately_and_resumes_once_it_clears() {
        let (messenger, registry, bus_handle) = messenger();
        let mut spawn_sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "spawned-researcher-000c",
            residuum_bus::session::SessionState::Completing,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry
            .record_resume_point(
                &info.address,
                sample_resume_point("run-old", residuum_config::ModelSelector::LARGE),
            )
            .await;

        let address = info.address.clone();
        let run_id = info.run_id.clone();

        let outcome = wait::guarded(
            "send to return for a completing target, not block on its teardown",
            messenger.send(
                address.as_ref(),
                AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "any updates?",
                    0,
                ),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Queued(addr) if addr == address));

        assert!(
            residuum_core::testing::clock::within(
                std::time::Duration::from_millis(50),
                spawn_sub.recv()
            )
            .await
            .is_none(),
            "the resume must wait for the completing run to clear before publishing"
        );

        registry.remove(&address, &run_id);

        let event = next_event(
            "the deferred resume to publish once the entry clears",
            &mut spawn_sub,
        )
        .await;
        assert_eq!(event.address, address);
        assert_eq!(
            event
                .delivered_messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["any updates?"]
        );
        assert_eq!(event.prompt, "", "the message is the whole opening input");
        assert_eq!(event.model, residuum_config::ModelSelector::LARGE);
    }

    #[tokio::test]
    async fn deliver_or_resume_deferred_delivers_into_a_run_that_already_won_the_race() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut spawn_sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let publisher = bus_handle.publisher();

        let winner = sample_live_info(
            "spawned-researcher-race1",
            residuum_bus::session::SessionState::Idle,
        );
        let mut winner_rx = registry.register(winner, CancellationToken::new()).unwrap();

        let msg = AgentMessageEvent::new(
            SessionAddress::from(MAIN_ADDRESS),
            "main",
            "second message",
            0,
        );
        let done = deliver_or_resume_deferred(
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

        bus_barrier(&bus_handle).await;
        assert!(
            spawn_sub.drain().is_empty(),
            "no resume should ever be published once the address is already live"
        );
    }

    #[tokio::test]
    async fn deliver_or_resume_deferred_resumes_when_the_address_is_genuinely_free() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut spawn_sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let publisher = bus_handle.publisher();
        let address = SessionAddress::from("spawned-researcher-race2");
        registry
            .record_resume_point(
                &address,
                sample_resume_point("run-old", residuum_config::ModelSelector::LARGE),
            )
            .await;

        let msg = AgentMessageEvent::new(
            SessionAddress::from(MAIN_ADDRESS),
            "main",
            "still there?",
            0,
        );
        let done = deliver_or_resume_deferred(&registry, &publisher, &store, &address, &msg).await;
        assert!(done, "resuming is a final outcome");

        let event = next_event("a genuinely free address to be resumed", &mut spawn_sub).await;
        assert_eq!(event.address, address);
        assert_eq!(event.prompt, "", "the message is the whole opening input");
        assert_eq!(
            event
                .delivered_messages
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec![msg.id.as_str()],
            "a deferred resume hands the new run the message it delivers"
        );
    }

    #[tokio::test]
    async fn send_to_completed_session_resumes_via_spawn_request() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0002");
        registry
            .record_resume_point(
                &address,
                sample_resume_point("run-old", residuum_config::ModelSelector::LARGE),
            )
            .await;

        let outcome = messenger
            .send(
                address.as_ref(),
                AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "any updates?",
                    3,
                )
                .with_id("m-resume"),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Resumed(addr) if addr == address));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        let delivered: Vec<&str> = event
            .delivered_messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            delivered,
            vec!["m-resume"],
            "the resumed run is handed the message, id and all, to report it read"
        );
        assert_eq!(event.skill.as_ref().map(AsRef::as_ref), Some("researcher"));
        assert_eq!(event.prompt, "", "the message is the whole opening input");
        assert_eq!(
            event.model,
            residuum_config::ModelSelector::LARGE,
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
        assert!(context.contains("run-old"));
        assert!(context.contains("memory_get"));
        assert!(!context.contains("episode"));
    }

    #[tokio::test]
    async fn send_to_completed_session_points_at_run_id_without_claiming_an_episode() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0003");
        registry
            .record_resume_point(
                &address,
                ResumePoint {
                    previous_run_id: "run-quiet".to_string(),
                    trigger: EventTrigger::Agent,
                    source_label: "agent:researcher".to_string(),
                    agent_skill: None,
                    model: residuum_config::ModelSelector::SMALL,
                    spawner: None,
                    depth: 1,
                    conversation_target: None,
                    recorded_at: chrono::Utc::now(),
                },
            )
            .await;

        messenger
            .send(
                address.as_ref(),
                AgentMessageEvent::new(SessionAddress::from(MAIN_ADDRESS), "main", "hi", 0),
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        let context = event.context.unwrap();
        assert!(context.contains("run-quiet"));
        assert!(context.contains("memory_get"));
        assert!(!context.contains("produced no episode"));
    }

    #[tokio::test]
    async fn resume_with_pending_hands_over_every_drained_agent_message() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0004");
        let point = sample_resume_point("run-old", residuum_config::ModelSelector::SMALL);
        registry.record_resume_point(&address, point.clone()).await;

        let messages = vec![
            PendingInput::Agent(AgentMessageEvent::new(
                SessionAddress::from(MAIN_ADDRESS),
                "main",
                "first",
                1,
            )),
            PendingInput::Agent(AgentMessageEvent::new(
                SessionAddress::from("spawned-other-0001"),
                "spawned",
                "second",
                5,
            )),
        ];

        messenger
            .resume_with_pending(&address, &point, &messages)
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        let delivered: Vec<(&str, &str)> = event
            .delivered_messages
            .iter()
            .map(|m| (m.from.as_ref(), m.content.as_str()))
            .collect();
        assert_eq!(
            delivered,
            vec![(MAIN_ADDRESS, "first"), ("spawned-other-0001", "second")],
            "each message opens the resumed run with its own sender"
        );
        assert_eq!(event.prompt, "", "there is no other opening input");
        assert_eq!(
            event.hop_count, 5,
            "the resumed run's hop count must be the highest among the combined messages"
        );
    }

    fn sample_inbound(content: &str, buffered: Option<&str>) -> MessageEvent {
        MessageEvent {
            id: "conv-1".to_string(),
            content: content.to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: Some(residuum_inference::MessageSender {
                    name: "Jane".to_string(),
                    id: "discord-jane".to_string(),
                    interface: "discord".to_string(),
                    location: Some("#builds".to_string()),
                }),
                conversation: Some(residuum_bus::origin::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: residuum_bus::origin::ConversationKind::Channel,
                    is_owner: false,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: buffered.map(str::to_string),
        }
    }

    fn conversation_spawn() -> ConversationSpawn {
        ConversationSpawn {
            source_label: "discord:#builds".to_string(),
            model: residuum_config::ModelSelector::MEDIUM,
            skill: None,
        }
    }

    #[tokio::test]
    async fn resume_with_pending_combines_agent_and_conversation_messages() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("external-discord-0004");
        let point = sample_resume_point("run-old", residuum_config::ModelSelector::SMALL);
        registry.record_resume_point(&address, point.clone()).await;

        let pending = vec![
            PendingInput::Agent(AgentMessageEvent::new(
                SessionAddress::from(MAIN_ADDRESS),
                "main",
                "checking in",
                2,
            )),
            PendingInput::External(sample_inbound("any updates?", None)),
        ];

        messenger
            .resume_with_pending(&address, &point, &pending)
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(
            !event.prompt.contains("checking in"),
            "the agent message opens the run as its own entry: {}",
            event.prompt
        );
        assert!(event.prompt.contains("any updates?"));
        assert!(event.prompt.contains("Jane"), "{}", event.prompt);
        assert_eq!(
            event
                .delivered_messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["checking in"],
            "only the agent message is handed over to be reported read"
        );
        assert_eq!(
            event
                .delivered_chat_messages
                .iter()
                .map(|m| (
                    m.content.as_str(),
                    m.origin.sender.as_ref().map(|s| s.name.as_str())
                ))
                .collect::<Vec<_>>(),
            vec![("any updates?", Some("Jane"))],
            "the conversation message is carried as it is, so a resume that loses the race can \
             deliver it with its own sender"
        );
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
            residuum_bus::session::SessionState::Idle,
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
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
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
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("external-discord-0002");
        let mut point = sample_resume_point("run-old", residuum_config::ModelSelector::LARGE);
        point.conversation_target = Some(residuum_bus::ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "chan-1".to_string(),
        });
        registry.record_resume_point(&address, point).await;

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
            event.model,
            residuum_config::ModelSelector::LARGE,
            "a resumed conversation session must keep its previous model tier"
        );
        let target = event
            .conversation
            .expect("conversation target must carry over");
        assert_eq!(target.conversation_id, "chan-1");
    }

    #[tokio::test(start_paused = true)]
    async fn deliver_conversation_to_a_completing_session_queues_and_resumes_once_clear() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "external-discord-0003",
            residuum_bus::session::SessionState::Completing,
        );
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry
            .record_resume_point(
                &info.address,
                sample_resume_point("run-old", residuum_config::ModelSelector::LARGE),
            )
            .await;

        let address = info.address.clone();
        let run_id = info.run_id.clone();

        let outcome = wait::guarded(
            "deliver_conversation to return for a completing target",
            messenger.deliver_conversation(
                &address,
                sample_inbound("any updates?", None),
                conversation_spawn(),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ConversationDeliveryOutcome::Queued(addr) if addr == address));

        assert!(
            residuum_core::testing::clock::within(std::time::Duration::from_millis(50), sub.recv())
                .await
                .is_none(),
            "the resume must wait for the completing run to clear before publishing"
        );

        registry.remove(&address, &run_id);

        let event = next_event(
            "the deferred resume to publish once the entry clears",
            &mut sub,
        )
        .await;
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-0005"),
                    "spawned",
                    "hello",
                    0,
                ),
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-leak"),
                    "spawned",
                    "hello",
                    3,
                ),
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-0006"),
                    "spawned",
                    "still going",
                    5,
                ),
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-0007"),
                    "spawned",
                    "just starting",
                    1,
                ),
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
                AgentMessageEvent::new(
                    SessionAddress::from("spawned-researcher-0008"),
                    "spawned",
                    "are we there yet",
                    10,
                ),
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
            err.to_string().contains("hop_hard_limit"),
            "error should name the config setting that controls the limit, got: {err}"
        );

        bus_barrier(&bus_handle).await;
        assert!(
            sub.drain().is_empty(),
            "a refused message must never actually reach the target"
        );
    }

    #[tokio::test]
    async fn hop_limit_refusal_records_a_note_in_a_live_sender_and_receiver_transcript() {
        let registry = Arc::new(SessionRegistry::new());
        let sender_info = sample_live_info(
            "spawned-sender-0001",
            residuum_bus::session::SessionState::Running,
        );
        let receiver_info = sample_live_info(
            "spawned-receiver-0001",
            residuum_bus::session::SessionState::Idle,
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
            residuum_bus::Publisher::noop(),
            Arc::clone(&store),
            HopLimits { soft: 5, hard: 3 },
        );
        let outcome = messenger
            .send(
                receiver_info.address.as_ref(),
                AgentMessageEvent::new(sender_info.address.clone(), "spawned", "loop!", 3),
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
        let bus_handle = residuum_bus::spawn_broker();
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let sender_info = sample_live_info(
            "spawned-sender-0002",
            residuum_bus::session::SessionState::Running,
        );
        let receiver_info = sample_live_info(
            "spawned-receiver-0002",
            residuum_bus::session::SessionState::Idle,
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
                AgentMessageEvent::new(sender_info.address.clone(), "spawned", "loop!", 3),
            )
            .await;
        assert!(matches!(outcome, Err(SendError::HopLimitExceeded { .. })));

        let mut tagged = Vec::new();
        for _ in 0..2 {
            let event = events.recv().await.unwrap().unwrap();
            assert!(
                matches!(&event.kind, SessionEventKind::Error { message, .. } if message.contains("Message Loop Limit")),
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

//! Conversation router: decides, for each admitted inbound interface
//! message, whether it belongs to the main agent or to a conversation's
//! session, and delivers it in the latter case.
//!
//! Admission (whether a message reaches the agent system at all — owner
//! claim, `respond_to_others`, standing) is decided upstream by each
//! interface handler before the message ever reaches the bus. This router
//! only decides who handles an already-admitted message:
//! [`crate::interfaces::types::MessageOrigin::belongs_to_main`] is the actual
//! routing predicate; this module is the delivery side once that predicate
//! says a message is not main's.

use std::sync::Arc;

use crate::config::BackgroundModelTier;
use crate::interfaces::types::InboundMessage;

use super::messaging::{AgentMessenger, ConversationSpawn};
use super::registry::conversation_session_address;

/// Routes admitted inbound conversation messages to their per-conversation
/// session, deriving each conversation's deterministic address and source
/// label and handing delivery to [`AgentMessenger::deliver_conversation`].
pub struct ConversationRouter {
    messenger: Arc<AgentMessenger>,
    /// Model tier a conversation session runs at. Fixed at `Medium`, matching
    /// the webhook `external` session precedent — there is no per-conversation
    /// configuration surface for this today.
    model_tier: BackgroundModelTier,
}

impl ConversationRouter {
    /// Create a router over the given agent messenger.
    #[must_use]
    pub fn new(messenger: Arc<AgentMessenger>) -> Self {
        Self {
            messenger,
            model_tier: BackgroundModelTier::Medium,
        }
    }

    /// Route one inbound message to its conversation's session.
    ///
    /// Callers must only pass messages for which
    /// `origin.belongs_to_main()` is `false` — this never checks that itself,
    /// since the two call sites (the gateway's idle-loop and mid-turn message
    /// intake) already branch on it before reaching here. A message with no
    /// conversation context is dropped with an error log, since routing has
    /// nothing to derive an address from; that should never actually happen
    /// given the caller contract.
    #[tracing::instrument(skip_all, fields(msg_id = %message.id, endpoint = %message.origin.endpoint))]
    pub async fn route(&self, message: InboundMessage) {
        let Some(conversation) = message.origin.conversation.as_ref() else {
            tracing::error!(
                msg_id = %message.id,
                endpoint = %message.origin.endpoint,
                "conversation router received a message with no conversation context; dropped"
            );
            return;
        };
        let address = conversation_session_address(&message.origin.endpoint, &conversation.id);
        let label = message
            .origin
            .sender
            .as_ref()
            .and_then(|s| s.location.clone())
            .unwrap_or_else(|| conversation.id.clone());
        let spawn = ConversationSpawn {
            source_label: format!("{}:{label}", message.origin.endpoint),
            model_tier: self.model_tier,
            skill: None,
        };

        match self
            .messenger
            .deliver_conversation(&address, message, spawn)
            .await
        {
            Ok(super::messaging::ConversationDeliveryOutcome::Live(addr)) => {
                tracing::debug!(address = %addr, "conversation message delivered to its live session");
            }
            Ok(super::messaging::ConversationDeliveryOutcome::Started(addr)) => {
                tracing::info!(address = %addr, "started a new conversation session");
            }
            Ok(super::messaging::ConversationDeliveryOutcome::Resumed(addr)) => {
                tracing::info!(address = %addr, "resumed a completed conversation session");
            }
            Ok(super::messaging::ConversationDeliveryOutcome::Queued(addr)) => {
                // Covers both a saturated channel and a completing run: a
                // detached task retries delivery until it succeeds (see
                // `AgentMessenger::deliver_conversation`) — this participant's
                // message is never dropped for either reason.
                tracing::debug!(address = %addr, "conversation message queued for retried delivery");
            }
            Err(e) => {
                tracing::error!(address = %address, error = %e, "failed to route conversation message to its session");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::types::{ConversationContext, ConversationKind, MessageOrigin};

    fn inbound(endpoint: &str, conversation_id: &str, location: Option<&str>) -> InboundMessage {
        InboundMessage {
            id: "m1".to_string(),
            content: "can you check the build?".to_string(),
            origin: MessageOrigin {
                endpoint: endpoint.to_string(),
                sender: location.map(|loc| crate::inference::MessageSender {
                    name: "Jane".to_string(),
                    id: "discord-jane".to_string(),
                    interface: endpoint.to_string(),
                    location: Some(loc.to_string()),
                }),
                conversation: Some(ConversationContext {
                    id: conversation_id.to_string(),
                    kind: ConversationKind::Channel,
                    is_owner: false,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: None,
        }
    }

    fn router() -> (
        ConversationRouter,
        crate::bus::BusHandle,
        Arc<super::super::registry::SessionRegistry>,
    ) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(super::super::registry::SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(super::super::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            store,
            crate::agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        (ConversationRouter::new(messenger), bus_handle, registry)
    }

    #[tokio::test]
    async fn routing_the_same_conversation_twice_resolves_to_the_same_address() {
        let (router, bus_handle, _registry) = router();
        let mut spawns: crate::bus::Subscriber<crate::bus::SpawnRequestEvent> = bus_handle
            .subscribe(crate::bus::topics::Background)
            .await
            .unwrap();

        router
            .route(inbound("discord", "chan-1", Some("#builds")))
            .await;
        router
            .route(inbound("discord", "chan-1", Some("#builds")))
            .await;

        let address = conversation_session_address("discord", "chan-1");
        let first = tokio::time::timeout(std::time::Duration::from_secs(1), spawns.recv())
            .await
            .expect("first message should start a session")
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), spawns.recv())
            .await
            .expect("second message in the same conversation should route the same way")
            .unwrap()
            .unwrap();
        assert_eq!(first.address, address);
        assert_eq!(
            second.address, address,
            "the same conversation must always resolve to the same session address"
        );
    }

    #[test]
    fn source_label_prefers_the_sender_location_over_the_raw_conversation_id() {
        let msg = inbound("discord", "chan-1", Some("#builds (Eng Team)"));
        let label = msg
            .origin
            .sender
            .as_ref()
            .and_then(|s| s.location.clone())
            .unwrap_or_else(|| msg.origin.conversation.as_ref().unwrap().id.clone());
        assert_eq!(label, "#builds (Eng Team)");
    }

    #[test]
    fn source_label_falls_back_to_the_conversation_id_without_a_location() {
        let msg = inbound("telegram", "-100123", None);
        let label = msg
            .origin
            .sender
            .as_ref()
            .and_then(|s| s.location.clone())
            .unwrap_or_else(|| msg.origin.conversation.as_ref().unwrap().id.clone());
        assert_eq!(label, "-100123");
    }

    #[tokio::test]
    async fn a_message_with_no_conversation_context_is_dropped_not_panicked() {
        let (router, _bus_handle, _registry) = router();
        let mut msg = inbound("discord", "chan-1", None);
        msg.origin.conversation = None;
        // Must not panic; the caller contract says this shouldn't happen,
        // but the router still degrades to a dropped-with-log message.
        router.route(msg).await;
    }

    #[tokio::test]
    async fn a_busy_session_still_delivers_the_user_message_via_retry() {
        // Regression: a saturated channel used to make the router drop this
        // participant's message on the floor (notifying main it happened,
        // but never actually delivering it). A chat user's message must
        // never be dropped for capacity, unlike an agent-to-agent message
        // (which does get a visible, and acceptable, Busy refusal) — there
        // is nobody on the other end of a chat message to hand a refusal
        // to, so this must keep retrying until the channel drains, exactly
        // the way a live session's own tool loop drains it continuously.
        let (router, _bus_handle, registry) = router();
        let address = conversation_session_address("discord", "chan-1");
        let info = crate::background::registry::SessionInfo {
            address: address.clone(),
            run_id: "run-1".to_string(),
            category: crate::background::registry::SessionCategory::External,
            trigger: crate::bus::EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: crate::background::registry::SessionState::Idle,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: Some(crate::bus::ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        // The receiver must stay alive and be drained below like a live
        // session's own tool loop would — dropping it here would close the
        // channel instead of merely saturating it, a different condition
        // (`DeliverOutcome::NotLive`, not `Full`) that this test isn't
        // exercising.
        let mut rx = registry
            .register(info, tokio_util::sync::CancellationToken::new())
            .unwrap();
        // Saturate the session's interrupt channel directly so the router's
        // own delivery attempt below finds it full.
        for _ in 0..crate::background::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(matches!(
                registry.deliver(
                    &address,
                    crate::agent::interrupt::Interrupt::UserMessage(inbound(
                        "discord",
                        "chan-1",
                        Some("#builds"),
                    )),
                ),
                crate::background::registry::DeliverOutcome::Delivered
            ));
        }

        let mut new_message = inbound("discord", "chan-1", Some("#builds"));
        new_message.id = "the-new-message".to_string();
        new_message.content = "can anyone see this?".to_string();
        router.route(new_message).await;

        // Drain the channel the way a live turn's own tool loop would, one
        // interrupt at a time, freeing capacity for the router's detached
        // retry task to land the new message.
        let mut delivered = false;
        for _ in 0..crate::background::registry::INTERRUPT_CHANNEL_CAPACITY + 5 {
            let Ok(Some(interrupt)) =
                tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let crate::agent::interrupt::Interrupt::UserMessage(m) = interrupt
                && m.id == "the-new-message"
            {
                assert_eq!(m.content, "can anyone see this?");
                delivered = true;
                break;
            }
        }
        assert!(
            delivered,
            "the new user message must eventually be delivered, never dropped"
        );
    }

    #[tokio::test]
    async fn a_completing_session_queues_the_message_without_blocking_the_router() {
        // The `Completing` branch (a run whose teardown is in progress)
        // must keep working exactly as before this fix — only the `Full`
        // handling changed. The run never actually clears in this test, so
        // this only asserts `route()` itself returns promptly rather than
        // blocking on however long that teardown takes.
        let (router, _bus_handle, registry) = router();
        let address = conversation_session_address("discord", "chan-2");
        let info = crate::background::registry::SessionInfo {
            address: address.clone(),
            run_id: "run-1".to_string(),
            category: crate::background::registry::SessionCategory::External,
            trigger: crate::bus::EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: crate::background::registry::SessionState::Completing,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: Some(crate::bus::ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-2".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
        };
        registry
            .register(info, tokio_util::sync::CancellationToken::new())
            .unwrap();

        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            router.route(inbound("discord", "chan-2", Some("#builds"))),
        )
        .await
        .expect("route() must hand off to a detached task, never block on a completing run");
    }
}

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
                tracing::debug!(address = %addr, "queued conversation message for a completing session");
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
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: None,
        }
    }

    fn router() -> (ConversationRouter, crate::bus::BusHandle) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(super::super::registry::SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(super::super::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            crate::agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        (ConversationRouter::new(messenger), bus_handle)
    }

    #[tokio::test]
    async fn routing_the_same_conversation_twice_resolves_to_the_same_address() {
        let (router, bus_handle) = router();
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
        let (router, _bus_handle) = router();
        let mut msg = inbound("discord", "chan-1", None);
        msg.origin.conversation = None;
        // Must not panic; the caller contract says this shouldn't happen,
        // but the router still degrades to a dropped-with-log message.
        router.route(msg).await;
    }
}

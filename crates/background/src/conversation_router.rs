use std::sync::Arc;

use residuum_bus::MessageEvent;
use residuum_config::ModelSelector;

use super::messaging::{AgentMessenger, ConversationSpawn};
use super::registry::conversation_session_address;

pub struct ConversationRouter {
    messenger: Arc<AgentMessenger>,
    model: ModelSelector,
}

impl ConversationRouter {
    #[must_use]
    pub fn new(messenger: Arc<AgentMessenger>) -> Self {
        Self {
            messenger,
            model: ModelSelector::MEDIUM,
        }
    }

    #[tracing::instrument(skip_all, fields(msg_id = %message.id, endpoint = %message.origin.endpoint))]
    pub async fn route(&self, message: MessageEvent) {
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
            model: self.model.clone(),
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
    use residuum_bus::origin::{ConversationContext, ConversationKind, MessageOrigin};
    use residuum_bus::testing::next_event;
    use residuum_core::testing::wait;

    fn inbound(endpoint: &str, conversation_id: &str, location: Option<&str>) -> MessageEvent {
        MessageEvent {
            id: "m1".to_string(),
            content: "can you check the build?".to_string(),
            origin: MessageOrigin {
                endpoint: endpoint.to_string(),
                sender: location.map(|loc| residuum_inference::MessageSender {
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
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    fn router() -> (
        ConversationRouter,
        residuum_bus::BusHandle,
        Arc<crate::registry::SessionRegistry>,
    ) {
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(crate::registry::SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(super::super::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        (ConversationRouter::new(messenger), bus_handle, registry)
    }

    #[tokio::test]
    async fn routing_the_same_conversation_twice_resolves_to_the_same_address() {
        let (router, bus_handle, _registry) = router();
        let mut spawns: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> = bus_handle
            .subscribe(residuum_bus::topics::Background)
            .await
            .unwrap();

        router
            .route(inbound("discord", "chan-1", Some("#builds")))
            .await;
        router
            .route(inbound("discord", "chan-1", Some("#builds")))
            .await;

        let address = conversation_session_address("discord", "chan-1");
        let first = next_event("the first message to start a session", &mut spawns).await;
        let second = next_event(
            "the second message in the same conversation to route the same way",
            &mut spawns,
        )
        .await;
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
        router.route(msg).await;
    }

    #[tokio::test]
    async fn a_busy_session_still_delivers_the_user_message_via_retry() {
        let (router, _bus_handle, registry) = router();
        let address = conversation_session_address("discord", "chan-1");
        let info = residuum_bus::session::SessionInfo {
            address: address.clone(),
            run_id: "run-1".to_string(),
            category: residuum_bus::session::SessionCategory::External,
            trigger: residuum_bus::EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: residuum_bus::session::SessionState::Idle,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(residuum_bus::ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let mut rx = registry
            .register(info, tokio_util::sync::CancellationToken::new())
            .unwrap();
        for _ in 0..crate::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(matches!(
                registry.deliver(
                    &address,
                    residuum_bus::interrupt::Interrupt::UserMessage(inbound(
                        "discord",
                        "chan-1",
                        Some("#builds"),
                    )),
                ),
                crate::registry::DeliverOutcome::Delivered
            ));
        }

        let mut new_message = inbound("discord", "chan-1", Some("#builds"));
        new_message.id = "the-new-message".to_string();
        new_message.content = "can anyone see this?".to_string();
        router.route(new_message).await;

        let interrupt = wait::next_matching(
            "the new user message to be delivered, never dropped",
            &mut rx,
            |interrupt| {
                matches!(
                    interrupt,
                    residuum_bus::interrupt::Interrupt::UserMessage(m) if m.id == "the-new-message"
                )
            },
        )
        .await;
        match interrupt {
            residuum_bus::interrupt::Interrupt::UserMessage(m) => {
                assert_eq!(m.content, "can anyone see this?");
            }
            residuum_bus::interrupt::Interrupt::AgentMessage(_)
            | residuum_bus::interrupt::Interrupt::Subconscious(_)
            | residuum_bus::interrupt::Interrupt::Stopped => {
                panic!("the new user message should be the one delivered")
            }
        }
    }

    #[tokio::test]
    async fn a_completing_session_queues_the_message_without_blocking_the_router() {
        let (router, _bus_handle, registry) = router();
        let address = conversation_session_address("discord", "chan-2");
        let info = residuum_bus::session::SessionInfo {
            address: address.clone(),
            run_id: "run-1".to_string(),
            category: residuum_bus::session::SessionCategory::External,
            trigger: residuum_bus::EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: residuum_bus::session::SessionState::Completing,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(residuum_bus::ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-2".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        registry
            .register(info, tokio_util::sync::CancellationToken::new())
            .unwrap();

        wait::guarded(
            "route() to hand off to a detached task, never block on a completing run",
            router.route(inbound("discord", "chan-2", Some("#builds"))),
        )
        .await;
    }
}

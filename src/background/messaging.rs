//! Agent messaging: routes a `message_agent` call to its target by address.
//!
//! Delivery depends on the target's lifecycle state:
//! - **running/idle session** — delivered through the registry's interrupt
//!   channel: an interrupt at the target's next tool-call boundary while
//!   running, or the input for a new turn while idle (the runtime on the
//!   other end decides which).
//! - **completed session** — a resume pointer is looked up in the registry
//!   and a fresh `SpawnRequestEvent` is published for it, starting a new run
//!   at the same address through the ordinary spawn-listener path.
//! - **main** — reuses the existing `MessageEvent`/`UserMessage` bus path,
//!   which already implements interrupt-if-running/new-turn-if-idle for the
//!   main agent.
//! - **unknown** — neither a live session nor a resume point exists.

use std::sync::Arc;

use crate::agent::interrupt::Interrupt;
use crate::bus::{AgentMessageEvent, MessageEvent, Publisher, SessionAddress, topics};
use crate::config::BackgroundModelTier;
use crate::interfaces::types::MessageOrigin;

use super::registry::{MAIN_ADDRESS, ResumePoint, SessionRegistry};

/// What happened when a message was sent to an address.
#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    /// Delivered to the main agent.
    Main,
    /// Delivered to a live (running, idle, or forking) session.
    Live(SessionAddress),
    /// The session had completed; a new run was started at the same address.
    Resumed(SessionAddress),
    /// No live session, and no record of this address ever having run.
    Unknown,
}

/// Routes agent-to-agent messages by address.
pub struct AgentMessenger {
    registry: Arc<SessionRegistry>,
    publisher: Publisher,
}

impl AgentMessenger {
    /// Create a new messenger over the given registry and bus publisher.
    #[must_use]
    pub fn new(registry: Arc<SessionRegistry>, publisher: Publisher) -> Self {
        Self {
            registry,
            publisher,
        }
    }

    /// Send `content` from `from` (identified by address and category) to
    /// `to`. `to` may be `"main"`, a live session's address, or a completed
    /// session's address.
    pub async fn send(
        &self,
        to: &str,
        from: SessionAddress,
        from_category: String,
        content: String,
    ) -> DeliveryOutcome {
        if to == MAIN_ADDRESS {
            self.deliver_to_main(from, from_category, content).await;
            return DeliveryOutcome::Main;
        }

        let address = SessionAddress::from(to);
        let message = AgentMessageEvent {
            from: from.clone(),
            from_category: from_category.clone(),
            content: content.clone(),
            hop_count: 0,
        };
        if self
            .registry
            .deliver(&address, Interrupt::AgentMessage(message))
        {
            return DeliveryOutcome::Live(address);
        }

        let Some(point) = self.registry.resume_point(&address) else {
            return DeliveryOutcome::Unknown;
        };
        self.resume(&address, &point, from, from_category, content)
            .await;
        DeliveryOutcome::Resumed(address)
    }

    /// Deliver to the main agent by publishing a `MessageEvent` on the
    /// `UserMessage` topic, exactly like a relayed session result does today
    /// — the gateway's own turn loop already treats an inbound message on
    /// this topic as an interrupt when a turn is running, and as fresh input
    /// for a new turn when it's idle.
    async fn deliver_to_main(&self, from: SessionAddress, from_category: String, content: String) {
        let msg = AgentMessageEvent {
            from,
            from_category,
            content,
            hop_count: 0,
        };
        let event = MessageEvent {
            id: format!("agent-msg-{}", uuid::Uuid::new_v4()),
            content: msg.format_for_agent(),
            origin: MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: Vec::new(),
            context: None,
        };
        if let Err(e) = self.publisher.publish(topics::UserMessage, event).await {
            tracing::warn!(error = %e, "failed to deliver agent message to main");
        }
    }

    /// Resume a completed session as a new run at the same address, by
    /// publishing a fresh `SpawnRequestEvent` for the spawn listener to pick
    /// up — the same path any other session fork takes. The new run's
    /// prompt is the delivered message; its context carries a pointer back
    /// to the previous run's episode (or its run id, if it produced none),
    /// so the resumed session can retrieve it with `memory_get`.
    async fn resume(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        from: SessionAddress,
        from_category: String,
        content: String,
    ) {
        let msg = AgentMessageEvent {
            from,
            from_category,
            content,
            hop_count: 0,
        };
        let pointer_note = match &point.previous_episode_id {
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
        };

        let event = crate::bus::SpawnRequestEvent {
            address: address.clone(),
            skill: point.agent_skill.clone(),
            source_label: point.source_label.clone(),
            prompt: msg.format_for_agent(),
            context: Some(pointer_note),
            source: point.trigger.clone(),
            model_tier: BackgroundModelTier::Medium,
        };

        if let Err(e) = self.publisher.publish(topics::Background, event).await {
            tracing::error!(error = %e, address = %address, "failed to publish resume spawn request");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{EventTrigger, SkillName, Subscriber};
    use tokio_util::sync::CancellationToken;

    fn messenger() -> (AgentMessenger, Arc<SessionRegistry>, crate::bus::BusHandle) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let messenger = AgentMessenger::new(Arc::clone(&registry), bus_handle.publisher());
        (messenger, registry, bus_handle)
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
            )
            .await;
        assert!(matches!(outcome, DeliveryOutcome::Main));

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.content.contains("spawned-researcher-3f9a"));
        assert!(event.content.contains("spawned"));
        assert!(event.content.contains("found the answer"));
        assert_eq!(event.origin.endpoint, "background");
    }

    #[tokio::test]
    async fn send_to_live_session_delivers_via_registry() {
        let (messenger, registry, _bus_handle) = messenger();
        let info = crate::background::registry::SessionInfo {
            address: SessionAddress::from("spawned-researcher-0001"),
            run_id: "run-1".to_string(),
            category: crate::background::registry::SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: crate::background::registry::SessionState::Idle,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            started_at: chrono::Utc::now(),
        };
        let mut rx = registry.register(info.clone(), CancellationToken::new());

        let outcome = messenger
            .send(
                info.address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "how's it going?".to_string(),
            )
            .await;
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
            )
            .await;
        assert!(matches!(outcome, DeliveryOutcome::Unknown));
    }

    #[tokio::test]
    async fn send_to_completed_session_resumes_via_spawn_request() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0002");
        registry.record_resume_point(
            &address,
            ResumePoint {
                previous_run_id: "run-old".to_string(),
                previous_episode_id: Some("ep-42".to_string()),
                trigger: EventTrigger::Agent,
                source_label: "agent:researcher".to_string(),
                agent_skill: Some(SkillName::from("researcher")),
            },
        );

        let outcome = messenger
            .send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "any updates?".to_string(),
            )
            .await;
        assert!(matches!(outcome, DeliveryOutcome::Resumed(addr) if addr == address));

        let event = sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        assert_eq!(event.skill.as_ref().map(AsRef::as_ref), Some("researcher"));
        assert!(event.prompt.contains("any updates?"));
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
            },
        );

        messenger
            .send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "hi".to_string(),
            )
            .await;

        let event = sub.recv().await.unwrap().unwrap();
        let context = event.context.unwrap();
        assert!(context.contains("run-quiet"));
        assert!(context.contains("produced no episode"));
    }
}

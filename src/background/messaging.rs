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

use std::fmt;
use std::sync::Arc;

use crate::agent::interrupt::Interrupt;
use crate::bus::{AgentMessageEvent, MessageEvent, Publisher, SessionAddress, topics};
use crate::interfaces::types::MessageOrigin;

use super::registry::{DeliverOutcome, MAIN_ADDRESS, ResumePoint, SessionRegistry};

/// What happened when a message was sent to an address.
#[derive(Debug, Clone)]
pub enum DeliveryOutcome {
    /// Delivered to the main agent.
    Main,
    /// Delivered to a live (running or idle) session.
    Live(SessionAddress),
    /// The session had completed (or was completing when the message
    /// arrived); a new run was started at the same address.
    Resumed(SessionAddress),
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
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(address) => write!(f, "agent {address} is busy, try again shortly"),
            Self::PublishFailed(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for SendError {}

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
    /// (or completing) session's address.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Busy`] if the target is live but its interrupt
    /// channel is saturated, and [`SendError::PublishFailed`] if delivering
    /// the message (to main, or as a resume spawn request) failed at the
    /// bus. Both are real delivery failures the caller must not treat as
    /// success.
    pub async fn send(
        &self,
        to: &str,
        from: SessionAddress,
        from_category: String,
        content: String,
    ) -> Result<DeliveryOutcome, SendError> {
        if to == MAIN_ADDRESS {
            self.deliver_to_main(from, from_category, content).await?;
            return Ok(DeliveryOutcome::Main);
        }

        let address = SessionAddress::from(to);
        let message = AgentMessageEvent {
            from: from.clone(),
            from_category: from_category.clone(),
            content: content.clone(),
            hop_count: 0,
        };
        match self
            .registry
            .deliver(&address, Interrupt::AgentMessage(message))
        {
            DeliverOutcome::Delivered => return Ok(DeliveryOutcome::Live(address)),
            DeliverOutcome::Full => return Err(SendError::Busy(address)),
            DeliverOutcome::Completing => {
                // The target's current run no longer accepts input; it must
                // fully leave the registry (recording its resume point on
                // the way out — see `finish_run`) before a new run can start
                // at the same address, so the two are never both live. By
                // the time it clears, a resume point for this exact run is
                // guaranteed to exist (finish_run always records one before
                // removing the entry), so a missing point here would be an
                // internal inconsistency, not a genuinely unknown address.
                self.registry.wait_until_clear(&address).await;
                let Some(point) = self.registry.resume_point(&address) else {
                    tracing::error!(address = %address, "session left the registry with no resume point recorded");
                    return Err(SendError::PublishFailed(format!(
                        "session {address} completed but left no resume point; message not delivered"
                    )));
                };
                self.resume(&address, &point, from, from_category, content)
                    .await?;
                return Ok(DeliveryOutcome::Resumed(address));
            }
            DeliverOutcome::NotLive => {}
        }

        let Some(point) = self.registry.resume_point(&address) else {
            return Ok(DeliveryOutcome::Unknown);
        };
        self.resume(&address, &point, from, from_category, content)
            .await?;
        Ok(DeliveryOutcome::Resumed(address))
    }

    /// Deliver to the main agent by publishing a `MessageEvent` on the
    /// `UserMessage` topic, exactly like a relayed session result does today
    /// — the gateway's own turn loop already treats an inbound message on
    /// this topic as an interrupt when a turn is running, and as fresh input
    /// for a new turn when it's idle.
    async fn deliver_to_main(
        &self,
        from: SessionAddress,
        from_category: String,
        content: String,
    ) -> Result<(), SendError> {
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
        self.publisher
            .publish(topics::UserMessage, event)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "failed to deliver agent message to main");
                SendError::PublishFailed("failed to deliver message to main".to_string())
            })
    }

    /// Resume a completed (or just-completed) session as a new run at the
    /// same address, by publishing a fresh `SpawnRequestEvent` for the spawn
    /// listener to pick up — the same path any other session fork takes.
    /// The new run's prompt is the delivered message; its context carries a
    /// pointer back to the previous run's episode (or its run id, if it
    /// produced none), so the resumed session can retrieve it with
    /// `memory_get`.
    async fn resume(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        from: SessionAddress,
        from_category: String,
        content: String,
    ) -> Result<(), SendError> {
        let msg = AgentMessageEvent {
            from,
            from_category,
            content,
            hop_count: 0,
        };
        self.publish_resume(address, point, msg.format_for_agent())
            .await
    }

    /// Resume a session as a new run, combining several buffered agent
    /// messages into a single kickoff prompt.
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
    pub(crate) async fn resume_with_messages(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        messages: &[AgentMessageEvent],
    ) -> Result<(), SendError> {
        let combined = messages
            .iter()
            .map(AgentMessageEvent::format_for_agent)
            .collect::<Vec<_>>()
            .join("\n\n");
        self.publish_resume(address, point, combined).await
    }

    /// Build and publish the `SpawnRequestEvent` that resumes `address` from
    /// `point`, with `prompt` as the new run's opening input. Shared by
    /// [`Self::resume`] and [`Self::resume_with_messages`], which differ
    /// only in how they arrive at `prompt`.
    async fn publish_resume(
        &self,
        address: &SessionAddress,
        point: &ResumePoint,
        prompt: String,
    ) -> Result<(), SendError> {
        let event = crate::bus::SpawnRequestEvent {
            address: address.clone(),
            skill: point.agent_skill.clone(),
            source_label: point.source_label.clone(),
            prompt,
            context: Some(Self::pointer_note(point)),
            source: point.trigger.clone(),
            model_tier: point.model_tier,
        };

        self.publisher
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
            )
            .await
            .unwrap();
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
        let info = sample_live_info(
            "spawned-researcher-0001",
            crate::background::registry::SessionState::Idle,
        );
        let mut rx = registry.register(info.clone(), CancellationToken::new());

        let outcome = messenger
            .send(
                info.address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "how's it going?".to_string(),
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
        let _rx = registry.register(info.clone(), CancellationToken::new());
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
    async fn send_to_a_completing_session_waits_then_resumes_once() {
        let (messenger, registry, bus_handle) = messenger();
        let mut spawn_sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let info = sample_live_info(
            "spawned-researcher-000c",
            crate::background::registry::SessionState::Completing,
        );
        let _rx = registry.register(info.clone(), CancellationToken::new());
        registry.record_resume_point(
            &info.address,
            sample_resume_point("run-old", crate::config::BackgroundModelTier::Large),
        );

        let address = info.address.clone();
        let run_id = info.run_id.clone();
        let send = tokio::spawn({
            let address = address.clone();
            async move {
                messenger
                    .send(
                        address.as_ref(),
                        SessionAddress::from(MAIN_ADDRESS),
                        "main".to_string(),
                        "any updates?".to_string(),
                    )
                    .await
            }
        });

        // The send must still be waiting while the completing entry is live.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(!send.is_finished());

        registry.remove(&address, &run_id);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), send)
            .await
            .expect("send should complete once the entry clears")
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, DeliveryOutcome::Resumed(addr) if addr == address));

        let event = spawn_sub.recv().await.unwrap().unwrap();
        assert_eq!(event.address, address);
        assert!(event.prompt.contains("any updates?"));
        assert_eq!(event.model_tier, crate::config::BackgroundModelTier::Large);
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
            },
        );

        messenger
            .send(
                address.as_ref(),
                SessionAddress::from(MAIN_ADDRESS),
                "main".to_string(),
                "hi".to_string(),
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        let context = event.context.unwrap();
        assert!(context.contains("run-quiet"));
        assert!(context.contains("produced no episode"));
    }

    #[tokio::test]
    async fn resume_with_messages_combines_all_drained_messages_into_one_prompt() {
        let (messenger, registry, bus_handle) = messenger();
        let mut sub: Subscriber<crate::bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let address = SessionAddress::from("spawned-researcher-0004");
        let point = sample_resume_point("run-old", crate::config::BackgroundModelTier::Small);
        registry.record_resume_point(&address, point.clone());

        let messages = vec![
            AgentMessageEvent {
                from: SessionAddress::from(MAIN_ADDRESS),
                from_category: "main".to_string(),
                content: "first".to_string(),
                hop_count: 0,
            },
            AgentMessageEvent {
                from: SessionAddress::from("spawned-other-0001"),
                from_category: "spawned".to_string(),
                content: "second".to_string(),
                hop_count: 0,
            },
        ];

        messenger
            .resume_with_messages(&address, &point, &messages)
            .await
            .unwrap();

        let event = sub.recv().await.unwrap().unwrap();
        assert!(event.prompt.contains("first"));
        assert!(event.prompt.contains("second"));
        assert!(event.prompt.contains("main"));
        assert!(event.prompt.contains("spawned-other-0001"));
    }

    #[tokio::test]
    async fn publish_failure_is_reported_as_an_error_not_success() {
        let registry = Arc::new(SessionRegistry::new());
        let messenger = AgentMessenger::new(Arc::clone(&registry), Publisher::noop());

        let err = messenger
            .send(
                MAIN_ADDRESS,
                SessionAddress::from("spawned-researcher-0005"),
                "spawned".to_string(),
                "hello".to_string(),
            )
            .await
            .expect_err("a noop publisher must surface as a delivery failure");
        assert!(matches!(err, SendError::PublishFailed(_)));
    }
}

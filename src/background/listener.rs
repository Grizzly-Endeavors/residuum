//! Bus listener that turns spawn requests into running sessions.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the run to `SessionRuntime`.
//! Everything the session runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::agent::interrupt::Interrupt;
use crate::background::registry::{
    DeliverOutcome, MAIN_ADDRESS, SessionCategory, SessionRegistry, SessionState,
};
use crate::background::runtime::SessionSpawnRequest;
use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::types::SubAgentConfig;
use crate::bus::{
    AgentMessageEvent, BusHandle, EventTrigger, SessionAddress, SpawnRequestEvent, Subscriber,
    topics,
};
use crate::interfaces::types::InboundMessage;

/// Subscribe to the `Background` topic and fork sessions on demand.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the subscriber closes.
#[tracing::instrument(skip_all)]
pub(crate) async fn spawn_listener(
    ctx: Arc<SpawnContext>,
    bus_handle: &BusHandle,
) -> Option<JoinHandle<()>> {
    let subscriber: Subscriber<SpawnRequestEvent> = match bus_handle
        .subscribe(topics::Background)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "spawn listener failed to subscribe to Background topic");
            return None;
        }
    };

    tracing::info!("spawn listener subscribed to Background topic");

    Some(tokio::spawn(listener_loop(ctx, subscriber)))
}

/// Main loop: reads spawn requests and executes them.
async fn listener_loop(ctx: Arc<SpawnContext>, mut subscriber: Subscriber<SpawnRequestEvent>) {
    loop {
        match subscriber.recv().await {
            Ok(Some(event)) => {
                let source_label = event.source_label.clone();
                if let Err(e) = handle_spawn_request(&ctx, event).await {
                    tracing::warn!(
                        source = %source_label,
                        error = %e,
                        "failed to fork session"
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "spawn listener subscriber error, shutting down");
                break;
            }
        }
    }
    tracing::info!("spawn listener shutting down");
}

/// Handle a single spawn request: guard against a live or tearing-down
/// address, then build resources and fork the session.
///
/// Two runs must never be live at the same address at once (see
/// `SessionRegistry::deliver` and `AgentMessenger::send`, which already wait
/// for a `completing` target to clear before publishing a resume). This is
/// the last line of defense against that: a request for an address that is
/// already `forking`/`running`/`idle` delivers its content into that live
/// run as a message instead of forking a second one — this is the normal
/// (not exceptional) outcome when two messages were queued to the same
/// completing session and both ended up resuming it (see
/// `AgentMessenger::deferred_resume`), since only one of the two resulting
/// spawn requests can actually win the fork. A request for an address that
/// is `completing` is deferred, off the listener's own task so it doesn't
/// block other addresses' spawns, until the address clears.
fn handle_spawn_request<'a>(
    ctx: &'a Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + 'a>> {
    // Written as a plain fn returning a boxed future, rather than `async
    // fn`, because the `Completing` branch below recurses into this same
    // function (via a detached task) — an `async fn` calling itself, even
    // indirectly through `tokio::spawn`, creates a cyclic `Send`-auto-trait
    // computation rustc cannot resolve on its own; boxing here gives the
    // recursive call a concrete, non-opaque type that breaks the cycle.
    Box::pin(async move {
        if let Some(existing) = ctx.session_registry.get(&event.address) {
            match existing.state {
                SessionState::Completing => {
                    tracing::info!(
                        address = %event.address,
                        "spawn target is completing; deferring until it clears the registry"
                    );
                    let ctx = Arc::clone(ctx);
                    tokio::spawn(async move {
                        ctx.session_registry.wait_until_clear(&event.address).await;
                        // Re-run the full guard rather than forking directly:
                        // by the time the wait ends, another spawn/resume for
                        // this same address may already have won (this
                        // detached task has no ordering relative to the
                        // listener's own sequential processing, or to
                        // another such detached task), so the address could
                        // be live again, or even back to `completing` once
                        // more. `handle_spawn_request` re-checks state fresh
                        // and recurses through this same deferral if so,
                        // instead of blindly clobbering a run that won the
                        // race.
                        if let Err(e) = handle_spawn_request(&ctx, event).await {
                            tracing::warn!(error = %e, "failed to fork deferred session resume");
                        }
                    });
                    return Ok(());
                }
                SessionState::Forking | SessionState::Running | SessionState::Idle => {
                    // Two concurrent resume attempts for the same address
                    // (e.g. two messages queued while a session was
                    // completing, each deferred separately — see
                    // `AgentMessenger::deferred_resume`) can both observe the
                    // address as free and both end up publishing a spawn
                    // request. By the time this second one is processed, the
                    // first has already won and registered a live run —
                    // deliver this request's content into that run as a
                    // message instead of refusing it outright and losing it.
                    tracing::warn!(
                        address = %event.address,
                        state = %existing.state,
                        "spawn target is already live; delivering this request's input into it \
                         instead of forking a second run"
                    );
                    return deliver_race_guard_content(&ctx.session_registry, event);
                }
                SessionState::Completed => {}
            }
        }
        fork_and_spawn(ctx, event).await
    })
}

/// Deliver a spawn/resume request's content into the run that already won
/// the race for its address, using [`race_guard_interrupt`] to pick the
/// right interrupt kind.
///
/// # Errors
///
/// Returns an error if delivery fails (the winning run's channel is
/// saturated, its own teardown is underway, or it has already left the
/// registry) — the content is dropped in that case.
fn deliver_race_guard_content(
    registry: &SessionRegistry,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let address = event.address.clone();
    let content = event.kickoff_text();
    let category = SessionCategory::from_trigger(&event.source);
    let interrupt = race_guard_interrupt(
        &event.source,
        event.inbound,
        event.spawner,
        category,
        content,
        event.hop_count,
    );
    match registry.deliver(&address, interrupt) {
        DeliverOutcome::Delivered => Ok(()),
        other @ (DeliverOutcome::Completing | DeliverOutcome::Full | DeliverOutcome::NotLive) => {
            anyhow::bail!(
                "session address {address} is already live and delivering this request's input \
                 into it failed ({other:?}); input dropped"
            );
        }
    }
}

/// Build the interrupt a spawn/resume request's content becomes when its
/// target address has already been won by another attempt (see
/// [`handle_spawn_request`]'s `Forking | Running | Idle` branch, and
/// `crate::background::runtime::deliver_losing_spawn_input`, which hits the
/// same situation from the narrower race window between a spawn request
/// clearing that guard and actually registering).
///
/// A `Conversation`-triggered request carrying its original inbound message
/// renders as [`Interrupt::UserMessage`] — the same sender-attributed
/// rendering ordinary mid-turn/idle conversation delivery gets (hop `0`,
/// `msg.into_history_messages()`), rather than an [`Interrupt::AgentMessage`]
/// that would show the receiving session "[Agent Message from main]" for
/// what is really a participant's own message. Everything else — including
/// a `Conversation` request with no inbound message, which the invariant in
/// `AgentMessenger::deliver_conversation`/`publish_conversation_spawn`/
/// `publish_conversation_resume` says should never happen — falls back to
/// `Interrupt::AgentMessage`, attributed to `spawner` (or `main` if it has
/// none).
pub(crate) fn race_guard_interrupt(
    trigger: &EventTrigger,
    inbound: Option<InboundMessage>,
    spawner: Option<SessionAddress>,
    category: SessionCategory,
    content: String,
    hop_count: u32,
) -> Interrupt {
    if matches!(trigger, EventTrigger::Conversation)
        && let Some(inbound) = inbound
    {
        return Interrupt::UserMessage(inbound);
    }
    Interrupt::AgentMessage(AgentMessageEvent {
        from: spawner.unwrap_or_else(|| SessionAddress::from(MAIN_ADDRESS)),
        from_category: category.as_str().to_string(),
        content,
        hop_count,
    })
}

/// Build resources and hand the run to `SessionRuntime`, once
/// `handle_spawn_request` has confirmed the address is free to take it.
async fn fork_and_spawn(
    ctx: &Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let skill = event.skill.as_ref().map(|s| s.as_ref().to_string());
    let category = SessionCategory::from_trigger(&event.source);

    let resources = build_spawn_resources(
        ctx,
        &event.model_tier,
        skill.as_deref(),
        crate::background::spawn_context::NewSessionContext {
            own_address: event.address.clone(),
            own_depth: event.depth,
            category,
            hop_count: event.hop_count,
            trigger: event.source.clone(),
            conversation_target: event.conversation.clone(),
        },
    )
    .await?;

    let request = SessionSpawnRequest {
        address: event.address,
        source_label: event.source_label,
        trigger: event.source,
        agent_skill: event.skill,
        spawner: event.spawner,
        depth: event.depth,
        subagent_config: SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: event.model_tier,
            hop_count: event.hop_count,
            sender: event.sender,
            inbound: event.inbound,
            images: event.images,
        },
        conversation_target: event.conversation,
    };

    let log_address = request.address.clone();
    let log_source_label = request.source_label.clone();
    ctx.session_runtime.spawn(request, Some(resources));
    tracing::info!(
        skill = skill.as_deref().unwrap_or("none"),
        address = %log_address,
        source = %log_source_label,
        "forked session"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::SessionInfo;
    use crate::bus::ConversationTarget;
    use crate::config::BackgroundModelTier;
    use crate::inference::MessageSender;
    use crate::interfaces::types::{ConversationContext, ConversationKind, MessageOrigin};
    use tokio_util::sync::CancellationToken;

    fn sample_inbound(id: &str, content: &str, sender_name: &str) -> InboundMessage {
        InboundMessage {
            id: id.to_string(),
            content: content.to_string(),
            origin: MessageOrigin {
                endpoint: "discord".to_string(),
                sender: Some(MessageSender {
                    name: sender_name.to_string(),
                    id: "discord-jane".to_string(),
                    interface: "discord".to_string(),
                    location: Some("#builds".to_string()),
                }),
                conversation: Some(ConversationContext {
                    id: "chan-1".to_string(),
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

    fn conversation_spawn_event(inbound: InboundMessage, address: &str) -> SpawnRequestEvent {
        SpawnRequestEvent {
            address: SessionAddress::from(address),
            skill: None,
            source_label: "discord:#builds".to_string(),
            prompt: inbound.content.clone(),
            context: inbound.context.clone(),
            source: EventTrigger::Conversation,
            model_tier: BackgroundModelTier::Medium,
            spawner: None,
            depth: 1,
            hop_count: 0,
            sender: inbound.origin.sender.clone(),
            conversation: Some(ConversationTarget {
                endpoint: inbound.origin.endpoint.clone(),
                conversation_id: "chan-1".to_string(),
            }),
            images: inbound.images.clone(),
            inbound: Some(inbound),
        }
    }

    fn sample_live_conversation_info(address: &str) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-winner".to_string(),
            category: SessionCategory::External,
            trigger: EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: SessionState::Idle,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model_tier: BackgroundModelTier::Medium,
            conversation_target: Some(ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
        }
    }

    #[test]
    fn race_guard_interrupt_for_a_conversation_trigger_carries_sender_attribution() {
        let inbound = sample_inbound("m2", "any updates?", "Jane");
        let interrupt = race_guard_interrupt(
            &EventTrigger::Conversation,
            Some(inbound),
            None,
            SessionCategory::External,
            "ignored fallback content".to_string(),
            0,
        );
        match interrupt {
            Interrupt::UserMessage(m) => {
                assert_eq!(m.content, "any updates?");
                assert_eq!(m.origin.sender.map(|s| s.name), Some("Jane".to_string()));
            }
            Interrupt::AgentMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!(
                    "expected a UserMessage interrupt for a conversation trigger carrying an inbound message"
                )
            }
        }
    }

    #[test]
    fn race_guard_interrupt_without_an_inbound_message_falls_back_to_agent_message() {
        // A `Conversation`-triggered request should always carry an inbound
        // message (the invariant `publish_conversation_spawn`/
        // `publish_conversation_resume` uphold) — but if it somehow doesn't,
        // this must still degrade to a delivered agent message rather than
        // panicking or dropping the content.
        let interrupt = race_guard_interrupt(
            &EventTrigger::Conversation,
            None,
            Some(SessionAddress::from(MAIN_ADDRESS)),
            SessionCategory::External,
            "hello".to_string(),
            0,
        );
        match interrupt {
            Interrupt::AgentMessage(m) => assert_eq!(m.content, "hello"),
            Interrupt::UserMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!("expected an agent message fallback when no inbound message is carried")
            }
        }
    }

    #[test]
    fn race_guard_interrupt_for_a_non_conversation_trigger_is_always_an_agent_message() {
        let inbound = sample_inbound("m1", "irrelevant", "Jane");
        let interrupt = race_guard_interrupt(
            &EventTrigger::Agent,
            Some(inbound),
            Some(SessionAddress::from(MAIN_ADDRESS)),
            SessionCategory::Spawned,
            "task update".to_string(),
            2,
        );
        match interrupt {
            Interrupt::AgentMessage(m) => {
                assert_eq!(m.content, "task update");
                assert_eq!(m.hop_count, 2);
            }
            Interrupt::UserMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!("an agent-triggered race guard must never render as a UserMessage")
            }
        }
    }

    #[tokio::test]
    async fn two_conversation_messages_to_a_brand_new_address_back_to_back_deliver_the_second_with_attribution()
     {
        // Regression test for the race-guard misattribution bug: when a
        // second spawn/resume request for the same address loses the race
        // (the first already registered the one live run), its content must
        // reach that run as `Interrupt::UserMessage` carrying the
        // participant's own sender attribution — never "[Agent Message from
        // main]".
        let registry = SessionRegistry::new();
        let address = "external-discord-chan-1";
        let info = sample_live_conversation_info(address);
        let mut rx = registry
            .register(info, CancellationToken::new())
            .expect("the first message wins the fork and registers the one live run");

        let second_msg = sample_inbound("m2", "any updates?", "Jane");
        let second_event = conversation_spawn_event(second_msg, address);
        deliver_race_guard_content(&registry, second_event)
            .expect("delivering into the live run must succeed");

        let delivered = rx
            .try_recv()
            .expect("the second message should be delivered into the one run that exists");
        match delivered {
            Interrupt::UserMessage(m) => {
                assert_eq!(m.content, "any updates?");
                assert_eq!(
                    m.origin.sender.map(|s| s.name),
                    Some("Jane".to_string()),
                    "the delivered message must carry the participant's own sender attribution"
                );
            }
            Interrupt::AgentMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!(
                    "expected the second conversation message delivered as a UserMessage with \
                     sender attribution, not misattributed as an agent message from main"
                )
            }
        }
        assert!(
            rx.try_recv().is_err(),
            "only one run should ever exist for the address — no second run's spawn ever \
             reaches the interrupt channel"
        );
    }
}

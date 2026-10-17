//! Bus listener that turns spawn requests into running sessions.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the run to `SessionRuntime`.
//! Everything the session runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::fork_failure::{owner_start_failure_message, pulse_start_failure_message};
use crate::registry::{DeliverOutcome, SessionRegistry};
use crate::runtime::SessionSpawnRequest;
use crate::spawn_context::{SpawnContext, build_spawn_resources};
use crate::types::SubAgentConfig;
use residuum_bus::MessageEvent;
use residuum_bus::interrupt::Interrupt;
use residuum_bus::session::{MAIN_ADDRESS, SessionCategory, SessionState};
use residuum_bus::{
    AgentMessageEvent, BusHandle, EventTrigger, NoticeEvent, NotifyName, Publisher, SYSTEM_CHANNEL,
    SessionAddress, SessionStartFailedEvent, SpawnRequestEvent, Subscriber, topics,
};

/// Subscribe to the `Background` topic and fork sessions on demand.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the subscriber closes.
#[tracing::instrument(skip_all)]
pub async fn spawn_listener(
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

    Some(residuum_core::util::spawn_in_span(listener_loop(
        ctx, subscriber,
    )))
}

/// Main loop: reads spawn requests and executes them.
///
/// `spawn_failures` dedupes fork failures for pulse/action triggers (see
/// [`handle_spawn_failure`]) across the whole life of this loop — those are
/// the triggers that retry automatically (a pulse re-fires on its schedule,
/// a due action is kept and retried per `spawn_due_actions`), so a
/// persistently broken one (e.g. naming a skill that doesn't exist) would
/// otherwise warn-log and notice on every single retry forever.
async fn listener_loop(ctx: Arc<SpawnContext>, mut subscriber: Subscriber<SpawnRequestEvent>) {
    let mut spawn_failures: HashMap<String, String> = HashMap::new();
    loop {
        match subscriber.recv().await {
            Ok(Some(event)) => {
                let source_label = event.source_label.clone();
                let trigger = event.source.clone();
                let pulse_address =
                    matches!(event.source, EventTrigger::Pulse).then(|| event.address.clone());
                let owner_start = super::owner_session::is_owner_start(&event).then(|| {
                    let resume = ctx.session_registry.resume_point(&event.address).is_some();
                    (event.address.clone(), resume)
                });
                let outcome = handle_spawn_request(&ctx, event).await;
                if let (Err(e), Some(address)) = (&outcome, pulse_address) {
                    publish_pulse_start_failed(&ctx.publisher, address, &source_label, e).await;
                }
                match outcome {
                    Ok(()) => {
                        if spawn_failures.remove(&source_label).is_some() {
                            tracing::info!(
                                source = %source_label,
                                "session fork recovered after previously failing"
                            );
                        }
                    }
                    Err(e) => match owner_start {
                        Some((address, resume)) => {
                            notify_owner_start_failed(&ctx.publisher, &address, resume, &e).await;
                        }
                        None => {
                            handle_spawn_failure(
                                &ctx.publisher,
                                &mut spawn_failures,
                                &trigger,
                                &source_label,
                                &e,
                            )
                            .await;
                        }
                    },
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

/// Tell the pulse fire waiting for its session at `address` to register that
/// it never will, so the fire ends as failed instead of showing as running.
/// The event carries a plain-language sentence for the Schedule place and
/// the full cause chain for the logs.
async fn publish_pulse_start_failed(
    publisher: &Publisher,
    address: SessionAddress,
    source_label: &str,
    error: &anyhow::Error,
) {
    let pulse = source_label
        .split_once(':')
        .map_or(source_label, |(_, name)| name);
    let event = SessionStartFailedEvent {
        address,
        error: pulse_start_failure_message(pulse, error),
        error_details: Some(format!("{error:#}")),
        owner_start: false,
    };
    if let Err(e) = publisher.publish(topics::Sessions, event).await {
        tracing::warn!(source = %source_label, error = %e, "failed to publish a pulse session's start failure");
    }
}

/// Record a spawn failure, deduplicated for scheduled triggers (pulses and
/// actions): logs `warn` and publishes an owner-facing notice only the
/// first time a given source label fails, or when the error text changes
/// from what was last reported for it; an unchanged, still-failing retry
/// logs at `debug` instead and publishes nothing. Every other trigger
/// (spawned/webhook/conversation/artifact) isn't retried automatically the
/// same way, so it keeps warning on every occurrence, same as before.
async fn handle_spawn_failure(
    publisher: &Publisher,
    spawn_failures: &mut HashMap<String, String>,
    trigger: &EventTrigger,
    source_label: &str,
    error: &anyhow::Error,
) {
    let message = format!("{error:#}");

    if !matches!(trigger, EventTrigger::Pulse | EventTrigger::Action) {
        tracing::warn!(source = %source_label, error = %message, "failed to fork session");
        return;
    }

    if spawn_failures.get(source_label) == Some(&message) {
        tracing::debug!(
            source = %source_label,
            error = %message,
            "session fork still failing with the same error"
        );
        return;
    }

    tracing::warn!(source = %source_label, error = %message, "failed to fork session");
    spawn_failures.insert(source_label.to_string(), message.clone());

    let kind = match trigger {
        EventTrigger::Pulse => "pulse",
        EventTrigger::Action => "scheduled action",
        // The caller only reaches here for Pulse/Action; every other
        // trigger keeps a generic label rather than a wildcard match, so a
        // future trigger variant doesn't silently fall through unnoticed.
        EventTrigger::Agent
        | EventTrigger::Webhook(_)
        | EventTrigger::Conversation
        | EventTrigger::Artifact(_) => "scheduled run",
    };
    let name = source_label
        .split_once(':')
        .map_or(source_label, |(_, n)| n);
    let notice = format!("Your {kind} \"{name}\" couldn't start: {message}");
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent::new(notice),
        )
        .await
    {
        tracing::warn!(source = %source_label, error = %e, "failed to publish spawn-failure notice");
    }
}

/// Tell the owner a session they started by hand couldn't start, or, when
/// `resume` is set, couldn't resume for the message sent to it. The start or
/// the message was already accepted (the web UI is waiting for the run to
/// appear, or a chat command already named its address), so the failure has
/// to reach them on its own: as a `SessionStartFailedEvent` the web UI
/// matches to the start by address, and as a notice. Both carry the same
/// plain-language sentence; the event also carries the full cause chain.
async fn notify_owner_start_failed(
    publisher: &Publisher,
    address: &SessionAddress,
    resume: bool,
    error: &anyhow::Error,
) {
    let details = format!("{error:#}");
    tracing::warn!(
        address = %address,
        resume,
        error = %details,
        "failed to fork a session the owner started"
    );
    let message = owner_start_failure_message(address, resume, error);
    if let Err(e) = publisher
        .publish(
            topics::Sessions,
            SessionStartFailedEvent {
                address: address.clone(),
                error: message.clone(),
                error_details: Some(details),
                owner_start: true,
            },
        )
        .await
    {
        tracing::warn!(address = %address, error = %e, "failed to publish an owner session's start failure");
    }
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent::new(message),
        )
        .await
    {
        tracing::warn!(address = %address, error = %e, "failed to publish an owner session's start-failure notice");
    }
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
                    let owner_address =
                        super::owner_session::is_owner_start(&event).then(|| event.address.clone());
                    residuum_core::util::spawn_in_span(async move {
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
                        let pulse_address = matches!(event.source, EventTrigger::Pulse)
                            .then(|| event.address.clone());
                        let source_label = event.source_label.clone();
                        let resume = ctx.session_registry.resume_point(&event.address).is_some();
                        if let Err(e) = handle_spawn_request(&ctx, event).await {
                            if let Some(address) = pulse_address {
                                publish_pulse_start_failed(
                                    &ctx.publisher,
                                    address,
                                    &source_label,
                                    &e,
                                )
                                .await;
                            }
                            if let Some(address) = owner_address {
                                notify_owner_start_failed(&ctx.publisher, &address, resume, &e)
                                    .await;
                            } else {
                                tracing::warn!(
                                    error = format!("{e:#}"),
                                    "failed to fork deferred session resume"
                                );
                            }
                        }
                    });
                    return Ok(());
                }
                SessionState::Forking
                | SessionState::Queued
                | SessionState::Running
                | SessionState::Idle => {
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
                    return deliver_race_guard_content(
                        &ctx.session_registry,
                        &ctx.publisher,
                        event,
                    );
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
/// A chat user's message (a `Conversation`-triggered request carrying its
/// original inbound message, which [`race_guard_interrupt`] renders as
/// [`Interrupt::UserMessage`]) is never dropped here: a saturated or
/// torn-down channel hands off to a detached retry task (see
/// [`super::messaging::retry_race_guard_user_message`]) instead of
/// erroring. An agent-to-agent message in the same situation still errors
/// as before — an unusual double-spawn race, with a live agent on the
/// other end able to notice and retry itself.
///
/// # Errors
///
/// Returns an error if delivering an *agent* message fails (the winning
/// run's channel is saturated, its own teardown is underway, or it has
/// already left the registry) — that content is dropped in that case.
fn deliver_race_guard_content(
    registry: &Arc<SessionRegistry>,
    publisher: &Publisher,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let address = event.address.clone();
    let content = event.kickoff_text();
    let category = SessionCategory::from_trigger(&event.source);
    let interrupt = race_guard_interrupt(
        &event.source,
        event.inbound.clone(),
        event.spawner.clone(),
        category,
        content,
        event.hop_count,
    );
    // `race_guard_interrupt` only ever renders `Interrupt::UserMessage` when
    // `event.inbound` is `Some` — recovered here rather than moved above so
    // it's still available both for the retry task's own redelivery
    // attempts and as part of `event` if a fresh spawn/resume is needed.
    let retryable_inbound = if matches!(interrupt, Interrupt::UserMessage(_)) {
        event.inbound.clone()
    } else {
        None
    };
    match registry.deliver(&address, interrupt) {
        DeliverOutcome::Delivered => Ok(()),
        outcome @ (DeliverOutcome::Completing | DeliverOutcome::Full | DeliverOutcome::NotLive)
            if let Some(inbound) = retryable_inbound =>
        {
            tracing::warn!(
                address = %address,
                ?outcome,
                "race-guard user message could not be delivered immediately; retrying on a \
                 detached task rather than dropping it"
            );
            let registry = Arc::clone(registry);
            let publisher = publisher.clone();
            residuum_core::util::spawn_in_span(async move {
                super::messaging::retry_race_guard_user_message(
                    registry, publisher, address, inbound, event,
                )
                .await;
            });
            Ok(())
        }
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
/// `crate::runtime::deliver_losing_spawn_input`, which hits the
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
    inbound: Option<MessageEvent>,
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

    let spawned = build_spawn_resources(
        ctx,
        &event.model,
        skill.as_deref(),
        crate::spawn_context::NewSessionContext {
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
            model: event.model,
            resolved_model: Some(spawned.resolved_model.clone()),
            hop_count: event.hop_count,
            sender: event.sender,
            inbound: event.inbound,
            images: event.images,
            carried_history: event.carried_history,
        },
        conversation_target: event.conversation,
        overlap: event.overlap,
    };

    let log_address = request.address.clone();
    let log_source_label = request.source_label.clone();
    ctx.session_runtime.spawn(request, Some(spawned.resources));
    tracing::info!(
        skill = skill.as_deref().unwrap_or("none"),
        model = %spawned.resolved_model,
        address = %log_address,
        source = %log_source_label,
        "forked session"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use residuum_bus::ConversationTarget;
    use residuum_bus::origin::{ConversationContext, ConversationKind, MessageOrigin};
    use residuum_bus::session::SessionInfo;
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_config::ModelSelector;
    use residuum_core::testing::wait;
    use residuum_inference::MessageSender;
    use tokio_util::sync::CancellationToken;

    fn sample_inbound(id: &str, content: &str, sender_name: &str) -> MessageEvent {
        MessageEvent {
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
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            context: None,
        }
    }

    fn conversation_spawn_event(inbound: MessageEvent, address: &str) -> SpawnRequestEvent {
        SpawnRequestEvent {
            address: SessionAddress::from(address),
            skill: None,
            source_label: "discord:#builds".to_string(),
            prompt: inbound.content.clone(),
            context: inbound.context.clone(),
            source: EventTrigger::Conversation,
            model: ModelSelector::MEDIUM,
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
            overlap: None,
            carried_history: Vec::new(),
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
            model: ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: chrono::Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
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
        let registry = Arc::new(SessionRegistry::new());
        let publisher = Publisher::noop();
        let address = "external-discord-chan-1";
        let info = sample_live_conversation_info(address);
        let mut rx = registry
            .register(info, CancellationToken::new())
            .expect("the first message wins the fork and registers the one live run");

        let second_msg = sample_inbound("m2", "any updates?", "Jane");
        let second_event = conversation_spawn_event(second_msg, address);
        deliver_race_guard_content(&registry, &publisher, second_event)
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

    #[tokio::test]
    async fn a_saturated_race_guard_still_delivers_the_user_message_via_retry() {
        // Regression: a race-guard user message used to be dropped outright
        // ("input dropped") when the winning run's channel was saturated —
        // the same silent-drop bug fixed for the primary conversation path.
        // There's nobody on the other end of a chat message to hand a
        // refusal to, so this must retry until the channel drains, exactly
        // the way a live session's own tool loop drains it continuously.
        let registry = Arc::new(SessionRegistry::new());
        let publisher = Publisher::noop();
        let address = "external-discord-chan-2";
        let info = sample_live_conversation_info(address);
        let mut rx = registry
            .register(info, CancellationToken::new())
            .expect("the winner registers the one live run");

        for _ in 0..crate::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(matches!(
                registry.deliver(
                    &SessionAddress::from(address),
                    Interrupt::UserMessage(sample_inbound("filler", "filler", "Jane")),
                ),
                DeliverOutcome::Delivered
            ));
        }

        let new_msg = sample_inbound("the-new-message", "can anyone see this?", "Jane");
        let new_event = conversation_spawn_event(new_msg, address);
        deliver_race_guard_content(&registry, &publisher, new_event)
            .expect("a saturated channel must hand off to a retry, not error");

        let interrupt = wait::next_matching(
            "the race-guard user message to be delivered, never dropped",
            &mut rx,
            |interrupt| matches!(interrupt, Interrupt::UserMessage(m) if m.id == "the-new-message"),
        )
        .await;
        match interrupt {
            Interrupt::UserMessage(m) => assert_eq!(m.content, "can anyone see this?"),
            Interrupt::AgentMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!("the race-guard user message should be the one delivered")
            }
        }
    }

    #[tokio::test]
    async fn a_race_guard_user_message_republishes_a_spawn_request_once_its_target_finishes() {
        // The `NotLive` case: the winning run finishes (and is removed from
        // the registry) before this content can be delivered into it. The
        // message must still not be dropped — it gets a fresh spawn/resume
        // attempt through the ordinary listener pipeline instead.
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let registry = Arc::new(SessionRegistry::new());
        let address = "external-discord-chan-3";
        let info = sample_live_conversation_info(address);
        registry
            .register(info, CancellationToken::new())
            .expect("the winner registers the one live run");
        registry.remove(&SessionAddress::from(address), "run-winner");

        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let new_msg = sample_inbound("the-new-message", "can anyone see this?", "Jane");
        let new_event = conversation_spawn_event(new_msg, address);
        deliver_race_guard_content(&registry, &publisher, new_event)
            .expect("a target that just finished must hand off to a retry, not error");

        let republished = next_event("a fresh spawn request to be republished", &mut spawns).await;
        assert_eq!(republished.address, SessionAddress::from(address));
        assert_eq!(
            republished.inbound.map(|m| m.content),
            Some("can anyone see this?".to_string()),
            "the republished request must still carry the original user message"
        );
    }

    #[tokio::test]
    async fn a_pulse_start_failure_names_the_address_in_plain_words_with_the_chain() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();

        let error = Err::<(), _>(anyhow::anyhow!("not in the skill index"))
            .context(crate::fork_failure::ForkFailure::Skill {
                name: "ghost".to_string(),
            })
            .unwrap_err();
        publish_pulse_start_failed(
            &bus.publisher(),
            SessionAddress::from("scheduled-email-check-0a1b"),
            "pulse:email_check",
            &error,
        )
        .await;

        let failed = next_event("the start failure", &mut sub).await;
        assert_eq!(
            failed,
            SessionStartFailedEvent {
                address: SessionAddress::from("scheduled-email-check-0a1b"),
                error: "The pulse \"email_check\" couldn't start its session because the skill \
                        \"ghost\" couldn't be loaded. Check that the skill is still installed."
                    .to_string(),
                error_details: Some(
                    "failed to activate skill 'ghost' for sub-agent: not in the skill index"
                        .to_string()
                ),
                owner_start: false,
            },
            "a plain sentence for the Schedule place, the chain for the logs, and never the \
             web UI's start map"
        );
    }

    // ── handle_spawn_failure dedup ───────────────────────────────────────

    #[tokio::test]
    async fn first_pulse_spawn_failure_publishes_a_notice() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &anyhow::anyhow!("skill 'ghost' not found"),
        )
        .await;

        let notice = next_event("the first failure's notice", &mut sub).await;
        assert!(notice.message.contains("email_check"));
        assert!(notice.message.contains("skill 'ghost' not found"));
    }

    #[tokio::test]
    async fn a_pulse_spawn_failure_notice_says_why_under_the_failed_step() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();
        let error = Err::<(), _>(anyhow::anyhow!("provider 'lcoal' is not configured"))
            .context(crate::fork_failure::ForkFailure::UnknownModel {
                model: "lcoal/qwen3:8b".to_string(),
            })
            .unwrap_err();

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &error,
        )
        .await;

        let notice = next_event("the failure's notice", &mut sub).await;
        assert_eq!(
            notice.message,
            "Your pulse \"email_check\" couldn't start: model 'lcoal/qwen3:8b' can't be used: \
             provider 'lcoal' is not configured"
        );
    }

    #[tokio::test]
    async fn an_owner_session_that_fails_to_start_tells_the_owner_which_one() {
        let bus = residuum_bus::spawn_broker();
        let mut notices: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let error = Err::<(), _>(anyhow::anyhow!("no API key for the medium tier"))
            .context(crate::fork_failure::ForkFailure::ProviderChain {
                model: "medium".to_string(),
            })
            .unwrap_err();

        notify_owner_start_failed(
            &bus.publisher(),
            &SessionAddress::from("spawned-multitask-1a2b"),
            false,
            &error,
        )
        .await;

        let failure = next_event("the owner's start failure", &mut failures).await;
        assert_eq!(
            failure.address,
            SessionAddress::from("spawned-multitask-1a2b")
        );
        assert_eq!(
            failure.error,
            crate::fork_failure::owner_start_failure_message(&failure.address, false, &error)
        );
        assert_eq!(
            failure.error_details.as_deref(),
            Some("failed to build provider chain for model medium: no API key for the medium tier"),
            "the details carry the whole cause chain"
        );
        let notice = next_event("the owner's failure notice", &mut notices).await;
        assert_eq!(
            notice.message, failure.error,
            "the notice says the same plain sentence as the frame"
        );
    }

    // ── owner start failures through the listener ───────────────────────

    struct NoSessionTools;

    impl crate::session_tools::SessionToolFactory for NoSessionTools {
        fn build(
            &self,
            _scope: crate::session_tools::SessionToolScope,
        ) -> residuum_agent::ToolRegistry {
            residuum_agent::ToolRegistry::new()
        }
    }

    /// A spawn context on `bus` whose every tier falls back to one Anthropic
    /// model with no API key, so every fork fails building its provider chain.
    async fn spawn_context_without_a_working_provider(bus: &BusHandle) -> Arc<SpawnContext> {
        let (session_runtime, _results, _runtime_bus) =
            crate::testing::test_runtime_held_idle().await;
        let (layout, observer, merge_writer) = crate::testing::test_memory_extras();
        let registry = Arc::new(SessionRegistry::new());
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::store::SessionStore::new(store_dir.path().to_path_buf())
                .with_tempdir_guard(store_dir),
        );
        let messenger = Arc::new(crate::messaging::AgentMessenger::new(
            Arc::clone(&registry),
            bus.publisher(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let kind = residuum_config::ProviderKind::Anthropic;
        let keyless = residuum_config::ProviderSpec {
            name: kind.to_string(),
            model: residuum_config::ModelSpec {
                kind,
                model: "claude-test".to_string(),
            },
            provider_url: kind.default_url().to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input: residuum_config::ImageInput::Undeclared,
        };
        Arc::new(SpawnContext {
            background_config: residuum_config::BackgroundConfig::default(),
            main_provider_specs: vec![keyless],
            model_catalog: Arc::new(tokio::sync::RwLock::new(
                residuum_config::ModelCatalog::default(),
            )),
            http_client: residuum_inference::SharedHttpClient::new(
                &residuum_inference::HttpClientConfig::default(),
            )
            .unwrap(),
            max_tokens: 1024,
            retry_config: residuum_config::RetryConfig::default(),
            options: residuum_inference::CompletionOptions::default(),
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            layout,
            role_overrides: HashMap::new(),
            session_tools: Arc::new(NoSessionTools),
            session_runtime: Arc::new(session_runtime),
            session_registry: registry,
            publisher: bus.publisher(),
            skill_state: residuum_skills::SkillState::new_shared(
                residuum_skills::SkillIndex::default(),
                vec![],
            ),
            mcp_registry: residuum_mcp::McpRegistry::new_shared(),
            observer,
            merge_writer,
            messenger,
            bg_tier_active_index: crate::spawn_context::BackgroundTierActiveIndex::default(),
        })
    }

    fn owner_start() -> SpawnRequestEvent {
        crate::owner_session::OwnerSessionStart {
            prompt: "summarize the build logs".to_string(),
            model: ModelSelector::MEDIUM,
            fork_of: None,
            conversation: None,
        }
        .into_spawn_event()
    }

    /// A resume point at `address`, as a finished run of an owner session
    /// leaves.
    async fn record_finished_owner_run(registry: &SessionRegistry, address: &SessionAddress) {
        registry
            .record_resume_point(
                address,
                crate::registry::ResumePoint {
                    previous_run_id: "run-previous".to_string(),
                    previous_episode_id: None,
                    trigger: EventTrigger::Agent,
                    source_label: "owner:session".to_string(),
                    agent_skill: None,
                    model: ModelSelector::MEDIUM,
                    spawner: None,
                    depth: 1,
                    conversation_target: None,
                    recorded_at: chrono::Utc::now(),
                },
            )
            .await;
    }

    const FIRST_START: &str = "couldn't start";
    const RESUME: &str = "couldn't pick up your message";

    fn assert_is_the_provider_failure(
        failure: &SessionStartFailedEvent,
        address: &SessionAddress,
        failed: &str,
    ) {
        assert_eq!(&failure.address, address);
        assert!(
            failure.error.starts_with(&format!(
                "The session {address} {failed} because its model (medium) couldn't be set up"
            )),
            "{}",
            failure.error
        );
        let details = failure.error_details.as_deref().unwrap_or_default();
        assert!(
            details.starts_with("failed to build provider chain for model medium: ")
                && details.contains("API key"),
            "the details carry the cause under the step: {details}"
        );
    }

    #[tokio::test]
    async fn an_owner_start_that_fails_to_fork_publishes_its_start_failure() {
        let bus = residuum_bus::spawn_broker();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let mut notices: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let ctx = spawn_context_without_a_working_provider(&bus).await;
        let _listener = spawn_listener(ctx, &bus).await.unwrap();

        let start = owner_start();
        let address = start.address.clone();
        bus.publisher()
            .publish(topics::Background, start)
            .await
            .unwrap();

        let failure = next_event("the owner start's failure", &mut failures).await;
        assert_is_the_provider_failure(&failure, &address, FIRST_START);
        let notice = next_event("the owner start's failure notice", &mut notices).await;
        assert_eq!(notice.message, failure.error);
    }

    #[tokio::test]
    async fn an_owner_session_resume_that_fails_to_fork_says_the_message_wasnt_picked_up() {
        let bus = residuum_bus::spawn_broker();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let ctx = spawn_context_without_a_working_provider(&bus).await;
        let resume = owner_start();
        let address = resume.address.clone();
        record_finished_owner_run(&ctx.session_registry, &address).await;
        let _listener = spawn_listener(ctx, &bus).await.unwrap();

        bus.publisher()
            .publish(topics::Background, resume)
            .await
            .unwrap();

        let failure = next_event("the owner resume's failure", &mut failures).await;
        assert_is_the_provider_failure(&failure, &address, RESUME);
    }

    #[tokio::test]
    async fn a_deferred_owner_start_that_fails_to_fork_publishes_its_start_failure() {
        let bus = residuum_bus::spawn_broker();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let ctx = spawn_context_without_a_working_provider(&bus).await;
        let start = owner_start();
        let address = start.address.clone();
        let mut previous = sample_live_conversation_info(address.as_ref());
        previous.run_id = "run-previous".to_string();
        let _interrupts = ctx
            .session_registry
            .register(previous, CancellationToken::new())
            .unwrap();
        ctx.session_registry
            .set_state(&address, SessionState::Completing);

        handle_spawn_request(&ctx, start)
            .await
            .expect("a start at a completing address is deferred, not refused");
        ctx.session_registry.remove(&address, "run-previous");

        let failure = next_event("the deferred owner start's failure", &mut failures).await;
        assert_is_the_provider_failure(&failure, &address, FIRST_START);
    }

    #[tokio::test]
    async fn a_deferred_owner_session_resume_that_fails_to_fork_says_the_message_wasnt_picked_up() {
        let bus = residuum_bus::spawn_broker();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let ctx = spawn_context_without_a_working_provider(&bus).await;
        let resume = owner_start();
        let address = resume.address.clone();
        let mut previous = sample_live_conversation_info(address.as_ref());
        previous.run_id = "run-previous".to_string();
        let _interrupts = ctx
            .session_registry
            .register(previous, CancellationToken::new())
            .unwrap();
        ctx.session_registry
            .set_state(&address, SessionState::Completing);

        handle_spawn_request(&ctx, resume)
            .await
            .expect("a resume at a completing address is deferred, not refused");
        // The run finishing records its resume point before it leaves the
        // registry.
        record_finished_owner_run(&ctx.session_registry, &address).await;
        ctx.session_registry.remove(&address, "run-previous");

        let failure = next_event("the deferred owner resume's failure", &mut failures).await;
        assert_is_the_provider_failure(&failure, &address, RESUME);
    }

    #[tokio::test]
    async fn a_start_an_agent_made_publishes_no_start_failure() {
        let bus = residuum_bus::spawn_broker();
        let mut failures: Subscriber<SessionStartFailedEvent> =
            bus.subscribe(topics::Sessions).await.unwrap();
        let ctx = spawn_context_without_a_working_provider(&bus).await;
        let _listener = spawn_listener(ctx, &bus).await.unwrap();

        let mut by_main = owner_start();
        by_main.spawner = Some(SessionAddress::from(MAIN_ADDRESS));
        bus.publisher()
            .publish(topics::Background, by_main)
            .await
            .unwrap();
        // The listener handles requests in order, so once the owner's start
        // has failed, main's has been handled too.
        let owners = owner_start();
        let owners_address = owners.address.clone();
        bus.publisher()
            .publish(topics::Background, owners)
            .await
            .unwrap();

        let failure = next_event("the owner start's failure", &mut failures).await;
        assert_eq!(
            failure.address, owners_address,
            "only the owner's start reports a start failure"
        );
        bus_barrier(&bus).await;
        assert_eq!(failures.drain(), Vec::<SessionStartFailedEvent>::new());
    }

    #[tokio::test]
    async fn repeated_identical_pulse_failure_does_not_renotice() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();
        let err = anyhow::anyhow!("skill 'ghost' not found");

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &err,
        )
        .await;
        next_event("the first call's notice", &mut sub).await;

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &err,
        )
        .await;
        bus_barrier(&bus).await;
        assert!(
            sub.drain().is_empty(),
            "an unchanged, still-failing retry must not notice again"
        );
    }

    #[tokio::test]
    async fn pulse_failure_renotices_once_the_error_changes() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &anyhow::anyhow!("skill 'ghost' not found"),
        )
        .await;
        next_event("the first pulse failure's notice", &mut sub).await;

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Pulse,
            "pulse:email_check",
            &anyhow::anyhow!("a different failure now"),
        )
        .await;
        let notice = next_event("a genuinely new error's notice", &mut sub).await;
        assert!(notice.message.contains("a different failure now"));
    }

    #[tokio::test]
    async fn action_spawn_failure_notice_names_it_a_scheduled_action() {
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();

        handle_spawn_failure(
            &bus.publisher(),
            &mut failures,
            &EventTrigger::Action,
            "action:nightly digest",
            &anyhow::anyhow!("skill not found"),
        )
        .await;

        let notice = next_event("the scheduled action's notice", &mut sub).await;
        assert!(notice.message.contains("scheduled action"));
        assert!(notice.message.contains("nightly digest"));
    }

    #[tokio::test]
    async fn non_scheduled_trigger_never_publishes_a_notice() {
        // Spawned/webhook/conversation/artifact triggers aren't retried
        // automatically the way a pulse or action is, so this dedup+notice
        // mechanism is scoped to scheduled triggers only — every other
        // failure just keeps warning (unchanged from before).
        let bus = residuum_bus::spawn_broker();
        let mut sub: Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut failures = HashMap::new();

        for _ in 0..3 {
            handle_spawn_failure(
                &bus.publisher(),
                &mut failures,
                &EventTrigger::Agent,
                "agent:researcher",
                &anyhow::anyhow!("boom"),
            )
            .await;
        }

        bus_barrier(&bus).await;
        assert!(sub.drain().is_empty());
    }
}

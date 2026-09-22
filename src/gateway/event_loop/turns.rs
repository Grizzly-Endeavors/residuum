//! Agent turn handling and message processing in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::agent::context::{PromptContext, SkillsContext};
use crate::agent::interrupt::Interrupt;
use crate::bus::{
    EndpointCapabilities, EndpointId, EndpointName, ErrorEvent, MessageEvent, NotifyName,
    Publisher, ResponseEvent, SYSTEM_CHANNEL, Subscriber, TurnLifecycleEvent, topics,
};

use crate::gateway::types::{GatewayRuntime, ReloadSignal, StopRequest};
use crate::inference::ImageData;
use crate::interfaces::types::{InboundMessage, MessageOrigin};
use crate::memory::types::Visibility;
use crate::skills::SharedSkillState;

use crate::agent::context::loading::build_skill_context_strings;
use crate::gateway::memory::MemorySubsystems;

/// Raw prompt context strings for constructing a `PromptContext`.
///
/// Held as owned `Option<String>` so that `PromptContext` can borrow via `as_deref()`.
pub struct PromptContextStrings {
    pub skill_index: Option<String>,
    pub skill_active: Option<String>,
}

impl PromptContextStrings {
    /// Build a borrowed `PromptContext` from these owned strings.
    pub(super) fn as_prompt_context(&self) -> PromptContext<'_> {
        PromptContext {
            skills: SkillsContext {
                index: self.skill_index.as_deref(),
                active_instructions: self.skill_active.as_deref(),
            },
        }
    }
}

/// Load prompt context strings from skill state.
pub async fn load_prompt_context_strings(skill_state: &SharedSkillState) -> PromptContextStrings {
    let (skill_index, skill_active) = build_skill_context_strings(skill_state).await;
    PromptContextStrings {
        skill_index,
        skill_active,
    }
}

/// Process leftover interrupts that arrived during an agent turn but weren't consumed.
///
/// User messages are injected into the agent's conversation for the next turn.
pub fn process_leftover_interrupts(leftovers: Vec<Interrupt>, rt: &mut GatewayRuntime) {
    for intr in leftovers {
        match intr {
            Interrupt::UserMessage(leftover_msg) => {
                rt.agent.inject_inbound_message(leftover_msg);
            }
            Interrupt::AgentMessage(msg) => {
                rt.agent.inject_system_message(msg.format_for_agent());
            }
            Interrupt::Subconscious(content) => {
                // A late mid-turn finding degrades to a note for the next turn.
                rt.agent.inject_system_message(content);
            }
            Interrupt::Stopped => {
                // The turn already ended by the time this was drained — the
                // stop note was injected where it was actually observed
                // (mid-generation cancellation or the tool-loop checkpoint
                // inside `execute_turn`). Nothing left to do here.
                tracing::debug!("leftover stop marker drained after turn already ended");
            }
        }
    }
}

/// Drain remaining interrupts from an interrupt channel after a turn completes.
pub fn drain_interrupts(interrupt_rx: &mut mpsc::Receiver<Interrupt>) -> Vec<Interrupt> {
    let mut leftovers = Vec::new();
    while let Ok(intr) = interrupt_rx.try_recv() {
        leftovers.push(intr);
    }
    leftovers
}

/// Persist new messages and run observation if thresholds are exceeded.
pub async fn persist_and_maybe_observe(
    rt: &mut GatewayRuntime,
    new_messages: &[crate::inference::Message],
    visibility: Visibility,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    use crate::gateway::memory::{execute_observation, persist_and_check_thresholds};

    let action =
        persist_and_check_thresholds(new_messages, visibility, &rt.observer, &rt.layout, rt.tz)
            .await;
    if apply_observe_action(action, observe_deadline, rt.observer.cooldown_secs()) {
        let mem = MemorySubsystems {
            observer: &rt.observer,
            merge_writer: &rt.merge_writer,
            layout: &rt.layout,
            tz: rt.tz,
        };
        execute_observation(&mem, &mut rt.agent).await;
    }
}

/// Helper to apply observe action and update deadline.
fn apply_observe_action(
    action: crate::memory::observer::ObserveAction,
    observe_deadline: &mut Option<tokio::time::Instant>,
    cooldown_secs: u64,
) -> bool {
    use crate::memory::observer::ObserveAction;
    match action {
        ObserveAction::None => false,
        ObserveAction::StartCooldown => {
            *observe_deadline =
                Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs(cooldown_secs));
            false
        }
        ObserveAction::ForceNow => {
            *observe_deadline = None;
            true
        }
    }
}

/// Run an agent turn while monitoring interrupt sources (bus messages,
/// reload signals). Returns the turn result and any leftover interrupts
/// that arrived after the turn completed.
#[expect(
    clippy::too_many_arguments,
    reason = "publisher and topic params added during bus migration"
)]
async fn run_agent_turn_with_interrupts(
    agent: &mut Agent,
    agent_messenger: &crate::background::messaging::AgentMessenger,
    content: &str,
    publisher: &Publisher,
    output_endpoint: Option<&EndpointName>,
    tool_activity_endpoint: Option<&EndpointName>,
    correlation_id: &str,
    origin: Option<&MessageOrigin>,
    prompt_ctx: &PromptContext<'_>,
    images: &[ImageData],
    agent_subscriber: &mut Subscriber<MessageEvent>,
    reload_rx: &mut tokio::sync::watch::Receiver<ReloadSignal>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    subconscious: Option<Arc<crate::subconscious::Subconscious>>,
) -> (
    anyhow::Result<Vec<String>>,
    Vec<Interrupt>,
    Option<Arc<std::sync::Mutex<crate::subconscious::TurnScratch>>>,
) {
    // Cloned before the mutable borrow below (`agent.hop_counter()` borrows
    // `agent`, and `process_message` needs it mutably) — a clone still
    // refers to the same shared cell, so mid-turn bumps below and reads from
    // the `message_agent`/`subagent_spawn` tools stay in sync regardless.
    let hop_counter = agent.hop_counter().clone();
    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);
    let watch =
        subconscious.map(|s| crate::subconscious::SubconsciousWatch::new(s, interrupt_tx.clone()));
    // Cancelled to abort an in-flight model call immediately; a stop that
    // lands between calls is instead observed via `Interrupt::Stopped` at
    // the tool loop's checkpoint (see `execute_turn`).
    let stop_token = CancellationToken::new();
    let turn_result = {
        let mut turn = std::pin::pin!(agent.process_message(
            content,
            publisher,
            output_endpoint,
            tool_activity_endpoint,
            correlation_id,
            origin,
            prompt_ctx,
            &mut interrupt_rx,
            images,
            watch.as_ref(),
            &stop_token,
        ));
        loop {
            tokio::select! {
                result = &mut turn => break result,
                next_msg = agent_subscriber.recv() => {
                    match next_msg {
                        Ok(Some(msg_event)) => {
                            // A mid-turn message on this topic may be a
                            // genuine user message (hop 0) or an agent
                            // message relayed to main (see
                            // `AgentMessenger::deliver_to_main`) — either
                            // way, it's one more input driving this turn.
                            hop_counter.bump(agent_messenger.take_main_hop(&msg_event.id));
                            let inbound = crate::interfaces::types::InboundMessage {
                                id: msg_event.id,
                                content: msg_event.content,
                                origin: msg_event.origin,
                                timestamp: chrono::Utc::now(),
                                images: msg_event.images,
                                context: msg_event.context,
                            };
                            if interrupt_tx.try_send(Interrupt::UserMessage(inbound)).is_err() {
                                tracing::warn!("interrupt channel full, dropping user message mid-turn");
                            }
                        }
                        Ok(None) => {
                            tracing::debug!("agent subscriber closed during turn");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "type mismatch on user:message during turn");
                        }
                    }
                }
                _ = reload_rx.changed() => {
                    tracing::info!("reload signal received during active turn, deferring");
                }
                stop_req = stop_rx.recv() => {
                    let Some(req) = stop_req else {
                        tracing::debug!("stop request channel closed during turn");
                        continue;
                    };
                    let matches = req.reply_to.as_deref().is_none_or(|id| id == correlation_id);
                    if matches {
                        tracing::info!(correlation_id = %correlation_id, "stopping active turn");
                        stop_token.cancel();
                        if interrupt_tx.try_send(Interrupt::Stopped).is_err() {
                            tracing::warn!("interrupt channel full, stop marker dropped (model-call cancellation still applies)");
                        }
                    } else {
                        tracing::debug!(
                            requested = ?req.reply_to,
                            active = %correlation_id,
                            "ignoring stop request for a different or already-finished turn"
                        );
                    }
                    if let Some(tx) = req.result_tx {
                        tx.send(matches).ok();
                    }
                }
            }
        }
    };

    // Capture the mid-turn scratch before the watch drops so the end-of-turn
    // pass can triage against what already happened this turn.
    let scratch = watch
        .as_ref()
        .map(crate::subconscious::SubconsciousWatch::scratch);

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    (turn_result, leftover_interrupts, scratch)
}

/// Fallback learning trigger for users running without the subconscious
/// learning path: after a configured number of foreground turns, spawn the
/// `learner` sub-agent to review the recent conversation.
///
/// Skipped when the subconscious learning path is active (that path owns
/// spawning, so this avoids double-spawns) or when `nudge_after_turns` is zero.
/// Shares the subconscious learning cooldown.
async fn maybe_nudge_learner(rt: &mut GatewayRuntime) {
    // When the subconscious learning path is active it already spawns from
    // detected signals; the dumb fallback must stay out of its way.
    if rt.subconscious.learning_enabled() {
        return;
    }
    let nudge_after = rt.cfg.learning.nudge_after_turns;
    if nudge_after == 0 {
        return;
    }
    let cooldown = rt.cfg.subconscious_settings.learning_cooldown();
    let Some(spawn) =
        rt.learning_state
            .on_turn_completed(nudge_after, cooldown, std::time::Instant::now())
    else {
        return;
    };
    if let Err(e) = rt
        .publisher
        .publish(crate::bus::topics::Background, spawn)
        .await
    {
        tracing::warn!(error = %e, "failed to publish learner nudge spawn request");
    } else {
        tracing::info!("learner sub-agent nudge spawn requested");
    }
}

/// Publish a `TurnLifecycleEvent::Ended` closing the turn on an endpoint.
async fn publish_turn_ended(publisher: &Publisher, endpoint: &EndpointName, correlation_id: &str) {
    if let Err(e) = publisher
        .publish(
            topics::Endpoint(endpoint.clone()),
            TurnLifecycleEvent::Ended {
                correlation_id: correlation_id.to_string(),
            },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish turn ended event");
    }
}

/// Translate a completed turn's result into the bus events clients observe.
///
/// On success, emits one `ResponseEvent` per reply text to the output endpoint,
/// then closes the turn with `TurnLifecycleEvent::Ended`. When there is no output
/// endpoint (e.g. a background turn with no prior user endpoint), success publishes
/// nothing.
///
/// On failure, logs the error, broadcasts an `ErrorEvent` on the system
/// notification channel regardless of output endpoint, and — if there is an output
/// endpoint — still closes the turn with `Ended`.
async fn publish_turn_outcome(
    turn_result: anyhow::Result<Vec<String>>,
    publisher: &Publisher,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
    tz: chrono_tz::Tz,
) {
    match turn_result {
        Ok(texts) => {
            let Some(ep) = output_endpoint else {
                return;
            };
            for text in &texts {
                if let Err(e) = publisher
                    .publish(
                        topics::Endpoint(ep.clone()),
                        ResponseEvent {
                            correlation_id: correlation_id.to_string(),
                            content: text.clone(),
                            timestamp: crate::time::now_local(tz),
                            attachment: None,
                            conversation: None,
                        },
                    )
                    .await
                {
                    tracing::warn!(error = %e, "failed to publish response event");
                }
            }
            publish_turn_ended(publisher, ep, correlation_id).await;
        }
        Err(e) => {
            tracing::error!(error = %e, "agent processing error");
            if let Err(pub_err) = publisher
                .publish(
                    topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                    ErrorEvent {
                        correlation_id: correlation_id.to_string(),
                        message: e.to_string(),
                    },
                )
                .await
            {
                tracing::warn!(error = %pub_err, "failed to publish agent error event");
            }
            if let Some(ep) = output_endpoint {
                publish_turn_ended(publisher, ep, correlation_id).await;
            }
        }
    }
}

/// Where a background turn's output goes: the `switch_endpoint` override if
/// the agent set one since the user last spoke, else the user's last endpoint.
fn background_output_endpoint(
    switched_to: Option<EndpointName>,
    last_user_endpoint: Option<&EndpointName>,
) -> Option<EndpointName> {
    switched_to.or_else(|| last_user_endpoint.cloned())
}

/// Handle an inbound user message: run agent turn, persist, observe, and process leftovers.
#[tracing::instrument(skip_all, fields(correlation_id = %message.id, origin = %message.origin.endpoint))]
pub async fn handle_inbound_message(
    message: InboundMessage,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) {
    let reply_id = message.id.clone();
    let origin = message.origin.clone();
    let is_background = origin.endpoint == "background";

    // Determine output endpoint: background turns go wherever `switch_endpoint`
    // last pointed them, else the user's last endpoint; user turns derive from
    // the origin and become the new last endpoint.
    let output_endpoint = if is_background {
        background_output_endpoint(
            rt.output_topic_override_tx.borrow().clone(),
            rt.last_output_endpoint.as_ref(),
        )
    } else {
        // Clear any switch_endpoint override so responses follow the user's endpoint.
        rt.output_topic_override_tx.send_replace(None);
        let ep = EndpointName::from(origin.endpoint.as_str());
        rt.last_output_endpoint = Some(ep.clone());
        Some(ep)
    };

    // Only publish tool-activity events to endpoints with STREAMING capability.
    let tool_activity_endpoint = output_endpoint.as_ref().filter(|ep| {
        let endpoint_id = EndpointId::from(ep.as_ref());
        rt.endpoint_registry
            .get(&endpoint_id)
            .is_some_and(|entry| entry.capabilities.contains(EndpointCapabilities::STREAMING))
    });

    if let Some(ref ep) = output_endpoint
        && let Err(e) = rt
            .publisher
            .publish(
                topics::Endpoint(ep.clone()),
                TurnLifecycleEvent::Started {
                    correlation_id: reply_id.clone(),
                },
            )
            .await
    {
        tracing::warn!(error = %e, "failed to publish turn started event");
    }

    let before = rt.agent.message_count();

    if let Some(context) = message.context.as_deref() {
        rt.agent.inject_system_message(context);
    }

    // This turn's kickoff input's hop count: 0 for a genuine external
    // message, or the hop count recorded when an agent message was
    // delivered to main (see `AgentMessenger::deliver_to_main`).
    let kickoff_hop = rt.agent_messenger.take_main_hop(&message.id);
    rt.agent.hop_counter().set(kickoff_hop);

    let ctx_strings = load_prompt_context_strings(&rt.skill_state).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (turn_result, leftover_interrupts, subconscious_scratch) = run_agent_turn_with_interrupts(
        &mut rt.agent,
        &rt.agent_messenger,
        &message.content,
        &rt.publisher,
        output_endpoint.as_ref(),
        tool_activity_endpoint,
        &reply_id,
        Some(&origin),
        &prompt_ctx,
        &message.images,
        &mut rt.agent_subscriber,
        &mut rt.reload_rx,
        &mut rt.stop_rx,
        (!is_background && rt.subconscious.mid_turn_enabled())
            .then(|| Arc::clone(&rt.subconscious)),
    )
    .await;

    publish_turn_outcome(
        turn_result,
        &rt.publisher,
        output_endpoint.as_ref(),
        &reply_id,
        rt.tz,
    )
    .await;

    let visibility = if is_background {
        Visibility::Background
    } else {
        Visibility::User
    };
    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(rt, &new_messages, visibility, observe_deadline).await;

    // Background turns (including subconscious correction turns) are never
    // evaluated — this gate is what bounds the correction feedback loop.
    if !is_background {
        super::subconscious_hook::run_end_of_turn_subconscious(
            rt,
            &new_messages,
            &reply_id,
            subconscious_scratch.as_ref(),
        )
        .await;
        maybe_nudge_learner(rt).await;
    }

    process_leftover_interrupts(leftover_interrupts, rt);

    // Only update idle timer for user messages, not background turns.
    if !is_background && !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Subscriber;
    use std::time::Duration;

    const TEST_TZ: chrono_tz::Tz = chrono_tz::Tz::UTC;

    fn endpoint() -> EndpointName {
        EndpointName::from("ws")
    }

    #[test]
    fn background_output_prefers_the_switched_endpoint() {
        let telegram = EndpointName::from("telegram");
        assert_eq!(
            background_output_endpoint(Some(telegram.clone()), Some(&endpoint())),
            Some(telegram),
            "switch_endpoint must take effect for background turns"
        );
        assert_eq!(
            background_output_endpoint(None, Some(&endpoint())),
            Some(endpoint())
        );
        assert_eq!(background_output_endpoint(None, None), None);
    }

    /// Assert a subscriber receives no event within a short window, proving the
    /// unit published nothing on that topic/type.
    async fn assert_no_event<E: Clone + Send + Sync + 'static>(sub: &mut Subscriber<E>) {
        let recv = tokio::time::timeout(Duration::from_millis(50), sub.recv()).await;
        assert!(recv.is_err(), "expected no event, but one was published");
    }

    #[tokio::test]
    async fn ok_publishes_one_response_per_text_then_ends_turn() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Ok(vec!["first".into(), "second".into()]),
            &publisher,
            Some(&endpoint()),
            "corr-1",
            TEST_TZ,
        )
        .await;

        let first = responses.recv().await.unwrap().unwrap();
        assert_eq!(first.content, "first");
        assert_eq!(first.correlation_id, "corr-1");
        assert!(first.attachment.is_none());
        let second = responses.recv().await.unwrap().unwrap();
        assert_eq!(second.content, "second");

        let ended = lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "corr-1")
        );
    }

    #[tokio::test]
    async fn ok_with_no_texts_still_ends_turn() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(Ok(vec![]), &publisher, Some(&endpoint()), "corr-2", TEST_TZ).await;

        let ended = lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "corr-2")
        );
        assert_no_event(&mut responses).await;
    }

    #[tokio::test]
    async fn ok_without_output_endpoint_publishes_nothing() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Ok(vec!["ignored".into()]),
            &publisher,
            None,
            "corr-3",
            TEST_TZ,
        )
        .await;

        assert_no_event(&mut responses).await;
        assert_no_event(&mut lifecycle).await;
    }

    #[tokio::test]
    async fn err_broadcasts_error_event_and_ends_turn() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Err(anyhow::anyhow!("boom")),
            &publisher,
            Some(&endpoint()),
            "corr-4",
            TEST_TZ,
        )
        .await;

        let error = errors.recv().await.unwrap().unwrap();
        assert_eq!(error.correlation_id, "corr-4");
        assert_eq!(error.message, "boom");

        let ended = lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "corr-4")
        );
        assert_no_event(&mut responses).await;
    }

    #[tokio::test]
    async fn err_without_output_endpoint_still_broadcasts_error_without_ending_turn() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Err(anyhow::anyhow!("kaboom")),
            &publisher,
            None,
            "corr-5",
            TEST_TZ,
        )
        .await;

        let error = errors.recv().await.unwrap().unwrap();
        assert_eq!(error.correlation_id, "corr-5");
        assert_eq!(error.message, "kaboom");
        assert_no_event(&mut lifecycle).await;
    }

    /// A provider that blocks on a shared `Notify` until the test releases
    /// it, so a mid-turn message can be reliably injected and observed
    /// before the turn's model call resolves — no sleep-based timing.
    struct GatedProvider {
        gate: Arc<tokio::sync::Notify>,
        response: String,
    }

    #[async_trait::async_trait]
    impl crate::inference::InferenceProvider for GatedProvider {
        async fn complete(
            &self,
            _messages: &[crate::inference::Message],
            _tools: &[crate::inference::ToolDefinition],
            _options: &crate::inference::CompletionOptions,
        ) -> Result<crate::inference::InferenceResponse, crate::inference::InferenceError> {
            self.gate.notified().await;
            Ok(crate::inference::InferenceResponse::new(
                self.response.clone(),
                vec![],
            ))
        }

        fn model_name(&self) -> &'static str {
            "gated"
        }
    }

    fn test_agent(provider: GatedProvider) -> Agent {
        Agent::new(
            Box::new(provider),
            crate::tools::ToolRegistry::new(),
            crate::mcp::McpRegistry::new_shared(),
            crate::workspace::identity::IdentityFiles::default(),
            crate::agent::AgentConfig {
                options: crate::inference::CompletionOptions::default(),
                tz: TEST_TZ,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        )
    }

    #[tokio::test]
    async fn main_hop_counter_starts_at_kickoff_and_bumps_on_a_mid_turn_agent_message() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(crate::background::messaging::AgentMessenger::new(
            Arc::new(crate::background::registry::SessionRegistry::new()),
            publisher.clone(),
            store,
            crate::agent::HopLimits { soft: 8, hard: 32 },
        ));

        let gate = Arc::new(tokio::sync::Notify::new());
        let mut agent = test_agent(GatedProvider {
            gate: Arc::clone(&gate),
            response: "done".to_string(),
        });
        // The turn's kickoff input's hop count, as `handle_inbound_message`
        // would set it from a looked-up `take_main_hop`.
        agent.hop_counter().set(2);
        let hop_counter = agent.hop_counter().clone();

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_reload_tx, mut reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_messenger = Arc::clone(&messenger);
        let turn_task = tokio::spawn(async move {
            let (turn_result, _leftovers, _scratch) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                "hello",
                &publisher,
                None,
                None,
                "corr-hop",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut reload_rx,
                &mut stop_rx,
                None,
            )
            .await;
            turn_result.expect("gated turn should complete successfully");
            agent
        });

        // Deliver an agent message to main with a higher hop count than the
        // kickoff while the turn is blocked on the (gated) model call — it
        // must be observed as a mid-turn arrival and bump the counter. Uses
        // the same `messenger` instance the spawned turn reads hop counts
        // from (`take_main_hop` state lives on the instance, not the bus).
        messenger
            .send(
                "main",
                crate::bus::SessionAddress::from("spawned-researcher-0001"),
                "spawned".to_string(),
                "still working".to_string(),
                7,
            )
            .await
            .unwrap();

        // Poll the shared hop counter until the mid-turn bump lands, rather
        // than sleeping a guessed duration.
        tokio::time::timeout(Duration::from_secs(2), async {
            while hop_counter.get() < 7 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("hop counter should bump to 7 once the mid-turn message is drained");

        gate.notify_one();
        let finished_agent = tokio::time::timeout(Duration::from_secs(2), turn_task)
            .await
            .expect("turn should complete once the gate releases")
            .unwrap();
        assert_eq!(
            finished_agent.hop_counter().get(),
            7,
            "the bump from the mid-turn message must survive to the end of the turn"
        );
    }
}

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

/// Process leftover interrupts that arrived during an agent turn but weren't
/// consumed: inject their content into `agent`'s conversation as context for
/// whichever turn picks them up next.
///
/// Also resolves this turn's hop counter to whatever should carry forward
/// into that next turn: an unconsumed message-bearing leftover (a genuine
/// user message or an agent message) may still be carrying a hop count this
/// turn's own reply never actually incorporated, so it must survive into the
/// next turn's kickoff rather than being discarded — see
/// `handle_inbound_message`, which folds this counter (via `bump`, not
/// `set`) into its own kickoff hop instead of overwriting it. A turn that
/// consumed everything itself resets to zero here, since whatever it bumped
/// the counter to along the way has already been replied to and has nothing
/// left to carry.
pub fn process_leftover_interrupts(leftovers: Vec<Interrupt>, agent: &mut Agent) {
    let mut carried_hop: Option<u32> = None;
    for intr in leftovers {
        match intr {
            Interrupt::UserMessage(leftover_msg) => {
                // Its hop, if any — this may be an agent message relayed to
                // main (see `AgentMessenger::deliver_to_main`), which carries
                // no hop count of its own in `InboundMessage` — was already
                // folded into the shared hop counter when it arrived
                // mid-turn (see `run_agent_turn_with_interrupts`). Carry that
                // current value forward since this message is still
                // unconsumed.
                carried_hop = Some(carried_hop.unwrap_or(0).max(agent.hop_counter().get()));
                agent.inject_inbound_message(leftover_msg);
            }
            Interrupt::AgentMessage(msg) => {
                carried_hop = Some(carried_hop.unwrap_or(0).max(msg.hop_count));
                agent.inject_system_message(msg.format_for_agent());
            }
            Interrupt::Subconscious(content) => {
                // A late mid-turn finding degrades to a note for the next
                // turn; it carries no hop count of its own.
                agent.inject_system_message(content);
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
    agent.hop_counter().set(carried_hop.unwrap_or(0));
}

/// Drain remaining interrupts from an interrupt channel after a turn completes.
pub fn drain_interrupts(interrupt_rx: &mut mpsc::UnboundedReceiver<Interrupt>) -> Vec<Interrupt> {
    let mut leftovers = Vec::new();
    while let Ok(intr) = interrupt_rx.try_recv() {
        leftovers.push(intr);
    }
    leftovers
}

/// Drain every stop request already queued on `stop_rx` and answer each one
/// `false` ("nothing is running"), without waiting for more to arrive.
///
/// Called right before a turn's own select loop starts watching this same
/// channel, so anything found here necessarily predates the turn about to
/// run and cannot have been meant for it.
fn drain_stale_stop_requests(stop_rx: &mut mpsc::Receiver<StopRequest>) {
    while let Ok(req) = stop_rx.try_recv() {
        tracing::debug!(
            requested = ?req.reply_to,
            "stale stop request drained before new turn started, nothing to stop"
        );
        if let Some(tx) = req.result_tx {
            tx.send(false).ok();
        }
    }
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
            publisher: &rt.publisher,
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
/// Stop the turn for a shutdown trigger (SIGTERM, HTTP shutdown, restart)
/// exactly the way an ordinary user stop does — cancel the model-call race
/// and queue the same `Interrupt::Stopped` marker — so partial turn state
/// is persisted identically either way.
///
/// Split out of `run_agent_turn_with_interrupts` purely to keep that
/// function's line count down; it has one call site per shutdown trigger.
fn stop_turn_for_shutdown(
    reason: crate::gateway::types::ShutdownReason,
    correlation_id: &str,
    stop_token: &CancellationToken,
    interrupt_tx: &mpsc::UnboundedSender<Interrupt>,
) -> crate::gateway::types::ShutdownReason {
    tracing::info!(correlation_id = %correlation_id, ?reason, "shutdown trigger received, stopping active turn");
    stop_token.cancel();
    if interrupt_tx.send(Interrupt::Stopped).is_err() {
        tracing::warn!(
            "interrupt channel closed, stop marker dropped (model-call cancellation still applies)"
        );
    }
    reason
}

/// Fold one mid-turn message-bus event into the turn: inject it if it
/// belongs to main, route it elsewhere (off this select loop) otherwise, or
/// just log a closed channel / type mismatch.
///
/// Split out of `run_agent_turn_with_interrupts` purely to keep that
/// function's line count down.
fn handle_mid_turn_message(
    next_msg: Result<Option<MessageEvent>, crate::bus::BusError>,
    agent_messenger: &crate::background::messaging::AgentMessenger,
    conversation_router: &Arc<crate::background::ConversationRouter>,
    interrupt_tx: &mpsc::UnboundedSender<Interrupt>,
    hop_counter: &crate::agent::HopCounter,
) {
    match next_msg {
        Ok(Some(msg_event)) => {
            let inbound = crate::interfaces::types::InboundMessage {
                id: msg_event.id,
                content: msg_event.content,
                origin: msg_event.origin,
                timestamp: chrono::Utc::now(),
                images: msg_event.images,
                context: msg_event.context,
            };
            if inbound.origin.belongs_to_main() {
                // A mid-turn message on this topic may be a genuine user
                // message (hop 0) or an agent message relayed to main (see
                // `AgentMessenger::deliver_to_main`) — either way, it's one
                // more input driving this turn. `take_main_hop` is looked up
                // unconditionally (it removes the entry either way, so a
                // message this channel refuses shouldn't leave the entry
                // stranded), but the counter is only bumped once the message
                // is actually queued into the turn — bumping on a failed
                // send would claim a hop the turn never actually received.
                let hop = agent_messenger.take_main_hop(&inbound.id);
                if interrupt_tx.send(Interrupt::UserMessage(inbound)).is_ok() {
                    hop_counter.bump(hop);
                } else {
                    tracing::warn!("interrupt channel closed, dropping user message mid-turn");
                }
            } else {
                // Not main's: a group chat, a channel, or a non-owner DM
                // message that arrived while main's own turn is running. It
                // must never be folded into main's turn — route it to its
                // conversation's session instead, off the turn's own select
                // loop so a completing target's teardown can't stall main.
                let router = Arc::clone(conversation_router);
                tokio::spawn(async move { router.route(inbound).await });
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

/// Handle one stop request observed while a turn is running: cancel and
/// queue an `Interrupt::Stopped` marker when it matches this turn, and
/// reply with whether it did. A `None` (the stop channel itself closed) is
/// just logged.
///
/// Split out of `run_agent_turn_with_interrupts` purely to keep that
/// function's line count down.
fn handle_mid_turn_stop_request(
    stop_req: Option<StopRequest>,
    correlation_id: &str,
    stop_token: &CancellationToken,
    interrupt_tx: &mpsc::UnboundedSender<Interrupt>,
) {
    let Some(req) = stop_req else {
        tracing::debug!("stop request channel closed during turn");
        return;
    };
    let matches = req
        .reply_to
        .as_deref()
        .is_none_or(|id| id == correlation_id);
    if matches {
        tracing::info!(correlation_id = %correlation_id, "stopping active turn");
        stop_token.cancel();
        if interrupt_tx.send(Interrupt::Stopped).is_err() {
            tracing::warn!(
                "interrupt channel closed, stop marker dropped (model-call cancellation still applies)"
            );
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

#[expect(
    clippy::too_many_arguments,
    reason = "publisher and topic params added during bus migration"
)]
async fn run_agent_turn_with_interrupts(
    agent: &mut Agent,
    agent_messenger: &crate::background::messaging::AgentMessenger,
    conversation_router: &Arc<crate::background::ConversationRouter>,
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
    sigterm: &mut crate::gateway::types::TermSignal,
    gateway_shutdown_rx: &mut mpsc::Receiver<()>,
    restart_rx: &mut mpsc::Receiver<()>,
    subconscious: Option<Arc<crate::subconscious::Subconscious>>,
) -> (
    anyhow::Result<Vec<String>>,
    Vec<Interrupt>,
    Option<Arc<std::sync::Mutex<crate::subconscious::TurnScratch>>>,
    Option<crate::gateway::types::ShutdownReason>,
) {
    // A request that arrived while the previous turn's synchronous
    // post-processing ran (persist, observe, subconscious) sits unread in
    // `stop_rx` until something drains it — nobody polls this channel
    // between the end of that turn's own select loop below and this one
    // starting. Left alone, the inner loop's `stop_req = stop_rx.recv()` arm
    // would read it as its very first event and, since a chat command's
    // stop carries `reply_to: None` ("stop whichever turn is running"),
    // apply it to this brand new, unrelated turn. Draining and answering
    // every such request here — before this turn's own loop ever starts
    // watching the channel — makes that impossible by construction: nothing
    // left over from before this turn can survive into it.
    drain_stale_stop_requests(stop_rx);

    // Cloned before the mutable borrow below (`agent.hop_counter()` borrows
    // `agent`, and `process_message` needs it mutably) — a clone still
    // refers to the same shared cell, so mid-turn bumps below and reads from
    // the `message_agent`/`subagent_spawn` tools stay in sync regardless.
    let hop_counter = agent.hop_counter().clone();
    let (interrupt_tx, mut interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
    let watch =
        subconscious.map(|s| crate::subconscious::SubconsciousWatch::new(s, interrupt_tx.clone()));
    // Cancelled to abort an in-flight model call immediately; a stop that
    // lands between calls is instead observed via `Interrupt::Stopped` at
    // the tool loop's checkpoint (see `execute_turn`).
    let stop_token = CancellationToken::new();
    // Set when a shutdown trigger (SIGTERM, HTTP shutdown, restart) fires
    // while this turn is running, so the caller can perform the same action
    // its own idle-time signal handling would have — the signal itself is
    // already consumed here and won't fire again for the outer event loop.
    let mut shutdown_reason: Option<crate::gateway::types::ShutdownReason> = None;
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
                    handle_mid_turn_message(
                        next_msg,
                        agent_messenger,
                        conversation_router,
                        &interrupt_tx,
                        &hop_counter,
                    );
                }
                _ = reload_rx.changed() => {
                    tracing::info!("reload signal received during active turn, deferring");
                }
                stop_req = stop_rx.recv() => {
                    handle_mid_turn_stop_request(stop_req, correlation_id, &stop_token, &interrupt_tx);
                }
                // The three shutdown triggers below all stop this turn the
                // same way a user stop does (see `stop_turn_for_shutdown`)
                // and record which one fired, for the caller to act on once
                // this call returns.
                () = sigterm.recv() => {
                    shutdown_reason = Some(stop_turn_for_shutdown(
                        crate::gateway::types::ShutdownReason::Sigterm,
                        correlation_id,
                        &stop_token,
                        &interrupt_tx,
                    ));
                }
                _ = gateway_shutdown_rx.recv() => {
                    shutdown_reason = Some(stop_turn_for_shutdown(
                        crate::gateway::types::ShutdownReason::GatewayShutdown,
                        correlation_id,
                        &stop_token,
                        &interrupt_tx,
                    ));
                }
                _ = restart_rx.recv() => {
                    shutdown_reason = Some(stop_turn_for_shutdown(
                        crate::gateway::types::ShutdownReason::Restart,
                        correlation_id,
                        &stop_token,
                        &interrupt_tx,
                    ));
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

    (turn_result, leftover_interrupts, scratch, shutdown_reason)
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
/// Publish `TurnLifecycleEvent::Started` (if there's an output endpoint)
/// and spawn the turn-start checkpoint, which captures the workspace state
/// before anything this turn does, attributed as an outside edit.
async fn publish_turn_started(
    rt: &GatewayRuntime,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
) {
    if let Some(ep) = output_endpoint
        && let Err(e) = rt
            .publisher
            .publish(
                topics::Endpoint(ep.clone()),
                TurnLifecycleEvent::Started {
                    correlation_id: correlation_id.to_string(),
                },
            )
            .await
    {
        tracing::warn!(error = %e, "failed to publish turn started event");
    }
    rt.checkpoints
        .spawn_turn_start_checkpoint(main_turn_checkpoint_context(
            correlation_id,
            crate::checkpoints::CheckpointTrigger::TurnStart,
            "outside edit before turn start".to_string(),
        ));
}

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

/// Build the checkpoint context for a main-agent turn boundary. `correlation_id`
/// (the inbound message id) doubles as the turn id — main has no separate
/// run id, so `run_id` is left unset.
fn main_turn_checkpoint_context(
    correlation_id: &str,
    trigger: crate::checkpoints::CheckpointTrigger,
    summary: String,
) -> crate::checkpoints::CheckpointContext {
    crate::checkpoints::CheckpointContext {
        address: "main".to_string(),
        run_id: None,
        turn_id: Some(correlation_id.to_string()),
        trigger,
        summary,
    }
}

/// A short, one-line description of a main-agent turn's outcome, for the
/// turn-end checkpoint's commit message.
fn main_turn_end_summary(turn_result: &anyhow::Result<Vec<String>>) -> String {
    match turn_result {
        Ok(texts) => texts.first().filter(|t| !t.is_empty()).map_or_else(
            || "turn completed".to_string(),
            |t| t.chars().take(120).collect(),
        ),
        Err(e) => format!("turn failed: {e}"),
    }
}

/// Spawn the turn-end checkpoint, attributed with a short summary of the
/// turn's outcome.
fn spawn_main_turn_end_checkpoint(
    rt: &GatewayRuntime,
    correlation_id: &str,
    turn_result: &anyhow::Result<Vec<String>>,
) {
    rt.checkpoints
        .spawn_turn_end_checkpoint(main_turn_checkpoint_context(
            correlation_id,
            crate::checkpoints::CheckpointTrigger::TurnEnd,
            main_turn_end_summary(turn_result),
        ));
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
            let described = crate::inference::describe_turn_failure(&e);
            tracing::error!(error = %described.details, "agent processing error");
            if let Err(pub_err) = publisher
                .publish(
                    topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                    ErrorEvent {
                        correlation_id: correlation_id.to_string(),
                        message: described.message,
                        details: Some(described.details),
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
///
/// Returns the shutdown trigger that interrupted this turn, if any — the
/// caller (`handle_bus_event`) must act on it, since the underlying signal
/// was already consumed while stopping the turn (see
/// `run_agent_turn_with_interrupts`) and won't fire again for the event
/// loop's own idle-time handling of it.
#[tracing::instrument(skip_all, fields(correlation_id = %message.id, origin = %message.origin.endpoint))]
pub async fn handle_inbound_message(
    message: InboundMessage,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> Option<crate::gateway::types::ShutdownReason> {
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

    publish_turn_started(rt, output_endpoint.as_ref(), &reply_id).await;

    let before = rt.agent.message_count();

    if let Some(context) = message.context.as_deref() {
        rt.agent.inject_system_message(context);
    }

    // This turn's kickoff input's hop count: 0 for a genuine external
    // message, or the hop count recorded when an agent message was
    // delivered to main (see `AgentMessenger::deliver_to_main`). Folded in
    // with `bump`, not overwritten with `set` — the previous turn's
    // `process_leftover_interrupts` left the counter at whatever an
    // unconsumed leftover is still carrying (zero if there was none), and
    // that must survive here rather than being reset to just this turn's own
    // kickoff hop.
    let kickoff_hop = rt.agent_messenger.take_main_hop(&message.id);
    rt.agent.hop_counter().bump(kickoff_hop);

    let ctx_strings = load_prompt_context_strings(&rt.skill_state).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (turn_result, leftover_interrupts, subconscious_scratch, shutdown_reason) =
        run_agent_turn_with_interrupts(
            &mut rt.agent,
            &rt.agent_messenger,
            &rt.conversation_router,
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
            &mut rt.sigterm,
            &mut rt.gateway_shutdown_rx,
            &mut rt.restart_rx,
            (!is_background && rt.subconscious.mid_turn_enabled())
                .then(|| Arc::clone(&rt.subconscious)),
        )
        .await;

    spawn_main_turn_end_checkpoint(rt, &reply_id, &turn_result);

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

    process_leftover_interrupts(leftover_interrupts, &mut rt.agent);

    // Only update idle timer for user messages, not background turns.
    if !is_background && !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }

    shutdown_reason
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
    fn interrupt_channel_accepts_more_than_the_old_bounded_capacity() {
        // Regression: the interrupt channel used to be a 32-slot bounded
        // channel, silently dropping a mid-turn user message (with only a
        // warn log) once a long turn's queue filled up. It's unbounded now,
        // so sending well past that old limit must never fail or drop.
        const PAST_OLD_CAPACITY: usize = 100;
        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        for _ in 0..PAST_OLD_CAPACITY {
            tx.send(Interrupt::Stopped)
                .expect("an unbounded channel must never refuse a send");
        }
        let leftovers = drain_interrupts(&mut rx);
        assert_eq!(
            leftovers.len(),
            PAST_OLD_CAPACITY,
            "every queued interrupt must be drained, none dropped"
        );
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
        assert_eq!(
            error.message,
            "Something went wrong while the agent was working. Try again; if it keeps \
             happening, check Residuum's logs for details.",
            "an unclassified error must still get a plain-language message, never the raw cause"
        );
        assert_eq!(
            error.details.as_deref(),
            Some("boom"),
            "the raw cause must survive in details for a developer or the details toggle"
        );

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
        assert_eq!(
            error.details.as_deref(),
            Some("kaboom"),
            "the raw cause must survive in details even with no output endpoint"
        );
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

    /// A termination-signal listener for tests that never triggers — its
    /// construction differs by platform (fallible on Unix, infallible
    /// elsewhere), and no test in this module needs it to actually fire.
    #[cfg(unix)]
    fn dummy_sigterm() -> crate::gateway::types::TermSignal {
        crate::gateway::types::TermSignal::new()
            .expect("failed to register a test-only SIGTERM listener")
    }
    #[cfg(not(unix))]
    fn dummy_sigterm() -> crate::gateway::types::TermSignal {
        crate::gateway::types::TermSignal::new()
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
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(crate::background::ConversationRouter::new(Arc::clone(
            &messenger,
        )));

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
        let mut sigterm = dummy_sigterm();
        let (_gateway_shutdown_tx, mut gateway_shutdown_rx) = mpsc::channel::<()>(1);
        let (_restart_tx, mut restart_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = tokio::spawn(async move {
            let (turn_result, _leftovers, _scratch, _shutdown) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
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
                &mut sigterm,
                &mut gateway_shutdown_rx,
                &mut restart_rx,
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

    #[tokio::test]
    async fn stale_stop_request_queued_before_turn_start_is_answered_and_does_not_cancel_it() {
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
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(crate::background::ConversationRouter::new(Arc::clone(
            &messenger,
        )));

        let gate = Arc::new(tokio::sync::Notify::new());
        // Permit stored ahead of time (Tokio's `Notify` keeps one), so the
        // model call returns immediately once awaited below.
        gate.notify_one();
        let mut agent = test_agent(GatedProvider {
            gate,
            response: "done".to_string(),
        });

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_reload_tx, mut reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(4);
        let mut sigterm = dummy_sigterm();
        let (_gateway_shutdown_tx, mut gateway_shutdown_rx) = mpsc::channel::<()>(1);
        let (_restart_tx, mut restart_rx) = mpsc::channel::<()>(1);

        // A stop request that arrived while the *previous* turn's
        // synchronous post-processing ran, before this turn's own select
        // loop starts watching `stop_rx` — exactly the window nothing used
        // to drain.
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        stop_tx
            .send(StopRequest {
                reply_to: None,
                result_tx: Some(result_tx),
            })
            .await
            .unwrap();

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let (turn_result, _leftovers, _scratch, _shutdown) = run_agent_turn_with_interrupts(
            &mut agent,
            &messenger,
            &conversation_router,
            "hello",
            &publisher,
            None,
            None,
            "corr-stale-stop",
            None,
            &prompt_ctx,
            &[],
            &mut agent_subscriber,
            &mut reload_rx,
            &mut stop_rx,
            &mut sigterm,
            &mut gateway_shutdown_rx,
            &mut restart_rx,
            None,
        )
        .await;

        assert!(
            !result_rx.await.unwrap(),
            "a stop request queued before this turn started must be answered false, not left pending"
        );
        turn_result
            .expect("the new turn must complete normally, not be cancelled by the stale request");
    }

    /// A group-chat message on "chan-1", sent by the owner — the owner
    /// speaking in a shared conversation still routes to that conversation's
    /// session, not to main.
    fn group_chat_message() -> crate::bus::MessageEvent {
        crate::bus::MessageEvent {
            id: "group-msg-1".to_string(),
            content: "anyone around?".to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: None,
                conversation: Some(crate::interfaces::types::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: crate::interfaces::types::ConversationKind::GroupChat,
                    is_owner: true,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            context: None,
        }
    }

    #[tokio::test]
    async fn mid_turn_message_not_belonging_to_main_is_routed_away_not_injected() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let registry = Arc::new(crate::background::registry::SessionRegistry::new());
        let messenger = Arc::new(crate::background::messaging::AgentMessenger::new(
            Arc::clone(&registry),
            publisher.clone(),
            store,
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(crate::background::ConversationRouter::new(Arc::clone(
            &messenger,
        )));
        let mut spawns: Subscriber<crate::bus::SpawnRequestEvent> =
            handle.subscribe(topics::Background).await.unwrap();

        let gate = Arc::new(tokio::sync::Notify::new());
        let mut agent = test_agent(GatedProvider {
            gate: Arc::clone(&gate),
            response: "done".to_string(),
        });
        agent.hop_counter().set(0);
        let hop_counter = agent.hop_counter().clone();

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_reload_tx, mut reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let mut sigterm = dummy_sigterm();
        let (_gateway_shutdown_tx, mut gateway_shutdown_rx) = mpsc::channel::<()>(1);
        let (_restart_tx, mut restart_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = tokio::spawn(async move {
            let (turn_result, _leftovers, _scratch, _shutdown) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
                "hello",
                &publisher,
                None,
                None,
                "corr-route",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut reload_rx,
                &mut stop_rx,
                &mut sigterm,
                &mut gateway_shutdown_rx,
                &mut restart_rx,
                None,
            )
            .await;
            turn_result.expect("gated turn should complete successfully");
            agent
        });

        // A group-chat message — even one the owner sent — must not reach
        // main's live turn.
        handle
            .publisher()
            .publish(crate::bus::topics::UserMessage, group_chat_message())
            .await
            .unwrap();

        // The router should start a session for this conversation instead of
        // ever reaching main's turn.
        let address =
            crate::background::registry::conversation_session_address("discord", "chan-1");
        let spawn_event = tokio::time::timeout(Duration::from_secs(2), spawns.recv())
            .await
            .expect("the group-chat message should have started its own conversation session")
            .unwrap()
            .unwrap();
        assert_eq!(spawn_event.address, address);
        assert_eq!(spawn_event.prompt, "anyone around?");

        assert_eq!(
            hop_counter.get(),
            0,
            "a message that doesn't belong to main must never bump its hop counter"
        );

        gate.notify_one();
        let finished_agent = tokio::time::timeout(Duration::from_secs(2), turn_task)
            .await
            .expect("turn should complete once the gate releases")
            .unwrap();
        assert!(
            !finished_agent
                .messages_since(0)
                .iter()
                .any(|m| m.content.contains("anyone around?")),
            "the group-chat message must never be injected into main's own turn"
        );
    }

    /// A cheap `Agent` for exercising `process_leftover_interrupts` directly
    /// — none of these tests ever run a turn, so the provider is never
    /// called.
    fn hop_test_agent() -> Agent {
        test_agent(GatedProvider {
            gate: Arc::new(tokio::sync::Notify::new()),
            response: "unused".to_string(),
        })
    }

    fn sample_inbound(content: &str) -> crate::interfaces::types::InboundMessage {
        crate::interfaces::types::InboundMessage {
            id: "leftover-1".to_string(),
            content: content.to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: None,
        }
    }

    #[test]
    fn no_leftovers_resets_the_hop_counter_for_a_clean_next_turn() {
        let mut agent = hop_test_agent();
        // What a turn's own mid-turn bumps left behind for messages it
        // actually consumed and already replied to — nothing should carry
        // forward from that once the turn is over.
        agent.hop_counter().bump(9);

        process_leftover_interrupts(vec![], &mut agent);

        assert_eq!(
            agent.hop_counter().get(),
            0,
            "a turn with nothing left over must not leak its already-answered hop count \
             into the next turn"
        );
    }

    #[test]
    fn a_leftover_user_message_carries_its_hop_forward_when_the_next_kickoff_is_lower() {
        // The exact "loop-defeat" scenario the hop-count guard exists to
        // prevent: a high-hop agent message relayed to main arrives mid-turn
        // (so it's already been folded into the shared hop counter — see
        // `run_agent_turn_with_interrupts`) but the turn ends before ever
        // draining it, so it survives as a leftover `Interrupt::UserMessage`.
        // If the next turn's kickoff simply overwrote the hop counter from
        // its own (lower, genuinely external) input, the loop guard would
        // reset to zero every time a looping message happened to arrive this
        // way, defeating hop-count enforcement entirely.
        let mut agent = hop_test_agent();
        agent.hop_counter().bump(9);

        process_leftover_interrupts(
            vec![Interrupt::UserMessage(sample_inbound("still looping"))],
            &mut agent,
        );
        assert_eq!(
            agent.hop_counter().get(),
            9,
            "an unconsumed leftover must keep the turn's hop count, not discard it"
        );

        // The next turn's kickoff arrives with its own, lower hop count.
        // Folding (`bump`), not overwriting (`set`), is what
        // `handle_inbound_message` does in production — simulate that here.
        agent.hop_counter().bump(0);
        assert_eq!(
            agent.hop_counter().get(),
            9,
            "the next turn must not reset the loop guard's hop count to its own lower kickoff"
        );
    }

    #[test]
    fn a_leftover_agent_message_carries_its_own_hop_count_forward() {
        let mut agent = hop_test_agent();
        // The shared counter never got bumped this time (unlike the
        // relayed-to-main case, an `Interrupt::AgentMessage` carries its hop
        // count directly, so `process_leftover_interrupts` doesn't need to
        // rely on the counter already reflecting it).
        process_leftover_interrupts(
            vec![Interrupt::AgentMessage(crate::bus::AgentMessageEvent {
                from: crate::bus::SessionAddress::from("spawned-researcher-0001"),
                from_category: "spawned".to_string(),
                content: "still looping".to_string(),
                hop_count: 12,
            })],
            &mut agent,
        );
        assert_eq!(
            agent.hop_counter().get(),
            12,
            "a leftover agent message's own hop count must carry forward"
        );
    }

    #[test]
    fn a_leftover_subconscious_note_alone_carries_no_hop() {
        // A subconscious correction is not a message from another agent and
        // carries no hop count of its own — it must not force a stale
        // mid-turn bump to survive when it's the only thing left over.
        let mut agent = hop_test_agent();
        agent.hop_counter().bump(9);

        process_leftover_interrupts(
            vec![Interrupt::Subconscious("[Subconscious] note".to_string())],
            &mut agent,
        );
        assert_eq!(
            agent.hop_counter().get(),
            0,
            "a subconscious-only leftover carries no hop count of its own"
        );
    }

    #[tokio::test]
    async fn shutdown_trigger_stops_an_active_turn_and_reports_the_reason() {
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
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(crate::background::ConversationRouter::new(Arc::clone(
            &messenger,
        )));

        // Never notified — the turn must end via the shutdown trigger below,
        // not because the model call happened to resolve on its own.
        let gate = Arc::new(tokio::sync::Notify::new());
        let mut agent = test_agent(GatedProvider {
            gate: Arc::clone(&gate),
            response: "unused".to_string(),
        });

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_reload_tx, mut reload_rx) = tokio::sync::watch::channel(ReloadSignal::None);
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let mut sigterm = dummy_sigterm();
        let (gateway_shutdown_tx, mut gateway_shutdown_rx) = mpsc::channel::<()>(1);
        let (_restart_tx, mut restart_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_task = tokio::spawn(async move {
            run_agent_turn_with_interrupts(
                &mut agent,
                &messenger,
                &conversation_router,
                "hello",
                &publisher,
                None,
                None,
                "corr-shutdown",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut reload_rx,
                &mut stop_rx,
                &mut sigterm,
                &mut gateway_shutdown_rx,
                &mut restart_rx,
                None,
            )
            .await
        });

        // The turn is now blocked on the gated model call; request a
        // shutdown the way the HTTP `/api/shutdown` endpoint does.
        gateway_shutdown_tx.send(()).await.unwrap();

        let (turn_result, _leftovers, _scratch, shutdown_reason) =
            tokio::time::timeout(std::time::Duration::from_secs(2), turn_task)
                .await
                .expect("a shutdown trigger should stop the turn well within 2s")
                .unwrap();

        assert_eq!(
            shutdown_reason,
            Some(crate::gateway::types::ShutdownReason::GatewayShutdown),
            "the caller must learn which trigger stopped the turn"
        );
        let texts = turn_result.expect("a shutdown-stopped turn is not a turn error");
        assert!(
            texts.is_empty(),
            "the turn was cut short before producing a reply"
        );
    }
}

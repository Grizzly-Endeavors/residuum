//! Agent turn handling and message processing in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::agent::context::{PromptContext, SkillsContext};
use crate::agent::interrupt::Interrupt;
use crate::bus::{
    EndpointName, ErrorEvent, MainConversationEvent, MessageEvent, NotifyName, Publisher,
    ResponseEvent, SYSTEM_CHANNEL, Subscriber, TurnLifecycleEvent, TurnOrigin, topics,
};

use crate::config::Config;
use crate::gateway::types::{AgentRuntime, StopRequest};
use crate::hub::activity::ActivityTracker;
use crate::inference::ImageData;
use crate::interfaces::types::{BACKGROUND_ENDPOINT, MessageOrigin, chat_interface_endpoint};
use crate::memory::types::Visibility;
use crate::skills::SharedSkillState;
use crate::tracing_service::TracingService;

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

/// Process leftover interrupts that arrived during an agent turn after its
/// last checkpoint drain, so no model call ever saw them.
///
/// Message-bearing leftovers (a user message or an agent message) are
/// returned, oldest first, for the caller to run as turns of their own: the
/// sender is waiting on a reply, and injecting the text into history as
/// context would leave the agent never acting on it. Each keeps its content,
/// images, context and sender attribution, and its hop count is recorded on
/// `messenger` so the turn that handles it starts from the hop count the
/// message carried. They are not injected into `agent`'s history here; the
/// turn that handles them records them.
///
/// A subconscious finding has no sender waiting on it, so it degrades to a
/// note in `agent`'s history for whichever turn comes next. A stop marker
/// needs nothing: the stop note was already injected where the stop was
/// observed.
///
/// Also resets the hop counter to zero: everything this turn consumed has
/// been replied to, and whatever a returned message still carries travels
/// with that message (see `handle_inbound_message`), not in the counter.
pub fn process_leftover_interrupts(
    leftovers: Vec<Interrupt>,
    agent: &mut Agent,
    messenger: &crate::background::messaging::AgentMessenger,
) -> Vec<MessageEvent> {
    let mut late_messages = Vec::new();
    for intr in leftovers {
        match intr {
            Interrupt::UserMessage(leftover_msg) => {
                // Its own hop, if any — this may be an agent message relayed
                // to main (see `AgentMessenger::deliver_to_main`), which
                // carries no hop count of its own in `MessageEvent` — noted
                // when the turn took it in (see `handle_mid_turn_message`).
                // The shared counter holds every input's hops combined, so
                // reading it here would hand a plain user message the
                // teammate traffic's hop count.
                let own_hop = messenger.mid_turn_hop(&leftover_msg.id);
                messenger.restore_main_hop(&leftover_msg.id, own_hop);
                late_messages.push(leftover_msg);
            }
            Interrupt::AgentMessage(msg) => {
                let event = MessageEvent::from_agent(&msg);
                messenger.restore_main_hop(&event.id, msg.hop_count);
                late_messages.push(event);
            }
            Interrupt::Subconscious(content) => {
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
    messenger.clear_mid_turn_hops();
    agent.hop_counter().set(0);
    if !late_messages.is_empty() {
        tracing::debug!(
            count = late_messages.len(),
            "messages arrived after the turn's last checkpoint, requeued to start a new turn"
        );
    }
    late_messages
}

/// Fold messages that will not get a turn of their own into `agent`'s
/// history as context, so they stay visible to whatever runs next, and
/// persist them to the recent-messages log so they survive a restart. Used
/// when the agent is stopping and there is no next turn to run them as; the
/// senders were already told these were delivered.
///
/// Web clients are not sent a `user_message` echo for them: the echo names the
/// turn a message started or joined, none runs for these, and the bus the
/// frame would travel on shuts down right after. History shows them once the
/// agent is running again.
pub(super) async fn inject_undelivered_messages(
    agent: &mut Agent,
    messenger: &crate::background::messaging::AgentMessenger,
    layout: &crate::workspace::layout::WorkspaceLayout,
    tz: chrono_tz::Tz,
    messages: impl IntoIterator<Item = MessageEvent>,
) {
    let mut senders = Vec::new();
    for message in messages {
        // Discard the hop entry `process_leftover_interrupts` recorded;
        // nothing will look it up now.
        messenger.forget_main_hop(&message.id);
        let visibility = if message.origin.endpoint == "background" {
            Visibility::Background
        } else {
            Visibility::User
        };
        let id = message.id.clone();
        senders.push(
            message
                .origin
                .agent_sender
                .as_ref()
                .map(|a| a.address.clone())
                .or_else(|| message.origin.sender.as_ref().map(|s| s.name.clone()))
                .unwrap_or_else(|| message.origin.endpoint.clone()),
        );
        let before = agent.message_count();
        agent.inject_inbound_message(message);
        if let Err(e) = crate::memory::recent_messages::append_recent_messages(
            &layout.recent_messages_json(),
            agent.messages_since(before),
            visibility,
            tz,
            Some(&id),
        )
        .await
        {
            tracing::warn!(
                error = %e,
                message_id = %id,
                "failed to persist a message delivered as the agent stopped, it will be missing from history after a restart"
            );
        }
    }
    if !senders.is_empty() {
        tracing::info!(
            count = senders.len(),
            senders = ?senders,
            "recorded messages that arrived as the agent stopped into history"
        );
    }
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
///
/// `turn_id` is the correlation id of the turn that produced these messages
/// (the same id sent as `reply_to` on `turn_started`/`turn_ended`).
pub async fn persist_and_maybe_observe(
    rt: &mut AgentRuntime,
    new_messages: &[crate::inference::Message],
    visibility: Visibility,
    observe_deadline: &mut Option<tokio::time::Instant>,
    turn_id: Option<&str>,
) {
    use crate::gateway::memory::persist_and_check_thresholds;

    let action = persist_and_check_thresholds(
        new_messages,
        visibility,
        &rt.observer,
        &rt.layout,
        rt.tz,
        turn_id,
    )
    .await;
    if apply_observe_action(action, observe_deadline, rt.observer.cooldown_secs()) {
        let mem = MemorySubsystems {
            observer: Arc::clone(&rt.observer),
            merge_writer: Arc::clone(&rt.merge_writer),
            layout: rt.layout.clone(),
            tz: rt.tz,
            publisher: rt.publisher.clone(),
        };
        // Fire-and-forget: the background worker (see
        // `crate::gateway::post_turn`) runs the cycle and reports its
        // `Agent`-touching tail back over `post_turn_result_rx`, which the
        // main loop applies — this call must not block the turn that
        // triggered it from returning, or the whole point of backgrounding
        // this is lost.
        rt.post_turn_observe.trigger(mem);
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

/// Stop the turn because the hub asked the agent to stop, exactly the way an
/// ordinary user stop does — cancel the model-call race and queue the same
/// `Interrupt::Stopped` marker — so partial turn state is persisted
/// identically either way.
///
/// Split out of `run_agent_turn_with_interrupts` purely to keep that
/// function's line count down.
fn stop_turn_for_agent_stop(
    correlation_id: &str,
    stop_token: &CancellationToken,
    interrupt_tx: &mpsc::UnboundedSender<Interrupt>,
) {
    tracing::info!(correlation_id = %correlation_id, "agent stop requested, stopping active turn");
    stop_token.cancel();
    if interrupt_tx.send(Interrupt::Stopped).is_err() {
        tracing::warn!(
            "interrupt channel closed, stop marker dropped (model-call cancellation still applies)"
        );
    }
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
        Ok(Some(inbound)) => {
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
                let id = inbound.id.clone();
                if interrupt_tx.send(Interrupt::UserMessage(inbound)).is_ok() {
                    hop_counter.bump(hop);
                    agent_messenger.note_mid_turn_hop(&id, hop);
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
                crate::util::spawn_in_span(async move { router.route(inbound).await });
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
    correlation_id: &str,
    origin: Option<&MessageOrigin>,
    prompt_ctx: &PromptContext<'_>,
    images: &[ImageData],
    agent_subscriber: &mut Subscriber<MessageEvent>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    agent_stop_rx: &mut mpsc::Receiver<()>,
    subconscious: Option<Arc<crate::subconscious::Subconscious>>,
) -> (
    anyhow::Result<Vec<String>>,
    Vec<Interrupt>,
    Option<Arc<std::sync::Mutex<crate::subconscious::TurnScratch>>>,
    bool,
) {
    // A request that arrived in the gap between the previous turn's own
    // select loop below ending and this one starting sits unread in
    // `stop_rx` until something drains it — nobody polls this channel in
    // that gap (persisting, triggering the post-turn background workers —
    // see `crate::gateway::post_turn` — updating the idle timer, and so on
    // all happen there, none of them touching `stop_rx`). Left alone, the
    // inner loop's `stop_req = stop_rx.recv()` arm would read it as its very
    // first event and, since a chat command's stop carries `reply_to: None`
    // ("stop whichever turn is running"), apply it to this brand new,
    // unrelated turn. Draining and answering every such request here —
    // before this turn's own loop ever starts
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
    // Set when the hub asks the agent to stop while this turn is running, so
    // the caller can shut the agent down — the request itself is already
    // consumed here and won't arrive again for the outer event loop.
    let mut stop_requested = false;
    let turn_result = {
        let mut turn = std::pin::pin!(agent.process_message(
            content,
            publisher,
            output_endpoint,
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
                stop_req = stop_rx.recv() => {
                    handle_mid_turn_stop_request(stop_req, correlation_id, &stop_token, &interrupt_tx);
                }
                // The hub's stop request stops this turn the same way a user
                // stop does (see `stop_turn_for_agent_stop`) and records that
                // it fired, for the caller to act on once this call returns.
                _ = agent_stop_rx.recv() => {
                    stop_turn_for_agent_stop(correlation_id, &stop_token, &interrupt_tx);
                    stop_requested = true;
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

    (turn_result, leftover_interrupts, scratch, stop_requested)
}

/// Fallback learning trigger for users running without the subconscious
/// learning path: after a configured number of foreground turns, spawn the
/// `learner` sub-agent to review the recent conversation.
///
/// Skipped when the subconscious learning path is active (that path owns
/// spawning, so this avoids double-spawns) or when `nudge_after_turns` is zero.
/// Shares the subconscious learning cooldown.
async fn maybe_nudge_learner(rt: &mut AgentRuntime) {
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
    let Some(spawn) = rt
        .learning_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// Publish one event of the main agent's conversation.
async fn publish_main_conversation(publisher: &Publisher, event: MainConversationEvent) {
    if let Err(e) = publisher.publish(topics::MainConversation, event).await {
        tracing::warn!(error = %e, "failed to publish main conversation event");
    }
}

/// Announce a turn: its opening person's message and its start to the main
/// conversation, whatever endpoint it is delivered to, and
/// `TurnLifecycleEvent::Started` to the output endpoint when that is a chat
/// interface (see [`chat_interface_endpoint`]).
/// Also spawns the turn-start checkpoint, which captures the workspace state
/// before anything this turn does, attributed as an outside edit.
async fn publish_turn_started(
    rt: &AgentRuntime,
    output_endpoint: Option<&EndpointName>,
    message: &MessageEvent,
    origin: TurnOrigin,
) {
    let correlation_id = message.id.as_str();
    if let Some(echo) = MainConversationEvent::user_message(message, correlation_id) {
        publish_main_conversation(&rt.publisher, echo).await;
    }
    publish_main_conversation(
        &rt.publisher,
        MainConversationEvent::TurnStarted {
            turn_id: correlation_id.to_string(),
            origin,
        },
    )
    .await;
    publish_turn_started_to_chat_interface(&rt.publisher, output_endpoint, correlation_id).await;
    rt.checkpoints
        .spawn_turn_start_checkpoint(main_turn_checkpoint_context(
            correlation_id,
            crate::checkpoints::CheckpointTrigger::TurnStart,
            "outside edit before turn start".to_string(),
        ));
}

/// Publish a `TurnLifecycleEvent::Started` to the chat interface a turn is
/// delivered to, for its typing indicator. Nothing is published when the turn
/// is delivered to the web UI or nowhere (see [`chat_interface_endpoint`]).
async fn publish_turn_started_to_chat_interface(
    publisher: &Publisher,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
) {
    if let Some(ep) = chat_interface_endpoint(output_endpoint)
        && let Err(e) = publisher
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
    rt: &AgentRuntime,
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
/// nothing to an endpoint. The web UI's endpoint is no output endpoint for this
/// purpose: it has no subscriber to a turn's events, and reads the reply, a failure
/// and the end of the turn from the main conversation and the system channel (see
/// [`chat_interface_endpoint`]).
///
/// However the turn ended, the main conversation gets `TurnEnded` last: it
/// follows every event the turn published there, and tells a client following
/// the conversation that the turn is over, whichever endpoint it was
/// delivered to.
///
/// On failure, logs the error, auto-reports the failure through
/// `tracing_service` (a no-op unless the user has enabled auto error
/// reporting), and publishes an `ErrorEvent` to the output endpoint, so a chat
/// interface can answer the conversation that started the turn, and on the
/// system notification channel regardless of output endpoint, for the web UI.
/// If there is an output endpoint it still closes the turn with `Ended`.
async fn publish_turn_outcome(
    turn_result: anyhow::Result<Vec<String>>,
    publisher: &Publisher,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
    tz: chrono_tz::Tz,
    tracing_service: &TracingService,
    cfg: &Config,
) {
    let output_endpoint = chat_interface_endpoint(output_endpoint);
    match turn_result {
        Ok(texts) => {
            if let Some(ep) = output_endpoint {
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
        }
        Err(e) => {
            let described = crate::inference::describe_turn_failure(&e);
            tracing::error!(error = %described.details, "agent processing error");
            tracing_service
                .on_error(
                    &described.details,
                    crate::tracing_service::client_context::gather_for_bug_report(cfg),
                )
                .await;
            let event = ErrorEvent {
                correlation_id: correlation_id.to_string(),
                message: described.message,
                details: Some(described.details),
            };
            if let Some(ep) = output_endpoint
                && let Err(pub_err) = publisher
                    .publish(topics::Endpoint(ep.clone()), event.clone())
                    .await
            {
                tracing::warn!(error = %pub_err, endpoint = %ep, "failed to publish turn error to its endpoint");
            }
            if let Err(pub_err) = publisher
                .publish(
                    topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                    event,
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
    publish_main_conversation(
        publisher,
        MainConversationEvent::TurnEnded {
            turn_id: correlation_id.to_string(),
        },
    )
    .await;
}

/// What a turn's result says about its replies, read before
/// [`publish_turn_outcome`] consumes the result.
struct TurnReplies {
    /// How many replies went to an endpoint: main-conversation messages the
    /// web UI may not have shown yet.
    published: usize,
    /// The last reply that has text.
    last_text: Option<String>,
}

impl TurnReplies {
    fn of(result: &anyhow::Result<Vec<String>>, output_endpoint: Option<&EndpointName>) -> Self {
        let Ok(texts) = result else {
            return Self {
                published: 0,
                last_text: None,
            };
        };
        Self {
            published: if output_endpoint.is_some() {
                texts.len()
            } else {
                0
            },
            last_text: texts
                .iter()
                .rev()
                .find(|text| !text.trim().is_empty())
                .cloned(),
        }
    }
}

/// Tell the activity tracker a main turn is over: each published reply counts
/// as unread while no client is connected, then the turn hook runs. Called
/// exactly once per main turn, whatever its outcome.
fn report_turn_end(
    activity: &ActivityTracker,
    replies: TurnReplies,
    user_message: Option<String>,
    visibility: Visibility,
) {
    for _ in 0..replies.published {
        activity.main_message_published();
    }
    activity.main_turn_ended(user_message, replies.last_text, visibility);
}

/// Where a background turn's output goes: the `switch_endpoint` override if
/// the agent set one since the user last spoke, else the user's last endpoint.
fn background_output_endpoint(
    switched_to: Option<EndpointName>,
    last_user_endpoint: Option<&EndpointName>,
) -> Option<EndpointName> {
    switched_to.or_else(|| last_user_endpoint.cloned())
}

/// Sort out the interrupts still queued when a main turn ended: messages
/// among them are queued to run as turns of their own, or, when the agent is
/// stopping and no turn will follow, added to history as context.
async fn defer_late_messages(rt: &mut AgentRuntime, leftovers: Vec<Interrupt>, stopping: bool) {
    let late_messages = process_leftover_interrupts(leftovers, &mut rt.agent, &rt.agent_messenger);
    if stopping {
        inject_undelivered_messages(
            &mut rt.agent,
            &rt.agent_messenger,
            &rt.layout,
            rt.tz,
            late_messages,
        )
        .await;
    } else {
        rt.deferred_inbound.extend(late_messages);
    }
}

/// Where a turn's replies go. Background turns go wherever `switch_endpoint`
/// last pointed them, else the user's last endpoint; user turns derive from
/// the origin and become the new last endpoint.
fn resolve_output_endpoint(
    rt: &mut AgentRuntime,
    origin: &MessageOrigin,
    is_background: bool,
) -> Option<EndpointName> {
    if is_background {
        return background_output_endpoint(
            rt.output_topic_override_tx.borrow().clone(),
            rt.last_output_endpoint.as_ref(),
        );
    }
    // Clear any switch_endpoint override so responses follow the user's endpoint.
    rt.output_topic_override_tx.send_replace(None);
    let ep = EndpointName::from(origin.endpoint.as_str());
    rt.last_output_endpoint = Some(ep.clone());
    Some(ep)
}

/// Handle an inbound user message: run agent turn, persist, observe, and process leftovers.
///
/// Returns whether the hub's stop request interrupted this turn — the caller
/// (`handle_bus_event`) must shut the agent down, since the request was
/// already consumed while stopping the turn (see
/// `run_agent_turn_with_interrupts`) and won't arrive again for the event
/// loop's own idle-time handling of it.
#[tracing::instrument(skip_all, fields(correlation_id = %message.id, origin = %message.origin.endpoint))]
pub async fn handle_inbound_message(
    message: MessageEvent,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let reply_id = message.id.clone();
    let origin = message.origin.clone();
    let is_background = origin.endpoint == BACKGROUND_ENDPOINT;
    let visibility = if is_background {
        Visibility::Background
    } else {
        Visibility::User
    };

    let output_endpoint = resolve_output_endpoint(rt, &origin, is_background);

    // Held for the whole turn, so the rail and Home show it busy until it
    // ends, however the turn ends.
    let _busy = rt.activity.main_turn();

    publish_turn_started(
        rt,
        output_endpoint.as_ref(),
        &message,
        TurnOrigin::new(&origin, visibility.clone()),
    )
    .await;

    let before = rt.agent.message_count();

    if let Some(context) = message.context.as_deref() {
        rt.agent.inject_system_message(context);
    }

    // This turn's kickoff input's hop count: 0 for a genuine external
    // message, or the hop count recorded when an agent message was
    // delivered to main (see `AgentMessenger::deliver_to_main`), or restored
    // for a message that arrived after the previous turn's last drain (see
    // `process_leftover_interrupts`). Folded in with `bump`, not overwritten
    // with `set`, so the counter never drops below what the kickoff carries.
    let kickoff_hop = rt.agent_messenger.take_main_hop(&message.id);
    rt.agent.hop_counter().bump(kickoff_hop);

    let ctx_strings = load_prompt_context_strings(&rt.skill_state).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (turn_result, leftover_interrupts, subconscious_scratch, stop_requested) =
        run_agent_turn_with_interrupts(
            &mut rt.agent,
            &rt.agent_messenger,
            &rt.conversation_router,
            &message.content,
            &rt.publisher,
            output_endpoint.as_ref(),
            &reply_id,
            Some(&origin),
            &prompt_ctx,
            &message.images,
            &mut rt.agent_subscriber,
            &mut rt.stop_rx,
            &mut rt.agent_stop_rx,
            (!is_background && rt.subconscious.mid_turn_enabled())
                .then(|| Arc::clone(&rt.subconscious)),
        )
        .await;

    spawn_main_turn_end_checkpoint(rt, &reply_id, &turn_result);

    let replies = TurnReplies::of(&turn_result, output_endpoint.as_ref());
    publish_turn_outcome(
        turn_result,
        &rt.publisher,
        output_endpoint.as_ref(),
        &reply_id,
        rt.tz,
        &rt.tracing_service,
        &rt.cfg,
    )
    .await;

    // A background turn was started by no user message.
    let user_message = (!is_background).then(|| message.content.clone());
    report_turn_end(&rt.activity, replies, user_message, visibility.clone());
    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(
        rt,
        &new_messages,
        visibility,
        observe_deadline,
        Some(&reply_id),
    )
    .await;

    // Background turns (including subconscious correction turns) are never
    // evaluated — this gate is what bounds the correction feedback loop.
    if !is_background {
        super::subconscious_hook::run_end_of_turn_subconscious(
            rt,
            &new_messages,
            &reply_id,
            subconscious_scratch.as_ref(),
        );
        maybe_nudge_learner(rt).await;
    }

    defer_late_messages(rt, leftover_interrupts, stop_requested).await;

    // Only update idle timer for user messages, not background turns.
    if !is_background && !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }

    stop_requested
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Subscriber;
    use crate::testing::wait;
    use crate::util::telemetry::{SpanBufferConfig, SpanBufferLayer};

    const TEST_TZ: chrono_tz::Tz = chrono_tz::Tz::UTC;

    /// A chat interface's endpoint: the kind a turn's events are published to.
    fn endpoint() -> EndpointName {
        EndpointName::from("telegram")
    }

    /// Build a minimal test config; only `tracing` is meaningful to
    /// `publish_turn_outcome`'s tests, the rest is filler to satisfy the type.
    fn test_config() -> Config {
        Config {
            agent_name: "test-agent".to_string(),
            autostart: true,
            main: vec![],
            observer: vec![],
            reflector: vec![],
            pulse: vec![],
            subconscious: vec![],
            embedding: None,
            workspace_dir: std::path::PathBuf::from("/tmp/test"),
            timeout_secs: 30,
            max_tokens: 4096,
            memory: crate::config::MemoryConfig::default(),
            pulse_enabled: false,
            subconscious_settings: crate::config::SubconsciousSettings::default(),
            learning: crate::config::LearningConfig::default(),
            gateway: crate::config::GatewayConfig::default(),
            timezone: chrono_tz::UTC,
            cloud: None,
            discord: None,
            telegram: None,
            teams: None,
            a2a: crate::config::A2aConfig::default(),
            webhooks: std::collections::HashMap::new(),
            skills: crate::config::SkillsConfig { dirs: vec![] },
            tools: crate::config::ToolsConfig { dirs: vec![] },
            retry: crate::inference::retry::RetryConfig::default(),
            background: crate::config::BackgroundConfig::default(),
            agent: crate::config::AgentAbilitiesConfig::default(),
            auto_mode: crate::config::AutoModeConfig::default(),
            idle: crate::config::IdleConfig::default(),
            temperature: None,
            thinking: None,
            web_search: crate::config::WebSearchConfig::default(),
            tracing: crate::config::TracingConfig::default(),
            role_overrides: std::collections::HashMap::new(),
            config_dir: std::path::PathBuf::from("/tmp/config"),
            load_notices: vec![],
        }
    }

    /// A `TracingService` with auto error reporting off — `on_error` calls
    /// through it are a guaranteed no-op, for tests that don't care about
    /// auto-reporting behavior.
    fn test_tracing_service() -> TracingService {
        let (_, handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        TracingService::new(crate::config::TracingConfig::default(), handle)
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

    /// Assert `sub` was sent nothing. A bus barrier proves every event published
    /// before it has been handed to every subscriber, so what `sub` holds is
    /// everything it was sent.
    async fn assert_no_event<E: Clone + Send + Sync + 'static>(
        bus: &crate::bus::BusHandle,
        sub: &mut Subscriber<E>,
    ) {
        wait::bus_barrier(bus).await;
        assert!(
            sub.drain().is_empty(),
            "expected no event, but one was published"
        );
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
            &test_tracing_service(),
            &test_config(),
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

        publish_turn_outcome(
            Ok(vec![]),
            &publisher,
            Some(&endpoint()),
            "corr-2",
            TEST_TZ,
            &test_tracing_service(),
            &test_config(),
        )
        .await;

        let ended = lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "corr-2")
        );
        assert_no_event(&handle, &mut responses).await;
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
            &test_tracing_service(),
            &test_config(),
        )
        .await;

        assert_no_event(&handle, &mut responses).await;
        assert_no_event(&handle, &mut lifecycle).await;
    }

    #[tokio::test]
    async fn a_turn_start_goes_to_a_chat_interface_but_not_to_the_web_ui() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let ws = EndpointName::from(crate::interfaces::types::WEB_UI_ENDPOINT);
        let mut chat: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut web: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(ws.clone()))
            .await
            .unwrap();

        publish_turn_started_to_chat_interface(&publisher, Some(&ws), "corr-web").await;
        publish_turn_started_to_chat_interface(&publisher, None, "corr-none").await;
        publish_turn_started_to_chat_interface(&publisher, Some(&endpoint()), "corr-chat").await;

        let started = chat.recv().await.unwrap().unwrap();
        assert!(
            matches!(started, TurnLifecycleEvent::Started { correlation_id } if correlation_id == "corr-chat"),
            "the chat interface hears its turn start, and only its own"
        );
        assert_no_event(&handle, &mut web).await;
    }

    /// The web UI follows a turn through the main conversation and the system
    /// channel, so nothing is published to its endpoint topic, where no
    /// subscriber reads a turn's events.
    #[tokio::test]
    async fn a_turn_delivered_to_the_web_ui_publishes_nothing_to_its_endpoint_topic() {
        let ws = EndpointName::from(crate::interfaces::types::WEB_UI_ENDPOINT);
        for outcome in [Ok(vec!["reply".to_string()]), Err(anyhow::anyhow!("boom"))] {
            let failed = outcome.is_err();
            let handle = crate::bus::spawn_broker();
            let publisher = handle.publisher();
            let mut responses: Subscriber<ResponseEvent> = handle
                .subscribe(topics::Endpoint(ws.clone()))
                .await
                .unwrap();
            let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
                .subscribe(topics::Endpoint(ws.clone()))
                .await
                .unwrap();
            let mut endpoint_errors: Subscriber<ErrorEvent> = handle
                .subscribe(topics::Endpoint(ws.clone()))
                .await
                .unwrap();
            let mut system_errors: Subscriber<ErrorEvent> = handle
                .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
                .await
                .unwrap();
            let mut main: Subscriber<MainConversationEvent> =
                handle.subscribe(topics::MainConversation).await.unwrap();

            publish_turn_outcome(
                outcome,
                &publisher,
                Some(&ws),
                "corr-web",
                TEST_TZ,
                &test_tracing_service(),
                &test_config(),
            )
            .await;

            assert_no_event(&handle, &mut responses).await;
            assert_no_event(&handle, &mut lifecycle).await;
            assert_no_event(&handle, &mut endpoint_errors).await;
            let ended = main.recv().await.unwrap().unwrap();
            assert!(
                matches!(&ended, MainConversationEvent::TurnEnded { turn_id } if turn_id == "corr-web"),
                "the main conversation still ends the turn: {ended:?}"
            );
            if failed {
                let error = system_errors.recv().await.unwrap().unwrap();
                assert_eq!(
                    error.correlation_id, "corr-web",
                    "the web hears of a failure on the system channel"
                );
            } else {
                assert_no_event(&handle, &mut system_errors).await;
            }
        }
    }

    /// Run `outcome` through `publish_turn_outcome` and say which turn the
    /// main conversation ended.
    async fn main_conversation_end_of(
        outcome: anyhow::Result<Vec<String>>,
        output_endpoint: Option<&EndpointName>,
    ) -> Option<String> {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut main: Subscriber<MainConversationEvent> =
            handle.subscribe(topics::MainConversation).await.unwrap();

        publish_turn_outcome(
            outcome,
            &publisher,
            output_endpoint,
            "corr-5",
            TEST_TZ,
            &test_tracing_service(),
            &test_config(),
        )
        .await;

        // The bus barrier proves every event the turn published has reached `main`.
        wait::bus_barrier(&handle).await;
        match main.drain().into_iter().next() {
            Some(MainConversationEvent::TurnEnded { turn_id }) => Some(turn_id),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_turn_ends_in_the_main_conversation_however_it_ended() {
        let ep = endpoint();
        assert_eq!(
            main_conversation_end_of(Ok(vec!["reply".into()]), Some(&ep)).await,
            Some("corr-5".to_string()),
            "a reply delivered to an endpoint"
        );
        assert_eq!(
            main_conversation_end_of(Ok(vec!["reply".into()]), None).await,
            Some("corr-5".to_string()),
            "a reply with no endpoint to go to is still a turn that ended"
        );
        assert_eq!(
            main_conversation_end_of(Err(anyhow::anyhow!("boom")), Some(&ep)).await,
            Some("corr-5".to_string()),
            "a failed turn"
        );
        assert_eq!(
            main_conversation_end_of(Err(anyhow::anyhow!("boom")), None).await,
            Some("corr-5".to_string()),
            "a failed turn nobody was following"
        );
    }

    #[tokio::test]
    async fn err_broadcasts_error_event_and_ends_turn() {
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let mut endpoint_errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
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
            &test_tracing_service(),
            &test_config(),
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
            "the raw cause must survive in details for a developer or the Details disclosure"
        );
        let endpoint_error = endpoint_errors.recv().await.unwrap().unwrap();
        assert_eq!(
            endpoint_error.correlation_id, "corr-4",
            "the endpoint that started the turn must hear about its failure"
        );
        assert_eq!(endpoint_error.message, error.message);

        let ended = lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "corr-4")
        );
        assert_no_event(&handle, &mut responses).await;
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
        let mut endpoint_errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Err(anyhow::anyhow!("kaboom")),
            &publisher,
            None,
            "corr-5",
            TEST_TZ,
            &test_tracing_service(),
            &test_config(),
        )
        .await;

        let error = errors.recv().await.unwrap().unwrap();
        assert_eq!(error.correlation_id, "corr-5");
        assert_eq!(
            error.details.as_deref(),
            Some("kaboom"),
            "the raw cause must survive in details even with no output endpoint"
        );
        assert_no_event(&handle, &mut lifecycle).await;
        assert_no_event(&handle, &mut endpoint_errors).await;
    }

    #[tokio::test]
    async fn err_auto_reports_through_tracing_service_when_enabled() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bug-report"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "public_id": "RR-TURN-FAILURE",
                "submitted_at": "2026-04-16T14:23:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (_, span_handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        let tracing_service = TracingService::new(
            crate::config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..crate::config::TracingConfig::default()
            },
            span_handle,
        );

        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        publish_turn_outcome(
            Err(anyhow::anyhow!("model completion failed: connection reset")),
            &publisher,
            None,
            "corr-6",
            TEST_TZ,
            &tracing_service,
            &test_config(),
        )
        .await;

        // The mock's `.expect(1)` (checked on drop) is the real assertion —
        // this recv just drains the event the Err arm always publishes.
        errors.recv().await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn ok_never_auto_reports_even_when_enabled() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bug-report"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "public_id": "RR-SHOULD-NOT-FIRE",
                "submitted_at": "2026-04-16T14:23:00Z"
            })))
            .expect(0)
            .mount(&server)
            .await;

        let (_, span_handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        let tracing_service = TracingService::new(
            crate::config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..crate::config::TracingConfig::default()
            },
            span_handle,
        );

        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        // An `Ok` outcome is what a user-cancelled or otherwise expected turn
        // ending looks like by the time it reaches `publish_turn_outcome` —
        // `execute_turn` never turns those into `Err` (see turn.rs).
        publish_turn_outcome(
            Ok(vec!["done".into()]),
            &publisher,
            Some(&endpoint()),
            "corr-7",
            TEST_TZ,
            &tracing_service,
            &test_config(),
        )
        .await;

        lifecycle.recv().await.unwrap().unwrap();
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

    /// A `GatedProvider` that also announces when its model call begins, so a
    /// test can act strictly after the turn's last checkpoint drain.
    struct AnnouncingGatedProvider {
        started: Arc<tokio::sync::Notify>,
        inner: GatedProvider,
    }

    #[async_trait::async_trait]
    impl crate::inference::InferenceProvider for AnnouncingGatedProvider {
        async fn complete(
            &self,
            messages: &[crate::inference::Message],
            tools: &[crate::inference::ToolDefinition],
            options: &crate::inference::CompletionOptions,
        ) -> Result<crate::inference::InferenceResponse, crate::inference::InferenceError> {
            self.started.notify_one();
            self.inner.complete(messages, tools, options).await
        }

        fn model_name(&self) -> &'static str {
            "announcing-gated"
        }
    }

    fn test_agent(provider: impl crate::inference::InferenceProvider + 'static) -> Agent {
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
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let (_agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = crate::util::spawn_in_span(async move {
            let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
                "hello",
                &publisher,
                None,
                "corr-hop",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
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
        wait::until_true(
            "the hop counter to bump to 7 once the mid-turn message is drained",
            || hop_counter.get() >= 7,
        )
        .await;

        gate.notify_one();
        let finished_agent =
            wait::guarded("turn should complete once the gate releases", turn_task)
                .await
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
        let (stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(4);
        let (_agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);

        // A stop request that arrived in the gap between the *previous*
        // turn's own select loop ending and this turn's own select loop
        // starting to watch `stop_rx` — exactly the window nothing used to
        // drain.
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

        let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
            &mut agent,
            &messenger,
            &conversation_router,
            "hello",
            &publisher,
            None,
            "corr-stale-stop",
            None,
            &prompt_ctx,
            &[],
            &mut agent_subscriber,
            &mut stop_rx,
            &mut agent_stop_rx,
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
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let (_agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = crate::util::spawn_in_span(async move {
            let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
                "hello",
                &publisher,
                None,
                "corr-route",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
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
        let spawn_event = wait::next_event(
            "the group-chat message to start its own conversation session",
            &mut spawns,
        )
        .await;
        assert_eq!(spawn_event.address, address);
        assert_eq!(spawn_event.prompt, "anyone around?");

        assert_eq!(
            hop_counter.get(),
            0,
            "a message that doesn't belong to main must never bump its hop counter"
        );

        gate.notify_one();
        let finished_agent =
            wait::guarded("turn should complete once the gate releases", turn_task)
                .await
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

    fn sample_inbound(content: &str) -> crate::bus::MessageEvent {
        crate::bus::MessageEvent {
            id: "leftover-1".to_string(),
            content: content.to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            context: None,
        }
    }

    /// A messenger over a throwaway bus, for tests that call
    /// `process_leftover_interrupts` directly.
    fn test_messenger() -> Arc<crate::background::messaging::AgentMessenger> {
        let handle = crate::bus::spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        Arc::new(crate::background::messaging::AgentMessenger::new(
            Arc::new(crate::background::registry::SessionRegistry::new()),
            handle.publisher(),
            store,
            crate::background::HopLimits { soft: 8, hard: 32 },
        ))
    }

    fn history_mentions(agent: &Agent, needle: &str) -> usize {
        agent
            .messages_since(0)
            .iter()
            .filter(|m| m.content.contains(needle))
            .count()
    }

    #[tokio::test]
    async fn no_leftovers_resets_the_hop_counter_for_a_clean_next_turn() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        // What a turn's own mid-turn bumps left behind for messages it
        // actually consumed and already replied to — nothing should carry
        // forward from that once the turn is over.
        agent.hop_counter().bump(9);

        let late = process_leftover_interrupts(vec![], &mut agent, &messenger);

        assert!(late.is_empty());
        assert_eq!(
            agent.hop_counter().get(),
            0,
            "a turn with nothing left over must not leak its already-answered hop count \
             into the next turn"
        );
    }

    #[tokio::test]
    async fn a_leftover_user_message_is_returned_to_start_a_turn_and_not_injected() {
        // A message that arrived after the turn's last checkpoint drain: no
        // model call saw it, so it must start a turn of its own rather than
        // sit in history as context nobody acts on.
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        agent.hop_counter().bump(9);
        messenger.note_mid_turn_hop("leftover-1", 9);

        let late = process_leftover_interrupts(
            vec![Interrupt::UserMessage(sample_inbound("still looping"))],
            &mut agent,
            &messenger,
        );

        let [message] = late.as_slice() else {
            panic!("expected exactly one requeued message, got {}", late.len());
        };
        assert_eq!(message.id, "leftover-1");
        assert_eq!(message.content, "still looping");
        assert_eq!(
            history_mentions(&agent, "still looping"),
            0,
            "the message is delivered by the turn it starts, not also as context"
        );
        assert_eq!(
            agent.hop_counter().get(),
            0,
            "the hop count travels with the message, not in the counter"
        );
        assert_eq!(
            messenger.take_main_hop("leftover-1"),
            9,
            "the loop guard's hop count must survive to the message's own turn"
        );
    }

    #[tokio::test]
    async fn leftover_messages_come_back_in_arrival_order() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        let mut second = sample_inbound("second");
        second.id = "leftover-2".to_string();

        let late = process_leftover_interrupts(
            vec![
                Interrupt::UserMessage(sample_inbound("first")),
                Interrupt::UserMessage(second),
            ],
            &mut agent,
            &messenger,
        );

        let contents: Vec<_> = late.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["first", "second"]);
    }

    #[tokio::test]
    async fn a_leftover_agent_message_keeps_its_sender_and_hop_count() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();

        let late = process_leftover_interrupts(
            vec![Interrupt::AgentMessage(crate::bus::AgentMessageEvent {
                from: crate::bus::SessionAddress::from("spawned-researcher-0001"),
                from_category: "spawned".to_string(),
                content: "still looping".to_string(),
                hop_count: 12,
            })],
            &mut agent,
            &messenger,
        );

        let [message] = late.as_slice() else {
            panic!("expected exactly one requeued message, got {}", late.len());
        };
        assert!(message.content.contains("still looping"));
        let sender = message
            .origin
            .agent_sender
            .as_ref()
            .expect("the requeued message keeps its agent attribution");
        assert_eq!(sender.address, "spawned-researcher-0001");
        assert_eq!(messenger.take_main_hop(&message.id), 12);
        assert_eq!(history_mentions(&agent, "still looping"), 0);
    }

    #[tokio::test]
    async fn a_leftover_subconscious_note_degrades_to_history_and_starts_no_turn() {
        // A subconscious correction has no sender waiting on a reply.
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        agent.hop_counter().bump(9);

        let late = process_leftover_interrupts(
            vec![Interrupt::Subconscious("[Subconscious] note".to_string())],
            &mut agent,
            &messenger,
        );

        assert!(late.is_empty());
        assert_eq!(history_mentions(&agent, "[Subconscious] note"), 1);
        assert_eq!(agent.hop_counter().get(), 0);
    }

    #[tokio::test]
    async fn a_leftover_stop_marker_changes_nothing() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();

        let late = process_leftover_interrupts(vec![Interrupt::Stopped], &mut agent, &messenger);

        assert!(late.is_empty());
        assert_eq!(agent.message_count(), 0);
    }

    #[tokio::test]
    async fn undelivered_messages_are_folded_into_history_once_and_drop_their_hop_entry() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        messenger.restore_main_hop("leftover-1", 4);

        inject_undelivered_messages(
            &mut agent,
            &messenger,
            &layout,
            TEST_TZ,
            vec![sample_inbound("stopping soon")],
        )
        .await;

        assert_eq!(history_mentions(&agent, "stopping soon"), 1);
        assert_eq!(messenger.take_main_hop("leftover-1"), 0);
    }

    #[tokio::test]
    async fn undelivered_messages_are_persisted_so_they_survive_a_restart() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let mut second = sample_inbound("second note");
        second.id = "leftover-2".to_string();

        inject_undelivered_messages(
            &mut agent,
            &messenger,
            &layout,
            TEST_TZ,
            vec![sample_inbound("first note"), second],
        )
        .await;

        // A restart rebuilds history from this file.
        let persisted =
            crate::memory::recent_messages::load_recent_messages(&layout.recent_messages_json())
                .await
                .unwrap();
        let contents: Vec<_> = persisted
            .iter()
            .map(|m| m.message.content.as_str())
            .collect();
        assert!(
            contents.iter().any(|c| c.contains("first note"))
                && contents.iter().any(|c| c.contains("second note")),
            "both stop-time messages must be on disk, got {contents:?}"
        );
    }

    #[tokio::test]
    async fn drained_bus_messages_are_folded_in_with_deferred_inbound_at_shutdown() {
        // Mirrors `graceful_shutdown`'s undelivered-message assembly: a
        // message still waiting for its turn (`deferred_inbound`) is chained
        // with whatever was published to the bus but never read
        // (`agent_subscriber.drain()`) before both are folded into history
        // and persisted together.
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        let mut on_bus = sample_inbound("still on the bus, unread");
        on_bus.id = "bus-1".to_string();
        publisher
            .publish(topics::UserMessage, on_bus)
            .await
            .unwrap();
        // The broker handles commands one at a time in order, so once a
        // second subscriber has seen this marker, the broker has already
        // finished offering the event above to `agent_subscriber` too.
        let mut barrier: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let mut marker = sample_inbound("marker");
        marker.id = "marker".to_string();
        publisher
            .publish(topics::UserMessage, marker)
            .await
            .unwrap();
        loop {
            let seen = barrier.recv().await.unwrap().unwrap();
            if seen.id == "marker" {
                break;
            }
        }

        let mut deferred = sample_inbound("waiting for its turn");
        deferred.id = "deferred-1".to_string();
        let mut undelivered = vec![deferred];
        undelivered.extend(agent_subscriber.drain());

        inject_undelivered_messages(&mut agent, &messenger, &layout, TEST_TZ, undelivered).await;

        assert_eq!(history_mentions(&agent, "waiting for its turn"), 1);
        assert_eq!(history_mentions(&agent, "still on the bus, unread"), 1);
        assert_eq!(history_mentions(&agent, "marker"), 1);

        let persisted =
            crate::memory::recent_messages::load_recent_messages(&layout.recent_messages_json())
                .await
                .unwrap();
        let contents: Vec<_> = persisted
            .iter()
            .map(|m| m.message.content.as_str())
            .collect();
        assert!(
            contents.iter().any(|c| c.contains("waiting for its turn"))
                && contents
                    .iter()
                    .any(|c| c.contains("still on the bus, unread")),
            "both the deferred message and the drained bus message must be on disk, got {contents:?}"
        );
    }

    #[tokio::test]
    async fn a_late_user_message_after_teammate_traffic_restores_hop_zero() {
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        // A teammate message the turn consumed mid-turn pushed the shared
        // counter to 9; the user message that arrived afterwards carried none.
        agent.hop_counter().bump(9);

        let late = process_leftover_interrupts(
            vec![Interrupt::UserMessage(sample_inbound("hello again"))],
            &mut agent,
            &messenger,
        );

        assert_eq!(late.len(), 1);
        assert_eq!(
            messenger.take_main_hop("leftover-1"),
            0,
            "a user message must not inherit the turn's teammate hops"
        );
    }

    /// Everything one main turn needs besides the agent's model, held
    /// together so a test can hand it to a task and take it back.
    struct TurnRig {
        agent: Agent,
        subscriber: Subscriber<MessageEvent>,
        stop_rx: mpsc::Receiver<StopRequest>,
        agent_stop_rx: mpsc::Receiver<()>,
        publisher: Publisher,
        messenger: Arc<crate::background::messaging::AgentMessenger>,
        router: Arc<crate::background::ConversationRouter>,
        /// Kept alive for the rig's lifetime: the broker, and the stop
        /// channels' senders (a closed channel would make the turn's select
        /// arm for it fire on every poll).
        keep_alive: (
            crate::bus::BusHandle,
            mpsc::Sender<StopRequest>,
            mpsc::Sender<()>,
        ),
    }

    impl TurnRig {
        fn bus(&self) -> &crate::bus::BusHandle {
            &self.keep_alive.0
        }

        async fn new(provider: impl crate::inference::InferenceProvider + 'static) -> Self {
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
            let router = Arc::new(crate::background::ConversationRouter::new(Arc::clone(
                &messenger,
            )));
            let subscriber = handle.subscribe(topics::UserMessage).await.unwrap();
            let (stop_tx, stop_rx) = mpsc::channel::<StopRequest>(1);
            let (agent_stop_tx, agent_stop_rx) = mpsc::channel::<()>(1);
            Self {
                agent: test_agent(provider),
                subscriber,
                stop_rx,
                agent_stop_rx,
                publisher,
                messenger,
                router,
                keep_alive: (handle, stop_tx, agent_stop_tx),
            }
        }

        /// Run one turn and return the interrupts left queued when it ended.
        async fn run_turn(
            &mut self,
            content: &str,
            correlation_id: &str,
            origin: Option<&MessageOrigin>,
        ) -> Vec<Interrupt> {
            let prompt_ctx = PromptContext {
                skills: crate::agent::context::SkillsContext {
                    index: None,
                    active_instructions: None,
                },
            };
            let (result, leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut self.agent,
                &self.messenger,
                &self.router,
                content,
                &self.publisher,
                None,
                correlation_id,
                origin,
                &prompt_ctx,
                &[],
                &mut self.subscriber,
                &mut self.stop_rx,
                &mut self.agent_stop_rx,
                None,
            )
            .await;
            result.expect("the turn completes");
            leftovers
        }
    }

    #[tokio::test]
    async fn a_message_arriving_during_the_final_model_call_starts_a_turn_and_is_recorded_once() {
        // The first turn's only model call is also its last: it answers with
        // text and no tool calls, so no checkpoint follows it.
        let gate = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let mut first_rig = TurnRig::new(AnnouncingGatedProvider {
            started: Arc::clone(&started),
            inner: GatedProvider {
                gate: Arc::clone(&gate),
                response: "first answer".to_string(),
            },
        })
        .await;
        let hop_counter = first_rig.agent.hop_counter().clone();
        let messenger = Arc::clone(&first_rig.messenger);

        let first_turn = crate::util::spawn_in_span(async move {
            let leftovers = first_rig.run_turn("kickoff", "corr-late-1", None).await;
            let late =
                process_leftover_interrupts(leftovers, &mut first_rig.agent, &first_rig.messenger);
            (first_rig, late)
        });

        // The turn's only checkpoint drain precedes the model call, so once
        // the call has started any message is necessarily late.
        wait::guarded("the first turn to reach its model call", started.notified()).await;
        // A teammate's message lands while the model is still producing the
        // turn's final answer. Its hop count doubles as the signal that the
        // turn's select loop has taken it into the interrupt queue, so the
        // gate is released only once the message is provably in flight.
        messenger
            .send(
                "main",
                crate::bus::SessionAddress::from("agent-beta"),
                "teammate".to_string(),
                "please review".to_string(),
                5,
            )
            .await
            .unwrap();
        wait::until_true(
            "the turn to take the message into its interrupt queue",
            || hop_counter.get() >= 5,
        )
        .await;
        gate.notify_one();

        let (mut rig, late) =
            wait::guarded("the first turn ends once the gate releases", first_turn)
                .await
                .unwrap();

        let [message] = late.as_slice() else {
            panic!("expected the late message back, got {}", late.len());
        };
        assert_eq!(
            history_mentions(&rig.agent, "please review"),
            0,
            "not injected as context on top of being delivered as a turn"
        );
        wait::bus_barrier(rig.bus()).await;
        assert!(
            rig.subscriber.drain().is_empty(),
            "requeueing is local, not a second broadcast on the bus"
        );

        // The event loop runs it as the next turn's kickoff.
        assert_eq!(messenger.take_main_hop(&message.id), 5);
        gate.notify_one();
        let before = rig.agent.message_count();
        let leftovers = rig
            .run_turn(&message.content, "corr-late-2", Some(&message.origin))
            .await;

        assert!(leftovers.is_empty());
        assert_eq!(
            history_mentions(&rig.agent, "please review"),
            1,
            "the message appears exactly once in history"
        );
        let recorded = rig
            .agent
            .messages_since(before)
            .iter()
            .find(|m| m.content.contains("please review"))
            .expect("the second turn recorded the message");
        assert_eq!(
            recorded.agent_sender.as_ref().map(|s| s.address.as_str()),
            Some("agent-beta"),
            "the turn keeps the teammate attribution"
        );
    }

    #[tokio::test]
    async fn a_stop_request_ends_an_active_turn_and_reports_it() {
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
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let (agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);

        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };

        let turn_task = crate::util::spawn_in_span(async move {
            run_agent_turn_with_interrupts(
                &mut agent,
                &messenger,
                &conversation_router,
                "hello",
                &publisher,
                None,
                "corr-shutdown",
                None,
                &prompt_ctx,
                &[],
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
                None,
            )
            .await
        });

        // The turn is now blocked on the gated model call; the hub asks the
        // agent to stop, the way a stop, restart, or hub shutdown does.
        agent_stop_tx.send(()).await.unwrap();

        let (turn_result, _leftovers, _scratch, stopped) = wait::guarded(
            "a stop request should stop the turn well within 2s",
            turn_task,
        )
        .await
        .unwrap();

        assert!(
            stopped,
            "the caller must learn that the stop request ended the turn"
        );
        let texts = turn_result.expect("a stop-interrupted turn is not a turn error");
        assert!(
            texts.is_empty(),
            "the turn was cut short before producing a reply"
        );
    }

    /// A provider whose first model call fails, as one does when the API
    /// keeps erroring past every retry, and whose later calls answer. It
    /// records the roles of the messages each call carried.
    struct FailsOnce {
        calls: std::sync::atomic::AtomicUsize,
        requests: Arc<std::sync::Mutex<Vec<Vec<crate::inference::Role>>>>,
    }

    #[async_trait::async_trait]
    impl crate::inference::InferenceProvider for FailsOnce {
        async fn complete(
            &self,
            messages: &[crate::inference::Message],
            _tools: &[crate::inference::ToolDefinition],
            _options: &crate::inference::CompletionOptions,
        ) -> Result<crate::inference::InferenceResponse, crate::inference::InferenceError> {
            self.requests
                .lock()
                .unwrap()
                .push(messages.iter().map(|m| m.role).collect());
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(crate::inference::InferenceError::Api(
                    "the model is down".to_string(),
                ))
            } else {
                Ok(crate::inference::InferenceResponse::new(
                    "back".to_string(),
                    vec![],
                ))
            }
        }

        fn model_name(&self) -> &'static str {
            "fails-once"
        }
    }

    /// What `handle_inbound_message` does with a turn that fails: the turn's
    /// new messages are persisted whatever its result was, so the user's
    /// message is recorded, with no assistant reply and no note of the
    /// failure, and stays in the conversation the next turn sends the model.
    #[tokio::test]
    async fn a_failed_turn_keeps_the_users_message_and_records_nothing_else() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = test_agent(FailsOnce {
            calls: std::sync::atomic::AtomicUsize::new(0),
            requests: Arc::clone(&requests),
        });
        let handle = crate::bus::spawn_broker();
        let publisher = handle.publisher();
        let ep = endpoint();
        let prompt_ctx = PromptContext {
            skills: crate::agent::context::SkillsContext {
                index: None,
                active_instructions: None,
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let recent_path = dir.path().join("recent_messages.json");

        let before = agent.message_count();
        let mut interrupts = crate::agent::interrupt::dead_interrupt_rx();
        let failed = agent
            .process_message(
                "are you there?",
                &publisher,
                Some(&ep),
                "corr-failed",
                None,
                &prompt_ctx,
                &mut interrupts,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await;
        assert!(failed.is_err(), "the model call failed");

        let new_messages = agent.messages_since(before).to_vec();
        crate::memory::recent_messages::append_recent_messages(
            &recent_path,
            &new_messages,
            Visibility::User,
            TEST_TZ,
            Some("corr-failed"),
        )
        .await
        .unwrap();
        let saved = crate::memory::recent_messages::load_recent_messages(&recent_path)
            .await
            .unwrap();
        let saved: Vec<_> = saved
            .iter()
            .map(|recent| {
                (
                    recent.message.role,
                    recent.message.content.as_str(),
                    recent.turn_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            saved,
            [(
                crate::inference::Role::User,
                "are you there?",
                Some("corr-failed")
            )],
            "the user's message is in recent_messages.json under its turn, alone"
        );

        agent
            .process_message(
                "hello again",
                &publisher,
                Some(&ep),
                "corr-next",
                None,
                &prompt_ctx,
                &mut interrupts,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        let conversation: Vec<_> = requests
            .last()
            .unwrap()
            .iter()
            .copied()
            .filter(|role| *role != crate::inference::Role::System)
            .collect();
        assert_eq!(
            conversation,
            [crate::inference::Role::User, crate::inference::Role::User],
            "the next turn sends the model the unanswered message and the new one"
        );
    }
}

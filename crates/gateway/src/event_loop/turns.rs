use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use residuum_agent::context::{IntermediateDelivery, PromptContext, SkillsContext};
use residuum_agent::{Agent, FailedTurn, LiveOutputEndpoint, TurnOutput, TurnWiring};
use residuum_bus::interrupt::{Interrupt, InterruptSource};
use residuum_bus::{
    EndpointName, ErrorEvent, MainConversationEvent, MessageEvent, NotifyName, Publisher,
    ResponseEvent, SYSTEM_CHANNEL, Subscriber, TurnLifecycleEvent, TurnOrigin, topics,
};

use crate::queued_messages::{CheckpointTake, QueuedMessages};
use crate::types::{AgentRuntime, RetryTurnRequest};
use residuum_bus::control::StopRequest;
use residuum_bus::origin::{BACKGROUND_ENDPOINT, MessageOrigin, chat_interface_endpoint};
use residuum_config::Config;
use residuum_hub::activity::ActivityTracker;
use residuum_inference::ImageData;
use residuum_memory::types::Visibility;
use residuum_skills::SharedSkillState;
use residuum_tracing_service::TracingService;

use crate::memory::MemorySubsystems;
use residuum_agent::context::loading::build_skill_context_strings;

pub struct PromptContextStrings {
    pub skill_index: Option<String>,
    pub skill_active: Option<String>,
    pub intermediate_delivery: IntermediateDelivery,
}

impl PromptContextStrings {
    pub(super) fn as_prompt_context(&self) -> PromptContext<'_> {
        PromptContext {
            skills: SkillsContext {
                index: self.skill_index.as_deref(),
                active_instructions: self.skill_active.as_deref(),
            },
            intermediate_delivery: self.intermediate_delivery,
        }
    }
}

pub async fn load_prompt_context_strings(
    skill_state: &SharedSkillState,
    cfg: &Config,
) -> PromptContextStrings {
    let (skill_index, skill_active) = build_skill_context_strings(skill_state).await;
    PromptContextStrings {
        skill_index,
        skill_active,
        intermediate_delivery: IntermediateDelivery::from_config(cfg),
    }
}

pub fn process_leftover_interrupts(
    leftovers: Vec<Interrupt>,
    agent: &mut Agent,
    messenger: &residuum_background::messaging::AgentMessenger,
) -> Vec<MessageEvent> {
    let mut late_messages = Vec::new();
    for intr in leftovers {
        match intr {
            Interrupt::UserMessage(leftover_msg) => {
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

pub(super) async fn inject_undelivered_messages(
    agent: &mut Agent,
    messenger: &residuum_background::messaging::AgentMessenger,
    layout: &residuum_config::WorkspaceLayout,
    tz: chrono_tz::Tz,
    messages: impl IntoIterator<Item = MessageEvent>,
) {
    let mut senders = Vec::new();
    for message in messages {
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
        if let Err(e) = residuum_memory::recent_messages::append_recent_messages(
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

pub fn drain_interrupts(interrupt_rx: &mut mpsc::UnboundedReceiver<Interrupt>) -> Vec<Interrupt> {
    let mut leftovers = Vec::new();
    while let Ok(intr) = interrupt_rx.try_recv() {
        leftovers.push(intr);
    }
    leftovers
}

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

pub async fn persist_and_maybe_observe(
    rt: &mut AgentRuntime,
    new_messages: &[residuum_inference::Message],
    visibility: Visibility,
    observe_deadline: &mut Option<tokio::time::Instant>,
    turn_id: Option<&str>,
) {
    use crate::memory::persist_and_check_thresholds;

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
        let mem = MemorySubsystems::from_runtime(rt);
        rt.memory_worker.trigger(mem);
    }
}

fn apply_observe_action(
    action: residuum_memory::observer::ObserveAction,
    observe_deadline: &mut Option<tokio::time::Instant>,
    cooldown_secs: u64,
) -> bool {
    use residuum_memory::observer::ObserveAction;
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

struct MainInterrupts<'a> {
    rx: mpsc::UnboundedReceiver<Interrupt>,
    queued: &'a QueuedMessages,
    held: Vec<MessageEvent>,
}

impl<'a> MainInterrupts<'a> {
    fn new(rx: mpsc::UnboundedReceiver<Interrupt>, queued: &'a QueuedMessages) -> Self {
        Self {
            rx,
            queued,
            held: Vec::new(),
        }
    }

    fn into_leftovers(mut self) -> Vec<Interrupt> {
        let mut leftovers: Vec<Interrupt> =
            self.held.drain(..).map(Interrupt::UserMessage).collect();
        for interrupt in drain_interrupts(&mut self.rx) {
            if let Interrupt::UserMessage(msg) = &interrupt
                && !self.queued.keep_after_turn(&msg.id)
            {
                tracing::info!(msg_id = %msg.id, "dropped a message withdrawn before the agent read it");
                continue;
            }
            leftovers.push(interrupt);
        }
        leftovers
    }
}

impl InterruptSource for MainInterrupts<'_> {
    fn try_recv(&mut self) -> Result<Interrupt, mpsc::error::TryRecvError> {
        loop {
            let interrupt = self.rx.try_recv()?;
            let Interrupt::UserMessage(msg) = interrupt else {
                return Ok(interrupt);
            };
            match self.queued.take_at_checkpoint(&msg.id) {
                CheckpointTake::Deliver => return Ok(Interrupt::UserMessage(msg)),
                CheckpointTake::Hold => {
                    tracing::info!(msg_id = %msg.id, "holding a message to send now for the next turn");
                    self.held.push(msg);
                }
                CheckpointTake::Drop => {
                    tracing::info!(msg_id = %msg.id, "dropped a message withdrawn before the agent read it");
                }
            }
        }
    }
}

async fn handle_mid_turn_message(
    next_msg: Result<Option<MessageEvent>, residuum_bus::BusError>,
    agent_messenger: &residuum_background::messaging::AgentMessenger,
    conversation_router: &Arc<residuum_background::ConversationRouter>,
    interrupt_tx: &mpsc::UnboundedSender<Interrupt>,
    hop_counter: &residuum_agent::HopCounter,
    waiting: (&QueuedMessages, &Publisher),
    follow: Option<&mut OwnerFollow<'_>>,
) {
    match next_msg {
        Ok(Some(inbound)) => {
            if inbound.origin.belongs_to_main() {
                if let Some(follow) = follow
                    && inbound.origin.is_from_person()
                {
                    let endpoint = EndpointName::from(inbound.origin.endpoint.as_str());
                    follow.owner_wrote_from(endpoint, waiting.1).await;
                }
                let hop = agent_messenger.take_main_hop(&inbound.id);
                let id = inbound.id.clone();
                if let Some(queued_event) = MainConversationEvent::message_queued(&inbound) {
                    let (queued, publisher) = waiting;
                    queued.enqueue(&id);
                    publish_main_conversation(publisher, queued_event).await;
                }
                if interrupt_tx.send(Interrupt::UserMessage(inbound)).is_ok() {
                    hop_counter.bump(hop);
                    agent_messenger.note_mid_turn_hop(&id, hop);
                } else {
                    tracing::warn!("interrupt channel closed, dropping user message mid-turn");
                }
            } else {
                let router = Arc::clone(conversation_router);
                residuum_core::util::spawn_in_span(async move { router.route(inbound).await });
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

enum TurnOpening<'m> {
    Message {
        content: &'m str,
        origin: Option<&'m MessageOrigin>,
        images: &'m [ImageData],
    },
    Rerun(FailedTurn),
}

fn refuse_retry_mid_turn(request: RetryTurnRequest, correlation_id: &str) {
    tracing::info!(
        requested = %request.id,
        active = %correlation_id,
        "refused a retry while a turn is running"
    );
    request
        .result_tx
        .send(Err(RETRY_REFUSED_TURN_RUNNING.to_string()))
        .ok();
}

const RETRY_REFUSED_TURN_RUNNING: &str = "The agent is working on a reply right now. Wait for it to finish, then try again if you still need to.";

const RETRY_REFUSED_MOVED_ON: &str = "That message can't be retried anymore because the conversation has moved on. Send it again to try once more.";

#[expect(
    clippy::too_many_arguments,
    reason = "publisher and topic params added during bus migration"
)]
async fn run_agent_turn_with_interrupts(
    agent: &mut Agent,
    agent_messenger: &residuum_background::messaging::AgentMessenger,
    conversation_router: &Arc<residuum_background::ConversationRouter>,
    opening: TurnOpening<'_>,
    publisher: &Publisher,
    output: TurnOutput<'_>,
    correlation_id: &str,
    prompt_ctx: &PromptContext<'_>,
    agent_subscriber: &mut Subscriber<MessageEvent>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    agent_stop_rx: &mut mpsc::Receiver<()>,
    retry_rx: &mut mpsc::Receiver<RetryTurnRequest>,
    subconscious: Option<Arc<residuum_agent::subconscious::Subconscious>>,
    queued: &QueuedMessages,
    mut follow: Option<OwnerFollow<'_>>,
) -> (
    anyhow::Result<Vec<String>>,
    Vec<Interrupt>,
    Option<Arc<std::sync::Mutex<residuum_agent::subconscious::TurnScratch>>>,
    bool,
) {
    drain_stale_stop_requests(stop_rx);

    let hop_counter = agent.hop_counter().clone();
    let (interrupt_tx, interrupt_rx) = mpsc::unbounded_channel::<Interrupt>();
    let mut interrupt_rx = MainInterrupts::new(interrupt_rx, queued);
    let watch = subconscious
        .map(|s| residuum_agent::subconscious::SubconsciousWatch::new(s, interrupt_tx.clone()));
    let stop_token = CancellationToken::new();
    let mut stop_requested = false;
    let turn_result = {
        let interrupts = &mut interrupt_rx;
        let watching = watch.as_ref();
        let stop = &stop_token;
        let mut turn = std::pin::pin!(async move {
            match opening {
                TurnOpening::Message {
                    content,
                    origin,
                    images,
                } => {
                    agent
                        .process_message(
                            content,
                            publisher,
                            output,
                            correlation_id,
                            origin,
                            prompt_ctx,
                            interrupts,
                            images,
                            watching,
                            stop,
                        )
                        .await
                }
                TurnOpening::Rerun(failed) => {
                    agent
                        .rerun_turn(
                            failed,
                            TurnWiring {
                                publisher,
                                output,
                                prompt_ctx,
                                interrupt_rx: interrupts,
                                subconscious: watching,
                                stop_token: stop,
                            },
                        )
                        .await
                }
            }
        });
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
                        (queued, publisher),
                        follow.as_mut(),
                    )
                    .await;
                }
                stop_req = stop_rx.recv() => {
                    handle_mid_turn_stop_request(stop_req, correlation_id, &stop_token, &interrupt_tx);
                }
                Some(request) = retry_rx.recv() => refuse_retry_mid_turn(request, correlation_id),
                _ = agent_stop_rx.recv() => {
                    stop_turn_for_agent_stop(correlation_id, &stop_token, &interrupt_tx);
                    stop_requested = true;
                }
            }
        }
    };

    let scratch = watch
        .as_ref()
        .map(residuum_agent::subconscious::SubconsciousWatch::scratch);

    drop(interrupt_tx);
    let leftover_interrupts = interrupt_rx.into_leftovers();

    (turn_result, leftover_interrupts, scratch, stop_requested)
}

async fn maybe_nudge_learner(rt: &mut AgentRuntime) {
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
        .publish(residuum_bus::topics::Background, spawn)
        .await
    {
        tracing::warn!(error = %e, "failed to publish learner nudge spawn request");
    } else {
        tracing::info!("learner sub-agent nudge spawn requested");
    }
}

async fn publish_main_conversation(publisher: &Publisher, event: MainConversationEvent) {
    if let Err(e) = publisher.publish(topics::MainConversation, event).await {
        tracing::warn!(error = %e, "failed to publish main conversation event");
    }
}

async fn publish_turn_started(
    rt: &AgentRuntime,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
    opening: Option<&MessageEvent>,
    origin: TurnOrigin,
) {
    if let Some(echo) =
        opening.and_then(|message| MainConversationEvent::user_message(message, correlation_id))
    {
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
            residuum_checkpoints::CheckpointTrigger::TurnStart,
            "outside edit before turn start".to_string(),
        ));
}

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

fn main_turn_checkpoint_context(
    correlation_id: &str,
    trigger: residuum_checkpoints::CheckpointTrigger,
    summary: String,
) -> residuum_checkpoints::CheckpointContext {
    residuum_checkpoints::CheckpointContext {
        address: "main".to_string(),
        run_id: None,
        turn_id: Some(correlation_id.to_string()),
        trigger,
        summary,
    }
}

fn main_turn_end_summary(turn_result: &anyhow::Result<Vec<String>>) -> String {
    match turn_result {
        Ok(texts) => texts.first().filter(|t| !t.is_empty()).map_or_else(
            || "turn completed".to_string(),
            |t| t.chars().take(120).collect(),
        ),
        Err(e) => format!("turn failed: {e}"),
    }
}

fn spawn_main_turn_end_checkpoint(
    rt: &AgentRuntime,
    correlation_id: &str,
    turn_result: &anyhow::Result<Vec<String>>,
) {
    rt.checkpoints
        .spawn_turn_end_checkpoint(main_turn_checkpoint_context(
            correlation_id,
            residuum_checkpoints::CheckpointTrigger::TurnEnd,
            main_turn_end_summary(turn_result),
        ));
}

async fn publish_turn_outcome(
    turn_result: anyhow::Result<Vec<String>>,
    publisher: &Publisher,
    output_endpoint: Option<&EndpointName>,
    correlation_id: &str,
    tz: chrono_tz::Tz,
    failure: &TurnFailureReport<'_>,
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
                                timestamp: residuum_core::time::now_local(tz),
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
            let described = residuum_inference::describe_turn_failure(&e);
            tracing::error!(error = %described.details, "agent processing error");
            failure
                .tracing_service
                .on_error(
                    &described.details,
                    residuum_tracing_service::client_context::gather_for_bug_report(
                        failure.build,
                        failure.cfg,
                    ),
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

struct TurnFailureReport<'a> {
    tracing_service: &'a TracingService,
    cfg: &'a Config,
    build: residuum_core::build_info::BuildInfo,
}

fn last_reply_text(result: &anyhow::Result<Vec<String>>) -> Option<String> {
    result
        .as_ref()
        .ok()?
        .iter()
        .rev()
        .find(|text| !text.trim().is_empty())
        .cloned()
}

fn report_turn_end(
    activity: &ActivityTracker,
    last_reply: Option<String>,
    user_message: Option<String>,
    visibility: Visibility,
) {
    activity.main_turn_ended(user_message, last_reply, visibility);
}

fn background_output_endpoint(
    switched_to: Option<EndpointName>,
    last_user_endpoint: Option<&EndpointName>,
) -> Option<EndpointName> {
    switched_to.or_else(|| last_user_endpoint.cloned())
}

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

fn resolve_output_endpoint(
    last_output_endpoint: &mut Option<EndpointName>,
    output_topic_override_tx: &tokio::sync::watch::Sender<Option<EndpointName>>,
    origin: &MessageOrigin,
    is_background: bool,
) -> Option<EndpointName> {
    if is_background {
        return background_output_endpoint(
            output_topic_override_tx.borrow().clone(),
            last_output_endpoint.as_ref(),
        );
    }
    let ep = EndpointName::from(origin.endpoint.as_str());
    follow_owner_to(last_output_endpoint, output_topic_override_tx, &ep);
    Some(ep)
}

fn follow_owner_to(
    last_output_endpoint: &mut Option<EndpointName>,
    output_topic_override_tx: &tokio::sync::watch::Sender<Option<EndpointName>>,
    endpoint: &EndpointName,
) {
    output_topic_override_tx.send_replace(None);
    *last_output_endpoint = Some(endpoint.clone());
}

fn reset_idle_timer(
    timeout: std::time::Duration,
    now: tokio::time::Instant,
    last_user_message_instant: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) {
    if timeout.is_zero() {
        return;
    }
    *last_user_message_instant = Some(now);
    *idle_deadline = Some(now + timeout);
}

fn note_owner_active_now(rt: &mut AgentRuntime, idle_deadline: &mut Option<tokio::time::Instant>) {
    reset_idle_timer(
        rt.cfg.idle.timeout,
        tokio::time::Instant::now(),
        &mut rt.last_user_message_instant,
        idle_deadline,
    );
}

struct OwnerFollow<'a> {
    output: &'a LiveOutputEndpoint,
    correlation_id: &'a str,
    last_output_endpoint: &'a mut Option<EndpointName>,
    output_topic_override_tx: &'a tokio::sync::watch::Sender<Option<EndpointName>>,
    idle_timeout: std::time::Duration,
    last_user_message_instant: &'a mut Option<tokio::time::Instant>,
    idle_deadline: &'a mut Option<tokio::time::Instant>,
}

impl OwnerFollow<'_> {
    async fn owner_wrote_from(&mut self, endpoint: EndpointName, publisher: &Publisher) {
        follow_owner_to(
            self.last_output_endpoint,
            self.output_topic_override_tx,
            &endpoint,
        );
        reset_idle_timer(
            self.idle_timeout,
            tokio::time::Instant::now(),
            self.last_user_message_instant,
            self.idle_deadline,
        );
        let previous = self.output.replace(endpoint.clone());
        if previous.as_ref() == Some(&endpoint) {
            return;
        }
        tracing::info!(
            correlation_id = %self.correlation_id,
            from = ?previous,
            to = %endpoint,
            "the owner wrote mid-turn, moving the agent-started turn's output to them"
        );
        if let Some(old) = chat_interface_endpoint(previous.as_ref()) {
            publish_turn_ended(publisher, old, self.correlation_id).await;
        }
        publish_turn_started_to_chat_interface(publisher, Some(&endpoint), self.correlation_id)
            .await;
    }
}

fn prepare_kickoff(rt: &mut AgentRuntime, message: &MessageEvent) {
    if let Some(context) = message.context.as_deref() {
        rt.agent.inject_system_message(context);
    }

    let kickoff_hop = rt.agent_messenger.take_main_hop(&message.id);
    rt.agent.hop_counter().bump(kickoff_hop);
}

enum MainTurnStart<'m> {
    Message(&'m MessageEvent),
    Rerun(FailedTurn),
}

#[tracing::instrument(skip_all, fields(correlation_id = %message.id, origin = %message.origin.endpoint))]
pub async fn handle_inbound_message(
    message: MessageEvent,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let origin = message.origin.clone();
    run_main_turn(
        MainTurnStart::Message(&message),
        &message.id,
        origin,
        rt,
        observe_deadline,
        idle_deadline,
    )
    .await
}

fn discard_queued_error_notes(errors: &mut Subscriber<ErrorEvent>) -> usize {
    errors.drain().len()
}

#[tracing::instrument(skip_all, fields(correlation_id = %request.id))]
pub async fn handle_retry_request(
    request: RetryTurnRequest,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let RetryTurnRequest { id, result_tx } = request;
    let Some(failed) = rt.agent.take_failed_turn(&id) else {
        tracing::info!(
            reason = RETRY_REFUSED_MOVED_ON,
            "refused a retry of a failed turn"
        );
        result_tx.send(Err(RETRY_REFUSED_MOVED_ON.to_string())).ok();
        return false;
    };
    result_tx.send(Ok(())).ok();
    let discarded = discard_queued_error_notes(&mut rt.error_subscriber);
    if discarded > 0 {
        tracing::debug!(
            count = discarded,
            "discarded error notes queued before a rerun"
        );
    }
    let origin = failed.origin().clone();
    run_main_turn(
        MainTurnStart::Rerun(failed),
        &id,
        origin,
        rt,
        observe_deadline,
        idle_deadline,
    )
    .await
}

fn open_main_turn<'a>(
    start: MainTurnStart<'a>,
    before: usize,
    origin: &'a MessageOrigin,
    rt: &mut AgentRuntime,
) -> (usize, String, TurnOpening<'a>) {
    let (turn_start, opening_text) = match &start {
        MainTurnStart::Message(message) => {
            prepare_kickoff(rt, message);
            (before, message.content.clone())
        }
        MainTurnStart::Rerun(failed) => {
            let text = rt
                .agent
                .messages_since(failed.kickoff_index())
                .first()
                .map(|opening| opening.content.clone())
                .unwrap_or_default();
            (failed.kickoff_index(), text)
        }
    };
    let opening = match start {
        MainTurnStart::Message(message) => TurnOpening::Message {
            content: &message.content,
            origin: Some(origin),
            images: &message.images,
        },
        MainTurnStart::Rerun(failed) => TurnOpening::Rerun(failed),
    };
    (turn_start, opening_text, opening)
}

async fn evaluate_person_turn(
    rt: &mut AgentRuntime,
    turn_start: usize,
    reply_id: &str,
    subconscious_scratch: Option<&Arc<std::sync::Mutex<residuum_agent::subconscious::TurnScratch>>>,
) {
    let turn_messages: Vec<_> = rt.agent.messages_since(turn_start).to_vec();
    super::subconscious_hook::run_end_of_turn_subconscious(
        rt,
        &turn_messages,
        reply_id,
        subconscious_scratch,
    );
    maybe_nudge_learner(rt).await;
}

async fn run_main_turn(
    start: MainTurnStart<'_>,
    reply_id: &str,
    origin: MessageOrigin,
    rt: &mut AgentRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let is_background = origin.endpoint == BACKGROUND_ENDPOINT;
    let visibility = if is_background {
        Visibility::Background
    } else {
        Visibility::User
    };

    let output_endpoint = resolve_output_endpoint(
        &mut rt.last_output_endpoint,
        &rt.output_topic_override_tx,
        &origin,
        is_background,
    );

    let _busy = rt.activity.main_turn();

    let opening_message = match &start {
        MainTurnStart::Message(message) => Some(*message),
        MainTurnStart::Rerun(_) => None,
    };
    publish_turn_started(
        rt,
        output_endpoint.as_ref(),
        reply_id,
        opening_message,
        TurnOrigin::new(&origin, visibility.clone()),
    )
    .await;

    let before = rt.agent.message_count();
    let (turn_start, opening_text, opening) = open_main_turn(start, before, &origin, rt);

    let ctx_strings = load_prompt_context_strings(&rt.skill_state, &rt.cfg).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let live_output = LiveOutputEndpoint::new(output_endpoint);
    let follow = is_background.then_some(OwnerFollow {
        output: &live_output,
        correlation_id: reply_id,
        last_output_endpoint: &mut rt.last_output_endpoint,
        output_topic_override_tx: &rt.output_topic_override_tx,
        idle_timeout: rt.cfg.idle.timeout,
        last_user_message_instant: &mut rt.last_user_message_instant,
        idle_deadline: &mut *idle_deadline,
    });
    let (turn_result, leftover_interrupts, subconscious_scratch, stop_requested) =
        run_agent_turn_with_interrupts(
            &mut rt.agent,
            &rt.agent_messenger,
            &rt.conversation_router,
            opening,
            &rt.publisher,
            TurnOutput::Live(&live_output),
            reply_id,
            &prompt_ctx,
            &mut rt.agent_subscriber,
            &mut rt.stop_rx,
            &mut rt.agent_stop_rx,
            &mut rt.retry_rx,
            (!is_background && rt.subconscious.mid_turn_enabled())
                .then(|| Arc::clone(&rt.subconscious)),
            &rt.queued,
            follow,
        )
        .await;
    let delivered_to = live_output.get();

    spawn_main_turn_end_checkpoint(rt, reply_id, &turn_result);

    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(
        rt,
        &new_messages,
        visibility.clone(),
        observe_deadline,
        Some(reply_id),
    )
    .await;

    let last_reply = last_reply_text(&turn_result);
    publish_turn_outcome(
        turn_result,
        &rt.publisher,
        delivered_to.as_ref(),
        reply_id,
        rt.tz,
        &TurnFailureReport {
            tracing_service: &rt.tracing_service,
            cfg: &rt.cfg,
            build: rt.build,
        },
    )
    .await;

    let user_message = (!is_background).then_some(opening_text);
    report_turn_end(&rt.activity, last_reply, user_message, visibility);

    if !is_background {
        evaluate_person_turn(rt, turn_start, reply_id, subconscious_scratch.as_ref()).await;
    }

    defer_late_messages(rt, leftover_interrupts, stop_requested).await;

    if !is_background {
        note_owner_active_now(rt, idle_deadline);
    }

    stop_requested
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_bus::Subscriber;
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_core::testing::wait;
    use residuum_core::util::telemetry::{SpanBufferConfig, SpanBufferLayer};

    use crate::testing::test_config;

    const TEST_TZ: chrono_tz::Tz = chrono_tz::Tz::UTC;

    fn endpoint() -> EndpointName {
        EndpointName::from("telegram")
    }

    fn test_tracing_service() -> TracingService {
        let (_, handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        TracingService::new(residuum_config::TracingConfig::default(), handle)
    }

    #[test]
    fn interrupt_channel_accepts_more_than_the_old_bounded_capacity() {
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

    fn web() -> EndpointName {
        EndpointName::from(residuum_config::WEB_UI_ENDPOINT)
    }

    fn registry_with_telegram() -> residuum_bus::EndpointRegistry {
        let interactive = |id: &str| residuum_bus::EndpointEntry {
            id: residuum_bus::EndpointId::from(id),
            topic: residuum_bus::TopicId::Endpoint(EndpointName::from(id)),
            capabilities: residuum_bus::EndpointCapabilities::INTERACTIVE,
            display_name: id.to_string(),
        };
        residuum_bus::EndpointRegistry::from_entries([
            interactive(residuum_config::WEB_UI_ENDPOINT),
            interactive("telegram"),
        ])
    }

    fn origin_on(endpoint: &str) -> MessageOrigin {
        MessageOrigin {
            endpoint: endpoint.to_string(),
            sender: None,
            conversation: None,
            agent_sender: None,
        }
    }

    fn agent_started() -> MessageOrigin {
        residuum_bus::MessageEvent::from_background(String::new()).origin
    }

    #[tokio::test]
    async fn after_idle_with_the_default_channel_an_agent_started_turn_reaches_only_the_web() {
        let (override_tx, _override_rx) = tokio::sync::watch::channel(None);
        let mut last = None;

        let owner = resolve_output_endpoint(&mut last, &override_tx, &origin_on("telegram"), false);
        assert_eq!(owner, Some(endpoint()), "the owner's turn replies in place");
        assert_eq!(
            resolve_output_endpoint(&mut last, &override_tx, &agent_started(), true),
            Some(endpoint()),
            "before the idle period an agent-started turn follows the owner"
        );

        let idle_channel = residuum_config::IdleConfig::default().idle_channel;
        last = Some(crate::idle::idle_output_endpoint(
            &registry_with_telegram(),
            &idle_channel,
        ));
        let after_idle = resolve_output_endpoint(&mut last, &override_tx, &agent_started(), true);
        assert_eq!(after_idle, Some(web()));

        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut chat_responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut chat_lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut chat_errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut main: Subscriber<MainConversationEvent> =
            handle.subscribe(topics::MainConversation).await.unwrap();

        publish_turn_started_to_chat_interface(&publisher, after_idle.as_ref(), "bg-1").await;
        publish_turn_outcome(
            Ok(vec!["the session finished".into()]),
            &publisher,
            after_idle.as_ref(),
            "bg-1",
            TEST_TZ,
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

        let ended = main.recv().await.unwrap().unwrap();
        assert!(
            matches!(&ended, MainConversationEvent::TurnEnded { turn_id } if turn_id == "bg-1"),
            "the web's main conversation carries the turn: {ended:?}"
        );
        assert_no_event(&handle, &mut chat_responses).await;
        assert_no_event(&handle, &mut chat_lifecycle).await;
        assert_no_event(&handle, &mut chat_errors).await;

        assert_eq!(
            resolve_output_endpoint(&mut last, &override_tx, &origin_on("telegram"), false),
            Some(endpoint()),
            "the owner's next message still replies in place"
        );
        assert_eq!(
            resolve_output_endpoint(&mut last, &override_tx, &agent_started(), true),
            Some(endpoint()),
            "and agent-started turns follow the owner again until the next idle period"
        );
    }

    #[tokio::test]
    async fn after_idle_an_explicit_chat_channel_receives_agent_started_turns() {
        let (override_tx, _override_rx) = tokio::sync::watch::channel(None);
        let mut last = None;
        resolve_output_endpoint(&mut last, &override_tx, &origin_on("ws"), false);

        last = Some(crate::idle::idle_output_endpoint(
            &registry_with_telegram(),
            "telegram",
        ));
        let after_idle = resolve_output_endpoint(&mut last, &override_tx, &agent_started(), true);
        assert_eq!(after_idle, Some(endpoint()));

        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut chat_responses: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        publish_turn_outcome(
            Ok(vec!["the session finished".into()]),
            &publisher,
            after_idle.as_ref(),
            "bg-2",
            TEST_TZ,
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

        let response = chat_responses.recv().await.unwrap().unwrap();
        assert_eq!(response.content, "the session finished");
        assert_eq!(response.correlation_id, "bg-2");
    }

    #[tokio::test]
    async fn an_owner_message_mid_turn_moves_routing_and_the_turn_at_once() {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let discord = EndpointName::from("discord");
        let mut old_chat: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(discord.clone()))
            .await
            .unwrap();
        let mut new_chat: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let live = LiveOutputEndpoint::new(Some(discord.clone()));
        let (override_tx, _override_rx) = tokio::sync::watch::channel(Some(discord.clone()));
        let mut last = Some(discord);
        let mut last_instant = None;
        let mut deadline = None;
        let timeout = std::time::Duration::from_mins(30);

        let before = tokio::time::Instant::now();
        OwnerFollow {
            output: &live,
            correlation_id: "bg-1",
            last_output_endpoint: &mut last,
            output_topic_override_tx: &override_tx,
            idle_timeout: timeout,
            last_user_message_instant: &mut last_instant,
            idle_deadline: &mut deadline,
        }
        .owner_wrote_from(endpoint(), &publisher)
        .await;
        let after = tokio::time::Instant::now();

        assert_eq!(
            live.get(),
            Some(endpoint()),
            "the rest of the turn goes to the owner"
        );
        assert_eq!(
            last,
            Some(endpoint()),
            "and so do later agent-started turns"
        );
        assert_eq!(
            *override_tx.borrow(),
            None,
            "the switch_endpoint override is cleared"
        );
        let at = last_instant.expect("the owner counts as active");
        assert!(
            before <= at && at <= after,
            "from the moment the message arrived"
        );
        assert_eq!(
            deadline,
            Some(at + timeout),
            "the idle timer restarts from then"
        );
        let ended = old_chat.recv().await.unwrap().unwrap();
        assert!(
            matches!(&ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "bg-1"),
            "the chat the turn was on stops showing it: {ended:?}"
        );
        let started = new_chat.recv().await.unwrap().unwrap();
        assert!(
            matches!(&started, TurnLifecycleEvent::Started { correlation_id } if correlation_id == "bg-1"),
            "the owner's chat shows it: {started:?}"
        );
    }

    struct GatedThreeCallProvider {
        started: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Notify>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for GatedThreeCallProvider {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            let tool_call = |id: &str| residuum_inference::ToolCall {
                id: id.to_string(),
                name: "look_around".to_string(),
                arguments: serde_json::json!({}),
                server: None,
            };
            match self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => {
                    self.started.notify_one();
                    self.gate.notified().await;
                    Ok(residuum_inference::InferenceResponse::new(
                        String::new(),
                        vec![tool_call("call-1")],
                    ))
                }
                1 => Ok(residuum_inference::InferenceResponse::new(
                    "checking once more".to_string(),
                    vec![tool_call("call-2")],
                )),
                _ => Ok(residuum_inference::InferenceResponse::new(
                    "all done".to_string(),
                    vec![],
                )),
            }
        }

        fn model_name(&self) -> &'static str {
            "gated-three-call"
        }
    }

    struct AgentStartedTurnEnd {
        leftovers: Vec<Interrupt>,
        last_output_endpoint: Option<EndpointName>,
        last_user_message_instant: Option<tokio::time::Instant>,
        idle_deadline: Option<tokio::time::Instant>,
        rig: TurnRig,
    }

    async fn agent_started_turn_after_idle(
        mut rig: TurnRig,
        idle_timeout: std::time::Duration,
    ) -> AgentStartedTurnEnd {
        let (override_tx, _override_rx) = tokio::sync::watch::channel(None);
        let mut last = Some(crate::idle::idle_output_endpoint(
            &registry_with_telegram(),
            &residuum_config::IdleConfig::default().idle_channel,
        ));
        let origin = agent_started();
        let initial = resolve_output_endpoint(&mut last, &override_tx, &origin, true);
        assert_eq!(
            initial,
            Some(web()),
            "after idle the turn starts on the web"
        );
        let live = LiveOutputEndpoint::new(initial);
        let mut last_instant = None;
        let mut deadline = None;
        let prompt_ctx = PromptContext::default();
        let (result, leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
            &mut rig.agent,
            &rig.messenger,
            &rig.router,
            TurnOpening::Message {
                content: "a session finished",
                origin: Some(&origin),
                images: &[],
            },
            &rig.publisher,
            TurnOutput::Live(&live),
            "bg-follow",
            &prompt_ctx,
            &mut rig.subscriber,
            &mut rig.stop_rx,
            &mut rig.agent_stop_rx,
            &mut rig.retry_rx,
            None,
            &rig.queued,
            Some(OwnerFollow {
                output: &live,
                correlation_id: "bg-follow",
                last_output_endpoint: &mut last,
                output_topic_override_tx: &override_tx,
                idle_timeout,
                last_user_message_instant: &mut last_instant,
                idle_deadline: &mut deadline,
            }),
        )
        .await;
        publish_turn_outcome(
            result,
            &rig.publisher,
            live.get().as_ref(),
            "bg-follow",
            TEST_TZ,
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;
        AgentStartedTurnEnd {
            leftovers,
            last_output_endpoint: last,
            last_user_message_instant: last_instant,
            idle_deadline: deadline,
            rig,
        }
    }

    #[tokio::test]
    async fn an_owner_chat_message_during_an_agent_started_turn_after_idle_takes_the_turn_there() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let rig = TurnRig::new(GatedThreeCallProvider {
            started: Arc::clone(&started),
            gate: Arc::clone(&gate),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
        .await;
        let publisher = rig.publisher.clone();
        let mut chat_lifecycle: Subscriber<TurnLifecycleEvent> = rig
            .bus()
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut chat_intermediates: Subscriber<residuum_bus::IntermediateEvent> = rig
            .bus()
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let mut chat_responses: Subscriber<ResponseEvent> = rig
            .bus()
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();
        let timeout = std::time::Duration::from_mins(30);

        let turn = residuum_core::util::spawn_in_span(agent_started_turn_after_idle(rig, timeout));

        wait::guarded("the first model call to start", started.notified()).await;
        let mut owner_message = person_message("tg-1", "what did it find?");
        owner_message.origin = origin_on("telegram");
        publisher
            .publish(topics::UserMessage, owner_message)
            .await
            .unwrap();
        let typing = wait::guarded("Telegram to show the turn", chat_lifecycle.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&typing, TurnLifecycleEvent::Started { correlation_id } if correlation_id == "bg-follow"),
            "the owner's chat shows the turn as soon as the message arrives: {typing:?}"
        );
        let arrived_by = tokio::time::Instant::now();
        gate.notify_one();

        let ended_turn = wait::guarded("the turn to end", turn).await.unwrap();
        assert!(
            ended_turn.leftovers.is_empty(),
            "the message joined the turn"
        );
        assert_eq!(
            history_mentions(&ended_turn.rig.agent, "what did it find?"),
            1
        );

        let intermediate = chat_intermediates.recv().await.unwrap().unwrap();
        assert_eq!(intermediate.content, "checking once more");
        let reply = chat_responses.recv().await.unwrap().unwrap();
        assert_eq!(reply.content, "all done");
        assert_eq!(reply.correlation_id, "bg-follow");
        let ended = chat_lifecycle.recv().await.unwrap().unwrap();
        assert!(
            matches!(&ended, TurnLifecycleEvent::Ended { correlation_id } if correlation_id == "bg-follow"),
            "Telegram hears the turn end: {ended:?}"
        );

        assert_eq!(
            ended_turn.last_output_endpoint,
            Some(endpoint()),
            "later agent-started turns go to Telegram"
        );
        let at = ended_turn
            .last_user_message_instant
            .expect("the owner counts as active");
        assert!(
            at <= arrived_by,
            "from when the message arrived, not the turn's end"
        );
        assert_eq!(ended_turn.idle_deadline, Some(at + timeout));
    }

    #[tokio::test]
    async fn agent_and_background_messages_during_an_agent_started_turn_leave_routing_alone() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let rig = TurnRig::new(GatedThreeCallProvider {
            started: Arc::clone(&started),
            gate: Arc::clone(&gate),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
        .await;
        let publisher = rig.publisher.clone();
        let messenger = Arc::clone(&rig.messenger);
        let hop_counter = rig.agent.hop_counter().clone();
        let bus = rig.bus().clone();
        let mut chat_lifecycle: Subscriber<TurnLifecycleEvent> =
            bus.subscribe(topics::Endpoint(endpoint())).await.unwrap();
        let mut chat_intermediates: Subscriber<residuum_bus::IntermediateEvent> =
            bus.subscribe(topics::Endpoint(endpoint())).await.unwrap();
        let mut chat_responses: Subscriber<ResponseEvent> =
            bus.subscribe(topics::Endpoint(endpoint())).await.unwrap();

        let turn = residuum_core::util::spawn_in_span(agent_started_turn_after_idle(
            rig,
            std::time::Duration::from_mins(30),
        ));

        wait::guarded("the first model call to start", started.notified()).await;
        publisher
            .publish(
                topics::UserMessage,
                MessageEvent::from_background("a session's relayed result".to_string()),
            )
            .await
            .unwrap();
        messenger
            .send(
                "main",
                residuum_bus::AgentMessageEvent::new(
                    residuum_bus::SessionAddress::from("agent-beta"),
                    "teammate",
                    "a teammate's note",
                    5,
                ),
            )
            .await
            .unwrap();
        wait::until_true("the turn to take both messages in", || {
            hop_counter.get() >= 5
        })
        .await;
        gate.notify_one();

        let ended_turn = wait::guarded("the turn to end", turn).await.unwrap();
        assert_eq!(
            history_mentions(&ended_turn.rig.agent, "a session's relayed result"),
            1
        );
        assert_eq!(
            history_mentions(&ended_turn.rig.agent, "a teammate's note"),
            1
        );
        assert_eq!(
            ended_turn.last_output_endpoint,
            Some(web()),
            "agent-started turns still go to the web"
        );
        assert_eq!(
            ended_turn.last_user_message_instant, None,
            "no owner activity"
        );
        assert_eq!(
            ended_turn.idle_deadline, None,
            "the idle timer is untouched"
        );
        assert_no_event(&bus, &mut chat_lifecycle).await;
        assert_no_event(&bus, &mut chat_intermediates).await;
        assert_no_event(&bus, &mut chat_responses).await;
    }

    async fn assert_no_event<E: Clone + Send + Sync + 'static>(
        bus: &residuum_bus::BusHandle,
        sub: &mut Subscriber<E>,
    ) {
        bus_barrier(bus).await;
        assert!(
            sub.drain().is_empty(),
            "expected no event, but one was published"
        );
    }

    #[tokio::test]
    async fn ok_publishes_one_response_per_text_then_ends_turn() {
        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
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
        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
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
        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

        assert_no_event(&handle, &mut responses).await;
        assert_no_event(&handle, &mut lifecycle).await;
    }

    #[tokio::test]
    async fn a_turn_start_goes_to_a_chat_interface_but_not_to_the_web_ui() {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let ws = EndpointName::from(residuum_config::WEB_UI_ENDPOINT);
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

    #[tokio::test]
    async fn a_turn_delivered_to_the_web_ui_publishes_nothing_to_its_endpoint_topic() {
        let ws = EndpointName::from(residuum_config::WEB_UI_ENDPOINT);
        for outcome in [Ok(vec!["reply".to_string()]), Err(anyhow::anyhow!("boom"))] {
            let failed = outcome.is_err();
            let handle = residuum_bus::spawn_broker();
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
                &TurnFailureReport {
                    tracing_service: &test_tracing_service(),
                    cfg: &test_config(),
                    build: residuum_core::testing::HARNESS_BUILD,
                },
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

    async fn main_conversation_end_of(
        outcome: anyhow::Result<Vec<String>>,
        output_endpoint: Option<&EndpointName>,
    ) -> Option<String> {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut main: Subscriber<MainConversationEvent> =
            handle.subscribe(topics::MainConversation).await.unwrap();

        publish_turn_outcome(
            outcome,
            &publisher,
            output_endpoint,
            "corr-5",
            TEST_TZ,
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

        bus_barrier(&handle).await;
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
        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
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
        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &test_tracing_service(),
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
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
            residuum_config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..residuum_config::TracingConfig::default()
            },
            span_handle,
        );

        let handle = residuum_bus::spawn_broker();
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
            &TurnFailureReport {
                tracing_service: &tracing_service,
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

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
            residuum_config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..residuum_config::TracingConfig::default()
            },
            span_handle,
        );

        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(endpoint()))
            .await
            .unwrap();

        publish_turn_outcome(
            Ok(vec!["done".into()]),
            &publisher,
            Some(&endpoint()),
            "corr-7",
            TEST_TZ,
            &TurnFailureReport {
                tracing_service: &tracing_service,
                cfg: &test_config(),
                build: residuum_core::testing::HARNESS_BUILD,
            },
        )
        .await;

        lifecycle.recv().await.unwrap().unwrap();
    }

    struct GatedProvider {
        gate: Arc<tokio::sync::Notify>,
        response: String,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for GatedProvider {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            self.gate.notified().await;
            Ok(residuum_inference::InferenceResponse::new(
                self.response.clone(),
                vec![],
            ))
        }

        fn model_name(&self) -> &'static str {
            "gated"
        }
    }

    struct AnnouncingGatedProvider {
        started: Arc<tokio::sync::Notify>,
        inner: GatedProvider,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for AnnouncingGatedProvider {
        async fn complete(
            &self,
            messages: &[residuum_inference::Message],
            tools: &[residuum_inference::ToolDefinition],
            options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            self.started.notify_one();
            self.inner.complete(messages, tools, options).await
        }

        fn model_name(&self) -> &'static str {
            "announcing-gated"
        }
    }

    fn test_agent(provider: impl residuum_inference::InferenceProvider + 'static) -> Agent {
        Agent::new(
            Box::new(provider),
            residuum_agent::ToolRegistry::new(),
            residuum_mcp::McpRegistry::new_shared(),
            residuum_workspace::identity::IdentityFiles::default(),
            residuum_agent::AgentConfig {
                options: residuum_inference::CompletionOptions::default(),
                tz: TEST_TZ,
                layout: None,
            },
            residuum_agent::HopCounter::new(0),
        )
    }

    #[tokio::test]
    async fn main_hop_counter_starts_at_kickoff_and_bumps_on_a_mid_turn_agent_message() {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(residuum_background::messaging::AgentMessenger::new(
            Arc::new(residuum_background::registry::SessionRegistry::new()),
            publisher.clone(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(residuum_background::ConversationRouter::new(
            Arc::clone(&messenger),
        ));

        let gate = Arc::new(tokio::sync::Notify::new());
        let mut agent = test_agent(GatedProvider {
            gate: Arc::clone(&gate),
            response: "done".to_string(),
        });
        agent.hop_counter().set(2);
        let hop_counter = agent.hop_counter().clone();

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let (_agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);
        let (_retry_tx, mut retry_rx) = mpsc::channel::<RetryTurnRequest>(1);

        let prompt_ctx = PromptContext::default();

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = residuum_core::util::spawn_in_span(async move {
            let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
                TurnOpening::Message {
                    content: "hello",
                    origin: None,
                    images: &[],
                },
                &publisher,
                TurnOutput::Fixed(None),
                "corr-hop",
                &prompt_ctx,
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
                &mut retry_rx,
                None,
                &QueuedMessages::default(),
                None,
            )
            .await;
            turn_result.expect("gated turn should complete successfully");
            agent
        });

        messenger
            .send(
                "main",
                residuum_bus::AgentMessageEvent::new(
                    residuum_bus::SessionAddress::from("spawned-researcher-0001"),
                    "spawned",
                    "still working",
                    7,
                ),
            )
            .await
            .unwrap();

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
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(residuum_background::messaging::AgentMessenger::new(
            Arc::new(residuum_background::registry::SessionRegistry::new()),
            publisher.clone(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(residuum_background::ConversationRouter::new(
            Arc::clone(&messenger),
        ));

        let gate = Arc::new(tokio::sync::Notify::new());
        gate.notify_one();
        let mut agent = test_agent(GatedProvider {
            gate,
            response: "done".to_string(),
        });

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(4);
        let (_agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);
        let (_retry_tx, mut retry_rx) = mpsc::channel::<RetryTurnRequest>(1);

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        stop_tx
            .send(StopRequest {
                reply_to: None,
                result_tx: Some(result_tx),
            })
            .await
            .unwrap();

        let prompt_ctx = PromptContext::default();

        let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
            &mut agent,
            &messenger,
            &conversation_router,
            TurnOpening::Message {
                content: "hello",
                origin: None,
                images: &[],
            },
            &publisher,
            TurnOutput::Fixed(None),
            "corr-stale-stop",
            &prompt_ctx,
            &mut agent_subscriber,
            &mut stop_rx,
            &mut agent_stop_rx,
            &mut retry_rx,
            None,
            &QueuedMessages::default(),
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

    fn group_chat_message() -> residuum_bus::MessageEvent {
        residuum_bus::MessageEvent {
            id: "group-msg-1".to_string(),
            content: "anyone around?".to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: None,
                conversation: Some(residuum_bus::origin::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: residuum_bus::origin::ConversationKind::GroupChat,
                    is_owner: true,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    #[tokio::test]
    async fn mid_turn_message_not_belonging_to_main_is_routed_away_not_injected() {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let registry = Arc::new(residuum_background::registry::SessionRegistry::new());
        let messenger = Arc::new(residuum_background::messaging::AgentMessenger::new(
            Arc::clone(&registry),
            publisher.clone(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(residuum_background::ConversationRouter::new(
            Arc::clone(&messenger),
        ));
        let mut spawns: Subscriber<residuum_bus::SpawnRequestEvent> =
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
        let (_retry_tx, mut retry_rx) = mpsc::channel::<RetryTurnRequest>(1);

        let prompt_ctx = PromptContext::default();

        let turn_messenger = Arc::clone(&messenger);
        let turn_conversation_router = Arc::clone(&conversation_router);
        let turn_task = residuum_core::util::spawn_in_span(async move {
            let (turn_result, _leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut agent,
                &turn_messenger,
                &turn_conversation_router,
                TurnOpening::Message {
                    content: "hello",
                    origin: None,
                    images: &[],
                },
                &publisher,
                TurnOutput::Fixed(None),
                "corr-route",
                &prompt_ctx,
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
                &mut retry_rx,
                None,
                &QueuedMessages::default(),
                None,
            )
            .await;
            turn_result.expect("gated turn should complete successfully");
            agent
        });

        handle
            .publisher()
            .publish(residuum_bus::topics::UserMessage, group_chat_message())
            .await
            .unwrap();

        let address =
            residuum_background::registry::conversation_session_address("discord", "chan-1");
        let spawn_event = next_event(
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

    fn hop_test_agent() -> Agent {
        test_agent(GatedProvider {
            gate: Arc::new(tokio::sync::Notify::new()),
            response: "unused".to_string(),
        })
    }

    fn sample_inbound(content: &str) -> residuum_bus::MessageEvent {
        residuum_bus::MessageEvent {
            id: "leftover-1".to_string(),
            content: content.to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    fn test_messenger() -> Arc<residuum_background::messaging::AgentMessenger> {
        let handle = residuum_bus::spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        Arc::new(residuum_background::messaging::AgentMessenger::new(
            Arc::new(residuum_background::registry::SessionRegistry::new()),
            handle.publisher(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
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
            vec![Interrupt::AgentMessage(
                residuum_bus::AgentMessageEvent::new(
                    residuum_bus::SessionAddress::from("spawned-researcher-0001"),
                    "spawned",
                    "still looping",
                    12,
                ),
            )],
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
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
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
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
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

        let persisted =
            residuum_memory::recent_messages::load_recent_messages(&layout.recent_messages_json())
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
        let mut agent = hop_test_agent();
        let messenger = test_messenger();
        let dir = tempfile::tempdir().unwrap();
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();

        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        let mut on_bus = sample_inbound("still on the bus, unread");
        on_bus.id = "bus-1".to_string();
        publisher
            .publish(topics::UserMessage, on_bus)
            .await
            .unwrap();
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
            residuum_memory::recent_messages::load_recent_messages(&layout.recent_messages_json())
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

    struct TurnRig {
        agent: Agent,
        subscriber: Subscriber<MessageEvent>,
        stop_rx: mpsc::Receiver<StopRequest>,
        agent_stop_rx: mpsc::Receiver<()>,
        retry_rx: mpsc::Receiver<RetryTurnRequest>,
        publisher: Publisher,
        messenger: Arc<residuum_background::messaging::AgentMessenger>,
        router: Arc<residuum_background::ConversationRouter>,
        queued: Arc<QueuedMessages>,
        keep_alive: (
            residuum_bus::BusHandle,
            mpsc::Sender<StopRequest>,
            mpsc::Sender<()>,
            mpsc::Sender<RetryTurnRequest>,
        ),
    }

    impl TurnRig {
        fn bus(&self) -> &residuum_bus::BusHandle {
            &self.keep_alive.0
        }

        async fn new(provider: impl residuum_inference::InferenceProvider + 'static) -> Self {
            let handle = residuum_bus::spawn_broker();
            let publisher = handle.publisher();
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(residuum_background::store::SessionStore::new(
                dir.path().to_path_buf(),
            ));
            let messenger = Arc::new(residuum_background::messaging::AgentMessenger::new(
                Arc::new(residuum_background::registry::SessionRegistry::new()),
                publisher.clone(),
                store,
                residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
            ));
            let router = Arc::new(residuum_background::ConversationRouter::new(Arc::clone(
                &messenger,
            )));
            let subscriber = handle.subscribe(topics::UserMessage).await.unwrap();
            let (stop_tx, stop_rx) = mpsc::channel::<StopRequest>(1);
            let (agent_stop_tx, agent_stop_rx) = mpsc::channel::<()>(1);
            let (retry_tx, retry_rx) = mpsc::channel::<RetryTurnRequest>(1);
            Self {
                agent: test_agent(provider),
                subscriber,
                stop_rx,
                agent_stop_rx,
                retry_rx,
                publisher,
                messenger,
                router,
                queued: Arc::default(),
                keep_alive: (handle, stop_tx, agent_stop_tx, retry_tx),
            }
        }

        fn stop_tx(&self) -> mpsc::Sender<StopRequest> {
            self.keep_alive.1.clone()
        }

        fn retry_tx(&self) -> mpsc::Sender<RetryTurnRequest> {
            self.keep_alive.3.clone()
        }

        async fn run_turn(
            &mut self,
            content: &str,
            correlation_id: &str,
            origin: Option<&MessageOrigin>,
        ) -> Vec<Interrupt> {
            let (result, leftovers) = self
                .run(
                    TurnOpening::Message {
                        content,
                        origin,
                        images: &[],
                    },
                    correlation_id,
                )
                .await;
            result.expect("the turn completes");
            leftovers
        }

        async fn run(
            &mut self,
            opening: TurnOpening<'_>,
            correlation_id: &str,
        ) -> (anyhow::Result<Vec<String>>, Vec<Interrupt>) {
            let prompt_ctx = PromptContext::default();
            let (result, leftovers, _scratch, _stopped) = run_agent_turn_with_interrupts(
                &mut self.agent,
                &self.messenger,
                &self.router,
                opening,
                &self.publisher,
                TurnOutput::Fixed(None),
                correlation_id,
                &prompt_ctx,
                &mut self.subscriber,
                &mut self.stop_rx,
                &mut self.agent_stop_rx,
                &mut self.retry_rx,
                None,
                &self.queued,
                None,
            )
            .await;
            (result, leftovers)
        }
    }

    struct ToolThenTextProvider {
        started: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Notify>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for ToolThenTextProvider {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            self.started.notify_one();
            self.gate.notified().await;
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(residuum_inference::InferenceResponse::new(
                    String::new(),
                    vec![residuum_inference::ToolCall {
                        id: "call-1".to_string(),
                        name: "look_around".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
                    }],
                ))
            } else {
                Ok(residuum_inference::InferenceResponse::new(
                    "done".to_string(),
                    vec![],
                ))
            }
        }

        fn model_name(&self) -> &'static str {
            "tool-then-text"
        }
    }

    fn person_message(id: &str, content: &str) -> MessageEvent {
        MessageEvent {
            id: id.to_string(),
            content: content.to_string(),
            origin: MessageOrigin {
                endpoint: "ws".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    async fn while_a_message_waits(
        act: impl FnOnce(&QueuedMessages, &str),
    ) -> (Vec<Interrupt>, Vec<String>, TurnRig) {
        let gate = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let mut rig = TurnRig::new(ToolThenTextProvider {
            started: Arc::clone(&started),
            gate: Arc::clone(&gate),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
        .await;
        let queued = Arc::clone(&rig.queued);
        let publisher = rig.publisher.clone();
        let mut main: Subscriber<MainConversationEvent> =
            rig.bus().subscribe(topics::MainConversation).await.unwrap();

        let turn = residuum_core::util::spawn_in_span(async move {
            let leftovers = rig.run_turn("kickoff", "corr-wait", None).await;
            (leftovers, rig)
        });
        wait::guarded("the first model call to start", started.notified()).await;
        publisher
            .publish(
                topics::UserMessage,
                person_message("m1", "also check the logs"),
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        loop {
            let event = wait::guarded("the message to be announced as waiting", main.recv())
                .await
                .unwrap()
                .unwrap();
            let waiting =
                matches!(&event, MainConversationEvent::MessageQueued { id, .. } if id == "m1");
            events.push(event);
            if waiting {
                break;
            }
        }
        act(&queued, "m1");
        gate.notify_one();
        gate.notify_one();
        let (leftovers, ended) = wait::guarded("the turn to end", turn).await.unwrap();
        bus_barrier(ended.bus()).await;
        events.extend(main.drain());
        let named = events
            .iter()
            .filter_map(|event| {
                if let MainConversationEvent::MessageQueued { id, .. } = event {
                    Some(format!("queued:{id}"))
                } else if let MainConversationEvent::UserMessage { id, .. } = event {
                    Some(format!("read:{id}"))
                } else {
                    None
                }
            })
            .collect();
        (leftovers, named, ended)
    }

    #[tokio::test]
    async fn a_message_sent_mid_turn_is_announced_waiting_then_read_at_the_checkpoint() {
        let (leftovers, named, rig) = while_a_message_waits(|_, _| {}).await;
        assert_eq!(named, ["queued:m1", "read:m1"]);
        assert!(leftovers.is_empty());
        assert_eq!(history_mentions(&rig.agent, "also check the logs"), 1);
        assert!(!rig.queued.withdraw("m1"), "a read message no longer waits");
    }

    #[tokio::test]
    async fn a_withdrawn_message_never_reaches_the_turn() {
        let (leftovers, named, rig) = while_a_message_waits(|queued, id| {
            assert!(queued.withdraw(id));
        })
        .await;
        assert_eq!(named, ["queued:m1"], "it is never announced as read");
        assert!(leftovers.is_empty(), "and starts no turn of its own");
        assert_eq!(history_mentions(&rig.agent, "also check the logs"), 0);
    }

    #[tokio::test]
    async fn a_message_to_send_now_is_kept_out_of_the_turn_and_left_to_start_the_next() {
        let (leftovers, named, rig) = while_a_message_waits(|queued, id| {
            assert!(queued.send_now(id));
        })
        .await;
        assert_eq!(named, ["queued:m1"]);
        assert_eq!(history_mentions(&rig.agent, "also check the logs"), 0);
        let [Interrupt::UserMessage(message)] = leftovers.as_slice() else {
            panic!("expected the message back to start the next turn");
        };
        assert_eq!(message.id, "m1");
        assert!(rig.queued.is_send_now("m1"), "it still waits, to run first");
    }

    #[tokio::test]
    async fn a_message_arriving_during_the_final_model_call_starts_a_turn_and_is_recorded_once() {
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

        let first_turn = residuum_core::util::spawn_in_span(async move {
            let leftovers = first_rig.run_turn("kickoff", "corr-late-1", None).await;
            let late =
                process_leftover_interrupts(leftovers, &mut first_rig.agent, &first_rig.messenger);
            (first_rig, late)
        });

        wait::guarded("the first turn to reach its model call", started.notified()).await;
        messenger
            .send(
                "main",
                residuum_bus::AgentMessageEvent::new(
                    residuum_bus::SessionAddress::from("agent-beta"),
                    "teammate",
                    "please review",
                    5,
                ),
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
        bus_barrier(rig.bus()).await;
        assert!(
            rig.subscriber.drain().is_empty(),
            "requeueing is local, not a second broadcast on the bus"
        );

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
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().to_path_buf(),
        ));
        let messenger = Arc::new(residuum_background::messaging::AgentMessenger::new(
            Arc::new(residuum_background::registry::SessionRegistry::new()),
            publisher.clone(),
            store,
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let conversation_router = Arc::new(residuum_background::ConversationRouter::new(
            Arc::clone(&messenger),
        ));

        let gate = Arc::new(tokio::sync::Notify::new());
        let mut agent = test_agent(GatedProvider {
            gate: Arc::clone(&gate),
            response: "unused".to_string(),
        });

        let mut agent_subscriber: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let (_stop_tx, mut stop_rx) = mpsc::channel::<StopRequest>(1);
        let (agent_stop_tx, mut agent_stop_rx) = mpsc::channel::<()>(1);
        let (_retry_tx, mut retry_rx) = mpsc::channel::<RetryTurnRequest>(1);

        let prompt_ctx = PromptContext::default();

        let turn_task = residuum_core::util::spawn_in_span(async move {
            run_agent_turn_with_interrupts(
                &mut agent,
                &messenger,
                &conversation_router,
                TurnOpening::Message {
                    content: "hello",
                    origin: None,
                    images: &[],
                },
                &publisher,
                TurnOutput::Fixed(None),
                "corr-shutdown",
                &prompt_ctx,
                &mut agent_subscriber,
                &mut stop_rx,
                &mut agent_stop_rx,
                &mut retry_rx,
                None,
                &QueuedMessages::default(),
                None,
            )
            .await
        });

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
        requests: Arc<std::sync::Mutex<Vec<Vec<residuum_inference::Role>>>>,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for FailsOnce {
        async fn complete(
            &self,
            messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            self.requests
                .lock()
                .unwrap()
                .push(messages.iter().map(|m| m.role).collect());
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(residuum_inference::InferenceError::Api(
                    "the model is down".to_string(),
                ))
            } else {
                Ok(residuum_inference::InferenceResponse::new(
                    "back".to_string(),
                    vec![],
                ))
            }
        }

        fn model_name(&self) -> &'static str {
            "fails-once"
        }
    }

    #[tokio::test]
    async fn a_failed_turn_keeps_the_users_message_and_records_nothing_else() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = test_agent(FailsOnce {
            calls: std::sync::atomic::AtomicUsize::new(0),
            requests: Arc::clone(&requests),
        });
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let ep = endpoint();
        let prompt_ctx = PromptContext::default();
        let dir = tempfile::tempdir().unwrap();
        let recent_path = dir.path().join("recent_messages.json");

        let before = agent.message_count();
        let mut interrupts = residuum_bus::testing::dead_interrupt_rx();
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
        residuum_memory::recent_messages::append_recent_messages(
            &recent_path,
            &new_messages,
            Visibility::User,
            TEST_TZ,
            Some("corr-failed"),
        )
        .await
        .unwrap();
        let saved = residuum_memory::recent_messages::load_recent_messages(&recent_path)
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
                residuum_inference::Role::User,
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
            .filter(|role| *role != residuum_inference::Role::System)
            .collect();
        assert_eq!(
            conversation,
            [
                residuum_inference::Role::User,
                residuum_inference::Role::User
            ],
            "the next turn sends the model the unanswered message and the new one"
        );
    }

    struct FailsThenHangs {
        calls: std::sync::atomic::AtomicUsize,
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for FailsThenHangs {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err(residuum_inference::InferenceError::Api(
                    "the model is down".to_string(),
                ));
            }
            self.started.notify_one();
            std::future::pending().await
        }

        fn model_name(&self) -> &'static str {
            "fails-then-hangs"
        }
    }

    #[tokio::test]
    async fn a_retry_sent_while_a_turn_runs_is_refused_at_once() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let mut rig = TurnRig::new(GatedProvider {
            gate: Arc::clone(&gate),
            response: "done".to_string(),
        })
        .await;
        let retry_tx = rig.retry_tx();
        let turn = residuum_core::util::spawn_in_span(async move {
            rig.run_turn("busy", "t-running", None).await;
            rig
        });

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        retry_tx
            .send(RetryTurnRequest {
                id: "t-failed".to_string(),
                result_tx,
            })
            .await
            .unwrap();
        let answer = wait::guarded("the running turn to answer the retry", result_rx)
            .await
            .unwrap();
        assert_eq!(answer, Err(RETRY_REFUSED_TURN_RUNNING.to_string()));

        gate.notify_one();
        wait::guarded("the turn to end once the gate opens", turn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_rerun_keeps_the_turn_id_and_is_stopped_by_it() {
        let started = Arc::new(tokio::sync::Notify::new());
        let mut rig = TurnRig::new(FailsThenHangs {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: Arc::clone(&started),
        })
        .await;
        let origin = origin_on("ws");
        let (first_attempt, _) = rig
            .run(
                TurnOpening::Message {
                    content: "hello",
                    origin: Some(&origin),
                    images: &[],
                },
                "t1",
            )
            .await;
        assert!(first_attempt.is_err(), "the first model call failed");
        let failed = rig.agent.take_failed_turn("t1").expect("recorded");
        let stop_tx = rig.stop_tx();
        let rerun = residuum_core::util::spawn_in_span(async move {
            let (result, _) = rig.run(TurnOpening::Rerun(failed), "t1").await;
            (rig, result)
        });

        wait::guarded("the rerun to reach its model call", started.notified()).await;
        let (matched_tx, matched_rx) = tokio::sync::oneshot::channel();
        stop_tx
            .send(StopRequest {
                reply_to: Some("t1".to_string()),
                result_tx: Some(matched_tx),
            })
            .await
            .unwrap();
        assert!(
            wait::guarded("the stop to be answered", matched_rx)
                .await
                .unwrap(),
            "a stop naming the failed turn's id reaches its rerun"
        );
        let (rig_after, result) = wait::guarded("the stopped rerun to end", rerun)
            .await
            .unwrap();
        assert!(
            result.expect("a stopped rerun is not a failure").is_empty(),
            "the rerun was stopped before it replied"
        );
        assert!(
            rig_after.agent.failed_turn().is_none(),
            "a stopped rerun did not fail, so there is nothing to retry"
        );
        assert_eq!(
            history_mentions(&rig_after.agent, "hello"),
            1,
            "the rerun added no message"
        );
    }

    #[tokio::test]
    async fn a_rerun_discards_the_error_notes_queued_before_it() {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let mut errors: Subscriber<ErrorEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        publisher
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                ErrorEvent {
                    correlation_id: "t1".to_string(),
                    message: "the model call failed".to_string(),
                    details: None,
                },
            )
            .await
            .unwrap();
        bus_barrier(&handle).await;

        assert_eq!(
            discard_queued_error_notes(&mut errors),
            1,
            "the failed attempt's error note is dropped before the rerun"
        );
        assert_eq!(
            discard_queued_error_notes(&mut errors),
            0,
            "nothing is left to drop once the queue is empty"
        );
    }
}

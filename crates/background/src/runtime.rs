use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::FutureExt;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use residuum_agent::hop::HopCounter;
use residuum_agent::recent_messages::RecentMessages;
use residuum_bus::MessageEvent;
use residuum_bus::interrupt::Interrupt;
use residuum_bus::origin::A2A_ENDPOINT;
use residuum_bus::{
    AgentMessageEvent, AgentResultEvent, AgentResultStatus, ConversationTarget,
    ConversationTypingEvent, EndpointName, EventTrigger, HEARTBEAT_OK, HEARTBEAT_URGENT, Publisher,
    PulseOverlap, ResultDisposition, SessionAddress, SessionEventKind, SessionResponseEvent,
    SkillName, ends_with_sentinel, topics,
};
use residuum_config::BackgroundConfig;
use residuum_tracing_service::{ClientContext, TracingService};

use super::merge_tasks::MergeTasks;
use super::messaging::{AgentMessenger, DeliveryOutcome, PendingInput};
use super::session_memory::{
    CompletionMerge, MergeHandles, SessionMemory, SessionMemoryEnv, TimedMessage,
};
use super::store::{RunTranscriptSink, SessionStore, TranscriptLine};
use super::subagent::{SessionTurnIdentity, SubAgentResources, TurnKickoff, execute_subagent};
use super::types::{SubAgentConfig, truncate_prompt_preview};
use crate::registry::{DeliverOutcome, ResumePoint, SessionRegistry};
use residuum_bus::session::publish_session_event;
use residuum_bus::session::{SessionCategory, SessionInfo, SessionState, generate_run_id};

#[derive(Debug, Clone, Copy)]
pub struct IdleTimeouts {
    pub(crate) scheduled: Duration,
    pub(crate) spawned: Duration,
    pub(crate) external: Duration,
    pub(crate) artifact: Duration,
}

impl IdleTimeouts {
    fn for_trigger(&self, trigger: &EventTrigger) -> Duration {
        match SessionCategory::from_trigger(trigger) {
            SessionCategory::Scheduled => self.scheduled,
            SessionCategory::Spawned => self.spawned,
            SessionCategory::Artifact => self.artifact,
            SessionCategory::External => {
                if matches!(trigger, EventTrigger::Webhook(_)) {
                    self.scheduled
                } else {
                    self.external
                }
            }
        }
    }
}

impl From<&BackgroundConfig> for IdleTimeouts {
    fn from(cfg: &BackgroundConfig) -> Self {
        Self {
            scheduled: cfg.idle_timeout_scheduled,
            spawned: cfg.idle_timeout_spawned,
            external: cfg.idle_timeout_external,
            artifact: cfg.idle_timeout_artifact,
        }
    }
}

pub struct SessionSpawnRequest {
    pub address: SessionAddress,
    pub source_label: String,
    pub trigger: EventTrigger,
    pub agent_skill: Option<SkillName>,
    pub spawner: Option<SessionAddress>,
    pub depth: u32,
    pub subagent_config: SubAgentConfig,
    pub conversation_target: Option<ConversationTarget>,
    pub overlap: Option<PulseOverlap>,
    pub stop: Option<CancellationToken>,
}

pub struct SessionRuntimeHandles {
    pub publisher: Publisher,
    pub tz: chrono_tz::Tz,
    pub messenger: Arc<AgentMessenger>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub tracing_service: Arc<TracingService>,
    pub tracing_client_context: Arc<ClientContext>,
}

pub struct SessionRuntime {
    semaphore: Arc<Semaphore>,
    registry: Arc<SessionRegistry>,
    store: Arc<SessionStore>,
    idle_timeouts: IdleTimeouts,
    publisher: Publisher,
    tz: chrono_tz::Tz,
    messenger: Arc<AgentMessenger>,
    checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    tracing_service: Arc<TracingService>,
    tracing_client_context: Arc<ClientContext>,
    merges: MergeTasks,
}

struct RunEnv {
    semaphore: Arc<Semaphore>,
    registry: Arc<SessionRegistry>,
    store: Arc<SessionStore>,
    publisher: Publisher,
    tz: chrono_tz::Tz,
    messenger: Arc<AgentMessenger>,
    checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    tracing_service: Arc<TracingService>,
    tracing_client_context: Arc<ClientContext>,
    merges: MergeTasks,
}

impl SessionRuntime {
    #[must_use]
    pub fn new(
        registry: Arc<SessionRegistry>,
        store: Arc<SessionStore>,
        session_budget: Arc<Semaphore>,
        idle_timeouts: impl Into<IdleTimeouts>,
        handles: SessionRuntimeHandles,
    ) -> Self {
        Self {
            semaphore: session_budget,
            registry,
            store,
            idle_timeouts: idle_timeouts.into(),
            publisher: handles.publisher,
            tz: handles.tz,
            messenger: handles.messenger,
            checkpoints: handles.checkpoints,
            tracing_service: handles.tracing_service,
            tracing_client_context: handles.tracing_client_context,
            merges: MergeTasks::default(),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn registry(&self) -> &Arc<SessionRegistry> {
        &self.registry
    }

    fn race_guard_sinks(&self) -> super::listener::RaceGuardSinks<'_> {
        super::listener::RaceGuardSinks {
            registry: &self.registry,
            publisher: &self.publisher,
            store: &self.store,
        }
    }

    #[tracing::instrument(skip_all, fields(session.address = %req.address))]
    pub fn spawn(&self, req: SessionSpawnRequest, resources: Option<SubAgentResources>) {
        let run_id = generate_run_id();
        let resources = resources.map(|res| res.keyed_to_run(&run_id));
        let category = SessionCategory::from_trigger(&req.trigger);
        let purpose = truncate_prompt_preview(&req.subagent_config.opening_text());
        let idle_timeout = self.idle_timeouts.for_trigger(&req.trigger);
        let model = req.subagent_config.model.clone();
        let resolved_model = req.subagent_config.resolved_model.clone();

        let info = SessionInfo {
            address: req.address,
            run_id,
            category,
            trigger: req.trigger,
            source_label: req.source_label,
            state: SessionState::Forking,
            spawner: req.spawner,
            depth: req.depth,
            purpose,
            agent_skill: req.agent_skill,
            model,
            resolved_model,
            conversation_target: req.conversation_target,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: req.overlap,
        };

        let stop_token = req.stop.unwrap_or_default();
        let interrupt_rx = match self.registry.register(info.clone(), stop_token.clone()) {
            Ok(rx) => rx,
            Err(e) => {
                deliver_losing_spawn_input(
                    self.race_guard_sinks(),
                    &info,
                    &req.subagent_config,
                    &e,
                );
                return;
            }
        };
        tracing::info!(address = %info.address, source_label = %info.source_label, "session forking");

        let env = RunEnv {
            semaphore: Arc::clone(&self.semaphore),
            registry: Arc::clone(&self.registry),
            store: Arc::clone(&self.store),
            publisher: self.publisher.clone(),
            tz: self.tz,
            messenger: Arc::clone(&self.messenger),
            checkpoints: Arc::clone(&self.checkpoints),
            tracing_service: Arc::clone(&self.tracing_service),
            tracing_client_context: Arc::clone(&self.tracing_client_context),
            merges: self.merges.clone(),
        };
        let config = req.subagent_config;

        residuum_core::util::spawn_in_span(async move {
            publish_session_event(
                &env.publisher,
                &info.address,
                &info.run_id,
                SessionEventKind::Started(Box::new(info.clone())),
            )
            .await;
            env.store.begin_run(&info).await;
            env.registry
                .record_resume_point(&info.address, resume_point(&info))
                .await;

            let panic_info = info.clone();
            let panic_env = PanicRecoveryEnv {
                registry: Arc::clone(&env.registry),
                store: Arc::clone(&env.store),
                publisher: env.publisher.clone(),
                messenger: Arc::clone(&env.messenger),
                tz: env.tz,
                merges: env.merges.clone(),
            };
            let mem_extras = resources.as_ref().map(|res| PanicMemoryExtras {
                handles: res.merge_handles(),
                hop_counter: res.hop_counter.clone(),
            });

            let outcome = std::panic::AssertUnwindSafe(run_session(
                info,
                config,
                resources,
                stop_token,
                interrupt_rx,
                idle_timeout,
                env,
            ))
            .catch_unwind()
            .await;

            if let Err(panic) = outcome {
                let msg = residuum_core::util::panic_message(&*panic);
                tracing::error!(
                    address = %panic_info.address,
                    run_id = %panic_info.run_id,
                    panic = msg,
                    "session task panicked"
                );
                recover_from_panic(&panic_info, &panic_env, msg, mem_extras).await;
            }
        });
    }

    pub async fn shutdown(&self, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        let signalled = self.registry.stop_all();
        if signalled > 0 {
            tracing::info!(sessions = signalled, "stopping live sessions for shutdown");
            if tokio::time::timeout_at(deadline, self.registry.wait_until_empty())
                .await
                .is_ok()
            {
                tracing::info!("all sessions completed for shutdown");
            } else {
                tracing::warn!(
                    remaining = self.registry.list_live().len(),
                    "shutdown timed out waiting for live sessions to complete"
                );
            }
        }
        self.merges.drain(deadline).await;
    }
}

fn deliver_losing_spawn_input(
    sinks: super::listener::RaceGuardSinks<'_>,
    info: &SessionInfo,
    config: &SubAgentConfig,
    register_error: &super::registry::RegisterError,
) {
    tracing::error!(
        address = %info.address,
        error = %register_error,
        "refusing to register a session run: address is already live; falling back to \
         delivering its input into the run that won the race"
    );
    if !config.delivered_messages.is_empty() || !config.delivered_chat_messages.is_empty() {
        deliver_losing_resume_input(sinks, info, config);
        return;
    }
    let content = match &config.context {
        Some(ctx) => format!("{ctx}\n\n{}", config.prompt),
        None => config.prompt.clone(),
    };
    let interrupt = super::listener::race_guard_interrupt(
        &info.trigger,
        config.inbound.clone(),
        info.spawner.clone(),
        info.category,
        content,
        config.hop_count,
    );
    let retryable_inbound = if matches!(interrupt, Interrupt::UserMessage(_)) {
        config.inbound.clone()
    } else {
        None
    };
    let outcome = sinks.registry.deliver(&info.address, interrupt);
    match outcome {
        DeliverOutcome::Delivered => {}
        DeliverOutcome::Completing | DeliverOutcome::Full | DeliverOutcome::NotLive
            if let Some(inbound) = retryable_inbound =>
        {
            tracing::warn!(
                address = %info.address,
                outcome = ?outcome,
                "losing spawn's user message could not be delivered immediately; retrying on a \
                 detached task rather than dropping it"
            );
            let spawn_event = residuum_bus::SpawnRequestEvent {
                address: info.address.clone(),
                skill: info.agent_skill.clone(),
                source_label: info.source_label.clone(),
                prompt: config.prompt.clone(),
                context: config.context.clone(),
                source: info.trigger.clone(),
                model: config.model.clone(),
                spawner: info.spawner.clone(),
                depth: info.depth,
                hop_count: config.hop_count,
                sender: config.sender.clone(),
                conversation: info.conversation_target.clone(),
                inbound: config.inbound.clone(),
                images: config.images.clone(),
                overlap: None,
                carried_history: Vec::new(),
                fire_id: None,
                delivered_messages: Vec::new(),
                delivered_chat_messages: Vec::new(),
            };
            let registry = Arc::clone(sinks.registry);
            let publisher = sinks.publisher.clone();
            let address = info.address.clone();
            residuum_core::util::spawn_in_span(async move {
                super::messaging::retry_race_guard_user_message(
                    registry,
                    publisher,
                    address,
                    inbound,
                    spawn_event,
                )
                .await;
            });
        }
        DeliverOutcome::Completing | DeliverOutcome::Full | DeliverOutcome::NotLive => {
            tracing::error!(
                address = %info.address,
                outcome = ?outcome,
                "failed to deliver a spawn's input into the run that won the race; input dropped"
            );
        }
    }
}

fn deliver_losing_resume_input(
    sinks: super::listener::RaceGuardSinks<'_>,
    info: &SessionInfo,
    config: &SubAgentConfig,
) {
    let spawn = super::messaging::ConversationSpawn {
        source_label: info.source_label.clone(),
        model: config.model.clone(),
        skill: info.agent_skill.clone(),
    };
    for (id, outcome) in super::listener::deliver_losing_resume(
        sinks,
        &info.address,
        config.delivered_messages.clone(),
        config.delivered_chat_messages.clone(),
        spawn,
    ) {
        tracing::error!(
            address = %info.address,
            message_id = %id,
            outcome = ?outcome,
            "failed to deliver a resume's message into the run that won the race; message \
             dropped"
        );
    }
}

struct PanicRecoveryEnv {
    registry: Arc<SessionRegistry>,
    store: Arc<SessionStore>,
    publisher: Publisher,
    messenger: Arc<AgentMessenger>,
    tz: chrono_tz::Tz,
    merges: MergeTasks,
}

struct PanicMemoryExtras {
    handles: MergeHandles,
    hop_counter: HopCounter,
}

async fn recover_from_panic(
    info: &SessionInfo,
    env: &PanicRecoveryEnv,
    panic_msg: &str,
    mem_extras: Option<PanicMemoryExtras>,
) {
    transition_state(
        &env.registry,
        &env.publisher,
        info,
        SessionState::Completing,
    )
    .await;
    publish_session_event(
        &env.publisher,
        &info.address,
        &info.run_id,
        SessionEventKind::Error {
            message: format!("session task panicked: {panic_msg}"),
            details: None,
        },
    )
    .await;

    let lines = env
        .store
        .read_transcript_lines(&info.run_id, info.started_at)
        .await;

    let mut transcript: Vec<residuum_inference::Message> =
        lines.iter().map(|line| line.message.clone()).collect();
    let merge = mem_extras.as_ref().map(|extras| {
        let summary = lines
            .last()
            .map(|line| line.message.content.clone())
            .unwrap_or_default();
        CompletionMerge::for_run(
            info,
            summary,
            lines,
            SessionMemory::new(),
            extras.handles.clone(),
            env.tz,
        )
    });

    let status = AgentResultStatus::Failed {
        error: format!("session task panicked: {panic_msg}"),
        details: None,
    };

    if info.category == SessionCategory::Spawned
        && let Some(spawner) = info.spawner.clone()
    {
        let hop_count = mem_extras.as_ref().map_or(0, |m| m.hop_counter.outgoing());
        let content = relay_content(&info.address, &status, "");
        let relay_env = RelayEnv {
            messenger: &env.messenger,
            store: &env.store,
            publisher: &env.publisher,
        };
        if let Some(note) =
            relay_result_to_spawner(info, &spawner, &content, hop_count, &relay_env).await
        {
            transcript.push(note);
        }
    }

    env.registry
        .record_resume_point(&info.address, resume_point(info))
        .await;

    let transcript_path = env
        .store
        .complete_run(
            info,
            SessionState::Completed.as_str(),
            &status,
            transcript,
            merge.is_some(),
        )
        .await;

    let event = build_result_event(info, status.clone(), String::new(), transcript_path, env.tz);
    if let Err(e) = env.publisher.publish(topics::Background, event).await {
        tracing::warn!(error = %e, "failed to publish panicked session result to bus");
    }

    env.registry.remove(&info.address, &info.run_id);
    publish_session_event(
        &env.publisher,
        &info.address,
        &info.run_id,
        SessionEventKind::Completed {
            status,
            memory_pending: merge.is_some(),
        },
    )
    .await;
    tracing::info!("session removed from registry after panic recovery");

    if let Some(merge) = merge {
        spawn_memory_merge(
            &env.merges,
            merge,
            info.clone(),
            Arc::clone(&env.store),
            env.publisher.clone(),
        );
    }
}

fn resume_point(info: &SessionInfo) -> ResumePoint {
    ResumePoint {
        previous_run_id: info.run_id.clone(),
        trigger: info.trigger.clone(),
        source_label: info.source_label.clone(),
        agent_skill: info.agent_skill.clone(),
        model: info.model.clone(),
        spawner: info.spawner.clone(),
        depth: info.depth,
        conversation_target: info.conversation_target.clone(),
        recorded_at: chrono::Utc::now(),
    }
}

async fn transition_state(
    registry: &SessionRegistry,
    publisher: &Publisher,
    info: &SessionInfo,
    state: SessionState,
) {
    registry.set_state(&info.address, state);
    publish_session_event(
        publisher,
        &info.address,
        &info.run_id,
        SessionEventKind::StateChanged(state),
    )
    .await;
}

enum IdleOutcome {
    Stopped,
    TimedOut,
    Woken(AgentMessageEvent),
    WokenExternal(MessageEvent),
}

async fn wait_idle(
    stop_token: &CancellationToken,
    idle_timeout: Duration,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> IdleOutcome {
    loop {
        tokio::select! {
            biased;
            () = stop_token.cancelled() => return IdleOutcome::Stopped,
            () = tokio::time::sleep(idle_timeout) => {
                return match interrupt_rx.try_recv() {
                    Ok(Interrupt::AgentMessage(msg)) => IdleOutcome::Woken(msg),
                    Ok(Interrupt::UserMessage(msg)) => IdleOutcome::WokenExternal(msg),
                    Ok(_) | Err(_) => IdleOutcome::TimedOut,
                };
            }
            received = interrupt_rx.recv() => {
                match received {
                    Some(Interrupt::AgentMessage(msg)) => return IdleOutcome::Woken(msg),
                    Some(Interrupt::UserMessage(msg)) => return IdleOutcome::WokenExternal(msg),
                    Some(_) => {}
                    None => return IdleOutcome::TimedOut,
                }
            }
        }
    }
}

struct FinishedTurn<'a> {
    status: &'a AgentResultStatus,
    summary: &'a str,
    first_message: usize,
}

async fn after_turn(
    info: &SessionInfo,
    turn: &FinishedTurn<'_>,
    resources: Option<&SubAgentResources>,
    env: &RunEnv,
    memory: &mut SessionMemory,
    history: &mut RunHistory,
) {
    let FinishedTurn {
        status,
        summary,
        first_message,
    } = *turn;
    if let Some(res) = resources {
        let mem_env = session_memory_env(res, env.tz);
        memory
            .maybe_stage(&history.own_timed(), &info.run_id, &mem_env)
            .await;
    }
    let recent_messages = &mut history.messages;

    maybe_relay_result(info, status, summary, resources, env, recent_messages).await;

    settle_chat_output(
        info,
        status,
        summary,
        history.messages.messages_since(first_message),
        env,
    )
    .await;
}

struct RunHistory {
    messages: RecentMessages,
    carried: usize,
}

impl RunHistory {
    fn new(carried_history: Vec<residuum_inference::Message>) -> Self {
        let carried = carried_history.len();
        let mut messages = RecentMessages::new();
        messages.extend(carried_history);
        Self { messages, carried }
    }

    fn own(&self) -> &[residuum_inference::Message] {
        self.messages.messages_since(self.carried)
    }

    fn own_timed(&self) -> Vec<TimedMessage<'_>> {
        self.own()
            .iter()
            .zip(self.messages.recorded_at_since(self.carried))
            .map(|(message, at)| TimedMessage {
                message,
                recorded_at: Some(*at),
            })
            .collect()
    }
}

fn initial_kickoff(config: SubAgentConfig, chat_note: Option<String>) -> TurnKickoff {
    let context = match (chat_note, config.context) {
        (Some(note), Some(context)) => Some(format!("{note}\n\n{context}")),
        (note, context) => note.or(context),
    };
    TurnKickoff::Initial {
        prompt: config.prompt,
        context,
        hop_count: config.hop_count,
        sender: config.sender,
        images: config.images,
        delivered: config.delivered_messages,
        resumed_by_chat: !config.delivered_chat_messages.is_empty(),
    }
}

#[tracing::instrument(skip_all, fields(session.address = %info.address, run.id = %info.run_id))]
async fn run_session(
    mut info: SessionInfo,
    mut config: SubAgentConfig,
    resources: Option<SubAgentResources>,
    stop_token: CancellationToken,
    mut interrupt_rx: mpsc::Receiver<Interrupt>,
    idle_timeout: Duration,
    env: RunEnv,
) {
    let mut history = RunHistory::new(std::mem::take(&mut config.carried_history));
    let mut memory = SessionMemory::new();
    let chat_note = chat_session_note(&info);
    let mut kickoff = initial_kickoff(config, chat_note);
    let mut status;
    let mut summary;
    let mut turn_number: u32 = 0;

    loop {
        transition_state(&env.registry, &env.publisher, &info, SessionState::Queued).await;
        let first_message = history.messages.len();
        (status, summary) = tokio::select! {
            biased;
            () = stop_token.cancelled() => {
                tracing::info!("session stopped before its turn started");
                (AgentResultStatus::Cancelled, String::new())
            }
            permit = env.semaphore.acquire() => {
                match permit {
                    Ok(_permit) => {
                        transition_state(&env.registry, &env.publisher, &info, SessionState::Running).await;
                        turn_number += 1;
                        let ctx = turn_ctx(&info, resources.as_ref(), &stop_token, &env);
                        let turn_id = format!("{}-t{turn_number}", info.run_id);
                        run_turn(&ctx, &turn_id, &mut history.messages, kickoff, &mut interrupt_rx).await
                    }
                    Err(_closed) => {
                        tracing::warn!("concurrency semaphore closed before session could acquire a permit");
                        (
                            AgentResultStatus::Failed {
                                error: "gateway is shutting down".to_string(),
                                details: None,
                            },
                            String::new(),
                        )
                    }
                }
            }
        };

        after_turn(
            &info,
            &FinishedTurn {
                status: &status,
                summary: &summary,
                first_message,
            },
            resources.as_ref(),
            &env,
            &mut memory,
            &mut history,
        )
        .await;

        if matches!(status, AgentResultStatus::Cancelled) {
            tracing::info!("session stopped, skipping idle and completing directly");
            break;
        }

        transition_state(&env.registry, &env.publisher, &info, SessionState::Idle).await;
        tracing::debug!(
            idle_secs = idle_timeout.as_secs(),
            "session idle, awaiting further input or timeout"
        );
        match wait_idle(&stop_token, idle_timeout, &mut interrupt_rx).await {
            IdleOutcome::Stopped => {
                tracing::info!("session stopped while idle");
                status = AgentResultStatus::Cancelled;
                break;
            }
            IdleOutcome::TimedOut => {
                tracing::debug!("session idle timeout elapsed");
                break;
            }
            IdleOutcome::Woken(msg) => {
                tracing::info!(from = %msg.from, "session woken by an agent message, starting another turn");
                kickoff = TurnKickoff::AgentMessage(msg);
            }
            IdleOutcome::WokenExternal(msg) => {
                tracing::info!(msg_id = %msg.id, "session woken by a conversation message, starting another turn");
                kickoff = TurnKickoff::External(msg);
            }
        }
    }

    let leftover_messages = drain_pending_interrupts(&mut interrupt_rx);

    finish_run(
        &mut info,
        resources.as_ref(),
        RunOutcome {
            status,
            summary,
            memory,
            history,
            leftover_messages,
        },
        &env,
    )
    .await;
}

fn drain_pending_interrupts(interrupt_rx: &mut mpsc::Receiver<Interrupt>) -> Vec<PendingInput> {
    let mut drained = Vec::new();
    while let Ok(interrupt) = interrupt_rx.try_recv() {
        match interrupt {
            Interrupt::AgentMessage(msg) => drained.push(PendingInput::Agent(msg)),
            Interrupt::UserMessage(msg) => drained.push(PendingInput::External(msg)),
            Interrupt::Subconscious(_) | Interrupt::Stopped => {}
        }
    }
    drained
}

struct RunOutcome {
    status: AgentResultStatus,
    summary: String,
    memory: SessionMemory,
    history: RunHistory,
    leftover_messages: Vec<PendingInput>,
}

async fn finish_run(
    info: &mut SessionInfo,
    resources: Option<&SubAgentResources>,
    outcome: RunOutcome,
    env: &RunEnv,
) {
    let RunOutcome {
        status,
        summary,
        memory,
        history,
        leftover_messages,
    } = outcome;

    info.state = SessionState::Completing;
    transition_state(
        &env.registry,
        &env.publisher,
        info,
        SessionState::Completing,
    )
    .await;

    let merge = resources.map(|res| {
        CompletionMerge::for_run(
            info,
            summary.clone(),
            owned_transcript(&history.own_timed()),
            memory,
            res.merge_handles(),
            env.tz,
        )
    });

    let point = resume_point(info);
    env.registry
        .record_resume_point(&info.address, point.clone())
        .await;

    let transcript_path = env
        .store
        .complete_run(
            info,
            SessionState::Completed.as_str(),
            &status,
            history.own().to_vec(),
            merge.is_some(),
        )
        .await;

    let event = build_result_event(info, status.clone(), summary, transcript_path, env.tz);
    if let Err(e) = env.publisher.publish(topics::Background, event).await {
        tracing::warn!(error = %e, "failed to publish session result to bus");
    }

    env.registry.remove(&info.address, &info.run_id);
    publish_session_event(
        &env.publisher,
        &info.address,
        &info.run_id,
        SessionEventKind::Completed {
            status,
            memory_pending: merge.is_some(),
        },
    )
    .await;
    tracing::info!("session completed");

    if let Some(merge) = merge {
        spawn_memory_merge(
            &env.merges,
            merge,
            info.clone(),
            Arc::clone(&env.store),
            env.publisher.clone(),
        );
    }

    if !leftover_messages.is_empty() {
        tracing::warn!(
            address = %info.address,
            count = leftover_messages.len(),
            "message(s) arrived as the run was tearing down; resuming the session as a new run to deliver them"
        );
        if let Err(e) = env
            .messenger
            .resume_with_pending(&info.address, &point, &leftover_messages)
            .await
        {
            tracing::error!(
                address = %info.address,
                error = %e,
                "failed to resume session to deliver messages queued at teardown"
            );
        }
    }
}

fn session_memory_env(res: &SubAgentResources, tz: chrono_tz::Tz) -> SessionMemoryEnv<'_> {
    SessionMemoryEnv {
        observer: &res.observer,
        merge_writer: &res.merge_writer,
        layout: &res.layout,
        episode_skip_token_floor: res.episode_skip_token_floor,
        tz,
    }
}

fn owned_transcript(transcript: &[TimedMessage<'_>]) -> Vec<TranscriptLine> {
    transcript
        .iter()
        .map(|m| TranscriptLine {
            message: m.message.clone(),
            recorded_at: m.recorded_at,
        })
        .collect()
}

fn spawn_memory_merge(
    merges: &MergeTasks,
    merge: CompletionMerge,
    info: SessionInfo,
    store: Arc<SessionStore>,
    publisher: Publisher,
) {
    merges.spawn(info.run_id.clone(), async move {
        let episode_id = merge.run().await;
        store.finish_memory_merge(&info, episode_id.clone()).await;
        publish_session_event(
            &publisher,
            &info.address,
            &info.run_id,
            SessionEventKind::MemoryMerged { episode_id },
        )
        .await;
    });
}

fn relay_content(address: &SessionAddress, status: &AgentResultStatus, summary: &str) -> String {
    match status {
        AgentResultStatus::Completed if !summary.is_empty() => summary.to_string(),
        AgentResultStatus::Completed
        | AgentResultStatus::Cancelled
        | AgentResultStatus::Failed { .. } => {
            format!("[Session Result] {address} — status: {status}")
        }
    }
}

async fn maybe_relay_result(
    info: &SessionInfo,
    status: &AgentResultStatus,
    summary: &str,
    resources: Option<&SubAgentResources>,
    env: &RunEnv,
    recent_messages: &mut RecentMessages,
) {
    if info.category != SessionCategory::Spawned {
        return;
    }
    let Some(spawner) = info.spawner.clone() else {
        return;
    };
    let content = relay_content(&info.address, status, summary);
    let hop_count = resources.map_or(0, |res| res.hop_counter.outgoing());
    let relay_env = RelayEnv {
        messenger: &env.messenger,
        store: &env.store,
        publisher: &env.publisher,
    };
    if let Some(note) =
        relay_result_to_spawner(info, &spawner, &content, hop_count, &relay_env).await
    {
        recent_messages.push(note);
    }
}

async fn relay_result_to_spawner(
    info: &SessionInfo,
    spawner: &SessionAddress,
    content: &str,
    hop_count: u32,
    env: &RelayEnv<'_>,
) -> Option<residuum_inference::Message> {
    let outcome = env
        .messenger
        .send(
            spawner.as_ref(),
            residuum_bus::AgentMessageEvent::new(
                info.address.clone(),
                info.category.as_str(),
                content,
                hop_count,
            ),
        )
        .await;

    let failure = match outcome {
        Ok(DeliveryOutcome::Unknown) => {
            Some("spawner is no longer reachable (it may have restarted)".to_string())
        }
        Ok(
            DeliveryOutcome::Main
            | DeliveryOutcome::Live(_)
            | DeliveryOutcome::Resumed(_)
            | DeliveryOutcome::Queued(_),
        ) => None,
        Err(e) => Some(e.to_string()),
    };

    let reason = failure?;
    tracing::warn!(
        address = %info.address,
        spawner = %spawner,
        error = %reason,
        "failed to relay turn result to spawner"
    );
    let note_text = format!(
        "[Result Relay Failed] could not deliver this turn's result to spawner {spawner}: {reason}"
    );
    publish_session_event(
        env.publisher,
        &info.address,
        &info.run_id,
        SessionEventKind::Error {
            message: note_text.clone(),
            details: None,
        },
    )
    .await;
    Some(
        env.store
            .append_note(&info.run_id, info.started_at, &note_text)
            .await,
    )
}

fn external_chat(info: &SessionInfo) -> Option<&ConversationTarget> {
    info.conversation_target.as_ref().filter(|target| {
        info.category == SessionCategory::External && target.endpoint != A2A_ENDPOINT
    })
}

fn chat_session_note(info: &SessionInfo) -> Option<String> {
    let target = external_chat(info)?;
    Some(format!(
        "[You are handling {label}. Nothing you write is posted there on its own: only a \
         send_message call with endpoint=\"{endpoint}\" and conversation=\"{conversation}\" \
         reaches that chat, so send everything its people should read that way. Your other \
         responses stay in this session's transcript, which the owner can read in the web UI.]",
        label = info.source_label,
        endpoint = target.endpoint,
        conversation = target.conversation_id,
    ))
}

fn sent_to_chat(messages: &[residuum_inference::Message], target: &ConversationTarget) -> bool {
    messages
        .iter()
        .filter_map(|message| message.tool_calls.as_deref())
        .flatten()
        .any(|call| {
            let arg = |name: &str| call.arguments.get(name).and_then(serde_json::Value::as_str);
            call.name == "send_message"
                && arg("endpoint") == Some(target.endpoint.as_str())
                && arg("conversation") == Some(target.conversation_id.as_str())
        })
}

async fn settle_chat_output(
    info: &SessionInfo,
    status: &AgentResultStatus,
    summary: &str,
    turn_messages: &[residuum_inference::Message],
    env: &RunEnv,
) {
    let Some(target) = info.conversation_target.as_ref() else {
        return;
    };
    if !matches!(status, AgentResultStatus::Completed) || summary.is_empty() {
        tracing::debug!(
            address = %info.address,
            status = %status,
            "conversation session turn produced no output to deliver"
        );
        return;
    }
    if info.category != SessionCategory::Spawned {
        if external_chat(info).is_some() && !sent_to_chat(turn_messages, target) {
            tracing::info!(
                address = %info.address,
                endpoint = %target.endpoint,
                conversation = %target.conversation_id,
                "conversation session turn ended without a send_message to its chat; its reply \
                 stays in the session transcript and the chat heard nothing"
            );
        }
        return;
    }
    let event = SessionResponseEvent {
        session_address: info.address.clone(),
        conversation_id: target.conversation_id.clone(),
        content: format!("{}:\n{summary}", info.address),
        attachment: None,
        timestamp: residuum_core::time::now_local(env.tz),
        is_final: true,
    };
    if let Err(e) = env
        .publisher
        .publish(
            topics::Endpoint(EndpointName::from(target.endpoint.as_str())),
            event,
        )
        .await
    {
        tracing::error!(
            address = %info.address,
            conversation = %target.conversation_id,
            error = %e,
            "failed to publish conversation session output to the bus"
        );
    }
}

async fn publish_conversation_typing(
    publisher: &Publisher,
    target: Option<&residuum_bus::ConversationTarget>,
    active: bool,
) {
    let Some(target) = target else {
        return;
    };
    if let Err(e) = publisher
        .publish(
            topics::Endpoint(EndpointName::from(target.endpoint.as_str())),
            ConversationTypingEvent {
                conversation_id: target.conversation_id.clone(),
                active,
            },
        )
        .await
    {
        tracing::warn!(
            conversation = %target.conversation_id,
            error = %e,
            "failed to publish conversation typing signal"
        );
    }
}

struct RelayEnv<'a> {
    messenger: &'a AgentMessenger,
    store: &'a SessionStore,
    publisher: &'a Publisher,
}

struct TurnCtx<'a> {
    info: &'a SessionInfo,
    resources: Option<&'a SubAgentResources>,
    stop_token: &'a CancellationToken,
    store: &'a SessionStore,
    publisher: &'a Publisher,
    registry: &'a SessionRegistry,
    checkpoints: &'a residuum_checkpoints::CheckpointEngine,
    tracing_service: &'a TracingService,
    tracing_client_context: &'a ClientContext,
}

fn turn_ctx<'a>(
    info: &'a SessionInfo,
    resources: Option<&'a SubAgentResources>,
    stop_token: &'a CancellationToken,
    env: &'a RunEnv,
) -> TurnCtx<'a> {
    TurnCtx {
        info,
        resources,
        stop_token,
        store: &env.store,
        publisher: &env.publisher,
        registry: env.registry.as_ref(),
        checkpoints: &env.checkpoints,
        tracing_service: &env.tracing_service,
        tracing_client_context: &env.tracing_client_context,
    }
}

async fn run_turn(
    ctx: &TurnCtx<'_>,
    turn_id: &str,
    recent_messages: &mut RecentMessages,
    kickoff: TurnKickoff,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> (AgentResultStatus, String) {
    let publish =
        |kind| publish_session_event(ctx.publisher, &ctx.info.address, &ctx.info.run_id, kind);
    publish(SessionEventKind::TurnStarted {
        turn_id: turn_id.to_string(),
    })
    .await;
    publish_conversation_typing(ctx.publisher, ctx.info.conversation_target.as_ref(), true).await;
    ctx.checkpoints
        .spawn_turn_start_checkpoint(turn_checkpoint_context(
            ctx,
            turn_id,
            residuum_checkpoints::CheckpointTrigger::TurnStart,
            "outside edit before turn start".to_string(),
        ));

    let result = execute_turn_outcome(ctx, turn_id, recent_messages, kickoff, interrupt_rx).await;

    match &result {
        (AgentResultStatus::Completed, summary) if !summary.is_empty() => {
            publish(SessionEventKind::Response {
                turn_id: turn_id.to_string(),
                content: summary.clone(),
            })
            .await;
        }
        (AgentResultStatus::Failed { error, details }, _) => {
            publish(SessionEventKind::Error {
                message: format!("turn failed: {error}"),
                details: details.clone(),
            })
            .await;
        }
        (AgentResultStatus::Completed | AgentResultStatus::Cancelled, _) => {}
    }
    publish(SessionEventKind::TurnEnded {
        turn_id: turn_id.to_string(),
    })
    .await;
    publish_conversation_typing(ctx.publisher, ctx.info.conversation_target.as_ref(), false).await;
    ctx.checkpoints
        .spawn_turn_end_checkpoint(turn_checkpoint_context(
            ctx,
            turn_id,
            residuum_checkpoints::CheckpointTrigger::TurnEnd,
            turn_end_summary(&result),
        ));
    result
}

fn turn_checkpoint_context(
    ctx: &TurnCtx<'_>,
    turn_id: &str,
    trigger: residuum_checkpoints::CheckpointTrigger,
    summary: String,
) -> residuum_checkpoints::CheckpointContext {
    residuum_checkpoints::CheckpointContext {
        address: ctx.info.address.to_string(),
        run_id: Some(ctx.info.run_id.clone()),
        turn_id: Some(turn_id.to_string()),
        trigger,
        summary,
    }
}

fn turn_end_summary(result: &(AgentResultStatus, String)) -> String {
    match result {
        (AgentResultStatus::Completed, summary) if !summary.is_empty() => {
            truncate_prompt_preview(summary)
        }
        (AgentResultStatus::Completed, _) => "turn completed".to_string(),
        (AgentResultStatus::Cancelled, _) => "turn stopped".to_string(),
        (AgentResultStatus::Failed { error, .. }, _) => format!("turn failed: {error}"),
    }
}

async fn execute_turn_outcome(
    ctx: &TurnCtx<'_>,
    turn_id: &str,
    recent_messages: &mut RecentMessages,
    kickoff: TurnKickoff,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> (AgentResultStatus, String) {
    let sink = RunTranscriptSink {
        store: ctx.store,
        run_id: &ctx.info.run_id,
        started_at: ctx.info.started_at,
    };
    let conversation_output =
        ctx.info
            .conversation_target
            .as_ref()
            .map(|target| super::subagent::ConversationOutput {
                endpoint: target.endpoint.as_str(),
                conversation_id: target.conversation_id.as_str(),
            });
    let outcome = async {
        let res = ctx
            .resources
            .ok_or_else(|| anyhow::anyhow!("session run requires SubAgentResources"))?;
        let usage_sink =
            super::registry::SessionUsageSink::new(ctx.registry, ctx.info, &res.layout);
        let identity = SessionTurnIdentity {
            publisher: ctx.publisher,
            address: &ctx.info.address,
            run_id: &ctx.info.run_id,
            turn_id,
        };
        execute_subagent(
            &identity,
            kickoff,
            recent_messages,
            res,
            super::subagent::TurnExecution {
                stop_token: ctx.stop_token,
                transcript_sink: Some(&sink),
                usage_sink: Some(&usage_sink),
                interrupt_rx,
            },
            conversation_output,
        )
        .await
    }
    .await;

    match outcome {
        Ok(summary) if ctx.stop_token.is_cancelled() => {
            tracing::info!("session turn stopped mid-flight");
            (AgentResultStatus::Cancelled, summary)
        }
        Ok(summary) => {
            tracing::info!("session turn completed");
            (AgentResultStatus::Completed, summary)
        }
        Err(e) => {
            let described = residuum_inference::describe_turn_failure(&e);
            tracing::warn!(error = %described.details, "session turn failed");
            let mut client = (*ctx.tracing_client_context).clone();
            client.active_subagents = ctx.registry.subagent_snapshot();
            ctx.tracing_service
                .on_error(&described.details, client)
                .await;
            (
                AgentResultStatus::Failed {
                    error: described.message,
                    details: Some(described.details),
                },
                String::new(),
            )
        }
    }
}

fn build_result_event(
    info: &SessionInfo,
    status: AgentResultStatus,
    summary: String,
    transcript_path: Option<std::path::PathBuf>,
    tz: chrono_tz::Tz,
) -> AgentResultEvent {
    let disposition = if matches!(info.trigger, EventTrigger::Pulse)
        && ends_with_sentinel(&summary, HEARTBEAT_OK)
    {
        ResultDisposition::Silent
    } else if ends_with_sentinel(&summary, HEARTBEAT_URGENT) {
        ResultDisposition::Urgent
    } else {
        ResultDisposition::Normal
    };

    AgentResultEvent {
        session_address: info.address.clone(),
        run_id: info.run_id.clone(),
        source_label: info.source_label.clone(),
        agent_skill: info.agent_skill.clone(),
        source: info.trigger.clone(),
        disposition,
        status,
        summary,
        transcript_path,
        timestamp: Utc::now().with_timezone(&tz).naive_local(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use residuum_bus::testing::{bus_barrier, next_event};
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;
    use residuum_inference::testing::model::GatedProvider;
    use residuum_inference::{
        CompletionOptions, InferenceError, InferenceResponse, Message, ToolDefinition,
    };
    use residuum_mcp::McpRegistry;
    use residuum_skills::{SkillIndex, SkillState};
    use residuum_workspace::identity::IdentityFiles;

    use crate::testing::{
        SequencedProvider, make_sequenced_resources, next_idle, sample_conversation_request,
        test_checkpoints, test_memory_extras, test_runtime_held_idle, test_store,
        test_tracing_client_context, test_tracing_service,
    };
    use residuum_bus::MessageEvent;
    use residuum_bus::session::{MAIN_ADDRESS, MAIN_DEPTH};

    async fn test_runtime(
        max_concurrent: usize,
    ) -> (SessionRuntime, residuum_bus::Subscriber<AgentResultEvent>) {
        let bus_handle = residuum_bus::spawn_broker();
        let sub = bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let idle_timeouts = IdleTimeouts {
            scheduled: Duration::from_millis(20),
            spawned: Duration::from_millis(20),
            external: Duration::from_millis(20),
            artifact: Duration::from_millis(20),
        };
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(max_concurrent)),
            idle_timeouts,
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );
        (runtime, sub)
    }

    async fn test_runtime_with_tracing_service(
        max_concurrent: usize,
        tracing_service: Arc<TracingService>,
    ) -> (SessionRuntime, residuum_bus::Subscriber<AgentResultEvent>) {
        let bus_handle = residuum_bus::spawn_broker();
        let sub = bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let idle_timeouts = IdleTimeouts {
            scheduled: Duration::from_millis(20),
            spawned: Duration::from_millis(20),
            external: Duration::from_millis(20),
            artifact: Duration::from_millis(20),
        };
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(max_concurrent)),
            idle_timeouts,
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service,
                tracing_client_context: test_tracing_client_context(),
            },
        );
        (runtime, sub)
    }

    #[tokio::test]
    async fn publish_conversation_typing_is_a_noop_with_no_target() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub = bus_handle
            .subscribe::<_, ConversationTypingEvent>(topics::Endpoint(EndpointName::from(
                "discord",
            )))
            .await
            .unwrap();
        let publisher = bus_handle.publisher();

        publish_conversation_typing(&publisher, None, true).await;

        bus_barrier(&bus_handle).await;
        assert!(
            sub.drain().is_empty(),
            "a session with no conversation target must publish nothing"
        );
    }

    #[tokio::test]
    async fn publish_conversation_typing_reaches_the_targets_endpoint() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub = bus_handle
            .subscribe::<_, ConversationTypingEvent>(topics::Endpoint(EndpointName::from(
                "discord",
            )))
            .await
            .unwrap();
        let publisher = bus_handle.publisher();
        let target = ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "555".to_string(),
        };

        publish_conversation_typing(&publisher, Some(&target), true).await;
        let started = sub.recv().await.unwrap().unwrap();
        assert_eq!(started.conversation_id, "555");
        assert!(started.active);

        publish_conversation_typing(&publisher, Some(&target), false).await;
        let ended = sub.recv().await.unwrap().unwrap();
        assert_eq!(ended.conversation_id, "555");
        assert!(!ended.active);
    }

    struct MockProvider {
        response: String,
    }

    #[async_trait]
    impl residuum_inference::InferenceProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Ok(InferenceResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock"
        }
    }

    fn make_resources(response: &str) -> SubAgentResources {
        let (layout, observer, merge_writer) = test_memory_extras();
        SubAgentResources {
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
            provider: Box::new(MockProvider {
                response: response.to_string(),
            }),
            tools: residuum_agent::ToolRegistry::new(),
            mcp_registry: McpRegistry::new_shared(),
            skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
            hop_counter: residuum_agent::HopCounter::new(0),
        }
    }

    fn make_gated_resources(response: &str, gate: &Gate) -> SubAgentResources {
        let (layout, observer, merge_writer) = test_memory_extras();
        SubAgentResources {
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
            provider: Box::new(GatedProvider::new(gate, response)),
            tools: residuum_agent::ToolRegistry::new(),
            mcp_registry: McpRegistry::new_shared(),
            skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
            hop_counter: residuum_agent::HopCounter::new(0),
        }
    }

    fn sample_request(address: &str) -> SessionSpawnRequest {
        SessionSpawnRequest {
            address: SessionAddress::from(address),
            source_label: "agent:researcher".to_string(),
            trigger: EventTrigger::Agent,
            agent_skill: None,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            subagent_config: SubAgentConfig {
                prompt: "do the thing".to_string(),
                context: None,
                model: residuum_config::ModelSelector::MEDIUM,
                resolved_model: None,
                hop_count: 0,
                sender: None,
                inbound: None,
                images: Vec::new(),
                carried_history: Vec::new(),
                delivered_messages: Vec::new(),
                delivered_chat_messages: Vec::new(),
            },
            conversation_target: None,
            overlap: None,
            stop: None,
        }
    }

    fn sample_inbound_message(content: &str) -> MessageEvent {
        MessageEvent {
            id: "conv-msg-1".to_string(),
            content: content.to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: None,
                conversation: Some(residuum_bus::origin::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: residuum_bus::origin::ConversationKind::Channel,
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

    #[test]
    fn deliver_losing_spawn_input_for_a_conversation_trigger_preserves_sender_attribution() {
        let registry = Arc::new(SessionRegistry::new());
        let publisher = Publisher::noop();
        let address = SessionAddress::from("external-discord-race");
        let winner = SessionInfo {
            address: address.clone(),
            run_id: "run-winner".to_string(),
            category: SessionCategory::External,
            trigger: EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: SessionState::Idle,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let mut rx = registry
            .register(winner.clone(), CancellationToken::new())
            .expect("the winning run registers first");

        let losing_info = SessionInfo {
            run_id: "run-loser".to_string(),
            ..winner
        };
        let losing_inbound = sample_inbound_message("any updates?");
        let config = SubAgentConfig {
            prompt: "any updates?".to_string(),
            context: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            hop_count: 0,
            sender: None,
            inbound: Some(losing_inbound),
            images: Vec::new(),
            carried_history: Vec::new(),
            delivered_messages: Vec::new(),
            delivered_chat_messages: Vec::new(),
        };
        let register_error = crate::registry::RegisterError {
            address: address.clone(),
        };

        deliver_losing_spawn_input(
            race_guard_sinks(&registry, &publisher, &test_store()),
            &losing_info,
            &config,
            &register_error,
        );

        let delivered = rx
            .try_recv()
            .expect("the losing run's content must be delivered into the winning run");
        match delivered {
            Interrupt::UserMessage(m) => assert_eq!(m.content, "any updates?"),
            Interrupt::AgentMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!(
                    "expected a UserMessage interrupt for a conversation trigger, not an agent \
                     message misattributed to main"
                )
            }
        }
    }

    #[test]
    fn deliver_losing_spawn_input_for_a_resume_delivers_each_message_as_it_was_sent() {
        use residuum_bus::session::{OWNER_ADDRESS as OWNER, TEAMMATE_SENDER_CATEGORY as TEAMMATE};
        let registry = Arc::new(SessionRegistry::new());
        let publisher = Publisher::noop();
        let address = SessionAddress::from("spawned-researcher-race-resume");
        let spawner = SessionAddress::from("spawned-lead-7c1d");
        let winner = SessionInfo {
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            spawner: Some(spawner.clone()),
            conversation_target: None,
            ..conversation_winner_info(&address, "run-winner")
        };
        let mut rx = registry
            .register(winner.clone(), CancellationToken::new())
            .expect("the winning run registers first");

        let losing_info = SessionInfo {
            run_id: "run-loser".to_string(),
            ..winner
        };
        let context = "[Resumed session] Your previous run was merged as episode ep-1.";
        let config = SubAgentConfig {
            prompt: "[From: Jane via discord (#builds)] any news?".to_string(),
            context: Some(context.to_string()),
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            hop_count: 2,
            sender: None,
            inbound: None,
            images: Vec::new(),
            carried_history: Vec::new(),
            delivered_messages: vec![
                residuum_bus::AgentMessageEvent::new(OWNER.into(), "owner", "are you there?", 0)
                    .with_id("m-owner"),
                residuum_bus::AgentMessageEvent::new(
                    "agent:ivan".into(),
                    TEAMMATE,
                    "the logs are in /tmp",
                    1,
                )
                .with_id("m-teammate"),
            ],
            delivered_chat_messages: vec![jane_chat_message("m-chat", "any news?")],
        };
        let register_error = crate::registry::RegisterError {
            address: address.clone(),
        };

        deliver_losing_spawn_input(
            race_guard_sinks(&registry, &publisher, &test_store()),
            &losing_info,
            &config,
            &register_error,
        );

        let (delivered, chat_delivered) = split_agent_then_chat(&mut rx);
        let summary: Vec<(&str, &str, &str, u32)> = delivered
            .iter()
            .map(|m| {
                (
                    m.from.as_ref(),
                    m.from_category.as_str(),
                    m.content.as_str(),
                    m.hop_count,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (OWNER, "owner", "are you there?", 0),
                ("agent:ivan", TEAMMATE, "the logs are in /tmp", 1),
            ],
            "the chat text is not delivered as an agent message from the spawner"
        );
        let ids: Vec<&str> = delivered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m-owner", "m-teammate"], "each keeps its own id");
        let chat_summary: Vec<(&str, &str, Option<&str>)> = chat_delivered
            .iter()
            .map(|m| {
                (
                    m.id.as_str(),
                    m.content.as_str(),
                    m.origin.sender.as_ref().map(|s| s.name.as_str()),
                )
            })
            .collect();
        assert_eq!(
            chat_summary,
            vec![("m-chat", "any news?", Some("Jane"))],
            "the chat message arrives as the chat message it is, with its own sender"
        );
        assert!(
            delivered.iter().all(|m| !m.content.contains(context)),
            "the resume context stays out of the delivered messages"
        );
    }

    fn jane_chat_message(id: &str, content: &str) -> MessageEvent {
        let inbound = sample_inbound_message(content);
        MessageEvent {
            id: id.to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                sender: Some(residuum_inference::MessageSender {
                    name: "Jane".to_string(),
                    id: "discord-jane".to_string(),
                    interface: "discord".to_string(),
                    location: Some("#builds".to_string()),
                }),
                ..inbound.origin
            },
            ..inbound
        }
    }

    fn split_agent_then_chat(
        rx: &mut mpsc::Receiver<Interrupt>,
    ) -> (Vec<residuum_bus::AgentMessageEvent>, Vec<MessageEvent>) {
        let mut agent = Vec::new();
        let mut chat = Vec::new();
        while let Ok(interrupt) = rx.try_recv() {
            match interrupt {
                Interrupt::AgentMessage(m) => {
                    assert!(
                        chat.is_empty(),
                        "the agent messages arrive ahead of the chat messages"
                    );
                    agent.push(m);
                }
                Interrupt::UserMessage(m) => chat.push(m),
                Interrupt::Subconscious(_) | Interrupt::Stopped => {
                    panic!("a losing resume delivers agent and chat messages only")
                }
            }
        }
        (agent, chat)
    }

    fn race_guard_sinks<'a>(
        registry: &'a Arc<SessionRegistry>,
        publisher: &'a Publisher,
        store: &'a Arc<SessionStore>,
    ) -> crate::listener::RaceGuardSinks<'a> {
        crate::listener::RaceGuardSinks {
            registry,
            publisher,
            store,
        }
    }

    fn conversation_winner_info(address: &SessionAddress, run_id: &str) -> SessionInfo {
        SessionInfo {
            address: address.clone(),
            run_id: run_id.to_string(),
            category: SessionCategory::External,
            trigger: EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: SessionState::Idle,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    #[tokio::test]
    async fn deliver_losing_spawn_input_saturated_channel_still_delivers_via_retry() {
        let registry = Arc::new(SessionRegistry::new());
        let publisher = Publisher::noop();
        let address = SessionAddress::from("external-discord-race-full");
        let winner = conversation_winner_info(&address, "run-winner");
        let mut rx = registry
            .register(winner.clone(), CancellationToken::new())
            .expect("the winning run registers first");

        for _ in 0..crate::registry::INTERRUPT_CHANNEL_CAPACITY {
            assert!(matches!(
                registry.deliver(
                    &address,
                    Interrupt::UserMessage(sample_inbound_message("filler"))
                ),
                DeliverOutcome::Delivered
            ));
        }

        let losing_info = SessionInfo {
            run_id: "run-loser".to_string(),
            ..winner
        };
        let config = SubAgentConfig {
            prompt: "can anyone see this?".to_string(),
            context: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            hop_count: 0,
            sender: None,
            inbound: Some(sample_inbound_message("can anyone see this?")),
            images: Vec::new(),
            carried_history: Vec::new(),
            delivered_messages: Vec::new(),
            delivered_chat_messages: Vec::new(),
        };
        let register_error = crate::registry::RegisterError {
            address: address.clone(),
        };
        deliver_losing_spawn_input(
            race_guard_sinks(&registry, &publisher, &test_store()),
            &losing_info,
            &config,
            &register_error,
        );

        wait::next_matching(
            "the losing spawn's user message to be delivered, never dropped",
            &mut rx,
            |interrupt| {
                matches!(
                    interrupt,
                    Interrupt::UserMessage(m) if m.content == "can anyone see this?"
                )
            },
        )
        .await;
    }

    #[tokio::test]
    async fn deliver_losing_spawn_input_republishes_a_spawn_request_once_its_target_finishes() {
        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let registry = Arc::new(SessionRegistry::new());
        let address = SessionAddress::from("external-discord-race-notlive");
        let winner = conversation_winner_info(&address, "run-winner");
        registry
            .register(winner.clone(), CancellationToken::new())
            .expect("the winning run registers first");
        registry.remove(&address, "run-winner");

        let mut spawns: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> = bus_handle
            .subscribe(residuum_bus::topics::Background)
            .await
            .unwrap();

        let losing_info = SessionInfo {
            run_id: "run-loser".to_string(),
            ..winner
        };
        let config = SubAgentConfig {
            prompt: "can anyone see this?".to_string(),
            context: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            hop_count: 0,
            sender: None,
            inbound: Some(sample_inbound_message("can anyone see this?")),
            images: Vec::new(),
            carried_history: Vec::new(),
            delivered_messages: Vec::new(),
            delivered_chat_messages: Vec::new(),
        };
        let register_error = crate::registry::RegisterError {
            address: address.clone(),
        };
        deliver_losing_spawn_input(
            race_guard_sinks(&registry, &publisher, &test_store()),
            &losing_info,
            &config,
            &register_error,
        );

        let republished = next_event("a fresh spawn request to be republished", &mut spawns).await;
        assert_eq!(republished.address, address);
        assert_eq!(
            republished.inbound.map(|m| m.content),
            Some("can anyone see this?".to_string()),
            "the republished request must still carry the original user message"
        );
    }

    #[tokio::test]
    async fn spawn_records_spawner_and_depth_from_the_request() {
        let (runtime, _sub) = test_runtime(3).await;
        let mut request = sample_request("spawned-child-0001");
        request.spawner = Some(SessionAddress::from("spawned-parent-0001"));
        request.depth = 2;
        runtime.spawn(request, None);

        let info = runtime
            .registry
            .get(&SessionAddress::from("spawned-child-0001"))
            .unwrap();
        assert_eq!(
            info.spawner,
            Some(SessionAddress::from("spawned-parent-0001"))
        );
        assert_eq!(info.depth, 2);
    }

    #[tokio::test]
    async fn spawn_registers_session_immediately() {
        let (runtime, _sub) = test_runtime(3).await;
        runtime.spawn(sample_request("spawned-researcher-0001"), None);

        let live = runtime.registry.list_live();
        assert_eq!(live.len(), 1);
        let session = live.first().unwrap();
        assert_eq!(session.category, SessionCategory::Spawned);
        assert_eq!(session.depth, 1);
        assert_eq!(
            session.spawner,
            Some(SessionAddress::from(MAIN_ADDRESS)),
            "a session spawned by an agent should record main as its spawner in phase 1"
        );
    }

    #[tokio::test]
    async fn spawn_delivers_its_input_into_the_run_that_already_won_the_registration_race() {
        let (runtime, mut sub, bus_handle) = test_runtime_with_bus(3).await;
        let address = SessionAddress::from("spawned-researcher-cas-race");

        let winner = SessionInfo {
            address: address.clone(),
            run_id: "run-winner".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Idle,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "already running".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let mut winner_rx = runtime
            .registry
            .register(winner, CancellationToken::new())
            .unwrap();

        runtime.spawn(sample_request(address.as_ref()), None);

        let delivered = wait::next(
            "the losing spawn's input to be delivered into the run that won",
            &mut winner_rx,
        )
        .await;
        match delivered {
            Interrupt::AgentMessage(m) => assert!(m.content.contains("do the thing")),
            Interrupt::UserMessage(_) | Interrupt::Subconscious(_) | Interrupt::Stopped => {
                panic!("expected an agent message delivered into the winning run")
            }
        }

        bus_barrier(&bus_handle).await;
        assert!(
            sub.drain().is_empty(),
            "a losing registration attempt must never itself produce a published result"
        );
        assert_eq!(
            runtime.registry.get(&address).map(|i| i.run_id),
            Some("run-winner".to_string()),
            "the winner's own entry must be untouched by the losing attempt"
        );
    }

    #[tokio::test]
    async fn successful_run_goes_running_then_idle_then_completes() {
        let (runtime, mut sub) = test_runtime(3).await;
        let address = SessionAddress::from("spawned-researcher-0002");

        let gate = Gate::closed("the turn's model call");
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(make_gated_resources("all done", &gate)),
        );

        gate.until_held(1).await;
        assert_eq!(
            runtime.registry.get(&address).map(|info| info.state),
            Some(SessionState::Running),
            "session should be running while its model call is held"
        );
        gate.release(1);

        let event = next_event("the run's result to be published", &mut sub).await;
        assert_eq!(event.summary, "all done");
        assert!(matches!(event.status, AgentResultStatus::Completed));

        assert!(runtime.registry.get(&address).is_none());

        let transcript_path = event
            .transcript_path
            .expect("completed run should record a transcript path");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.get("state").unwrap(), "completed");
        let transcript = record.get("transcript").unwrap().as_array().unwrap();
        assert!(
            transcript.len() >= 2,
            "transcript should hold at least the user prompt and the assistant reply"
        );
    }

    struct RecordingProvider {
        response: String,
        calls: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl residuum_inference::InferenceProvider for RecordingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.calls
                .lock()
                .unwrap()
                .push(messages.iter().map(|m| m.content.clone()).collect());
            Ok(InferenceResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "recording"
        }
    }

    #[tokio::test]
    async fn a_fork_sends_main_s_conversation_to_the_model_but_keeps_it_out_of_its_transcript() {
        let (runtime, mut sub) = test_runtime(3).await;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut resources = make_resources("unused");
        resources.provider = Box::new(RecordingProvider {
            response: "forked answer".to_string(),
            calls: Arc::clone(&calls),
        });
        let mut request = sample_request("spawned-multitask-0001");
        request.subagent_config.carried_history = vec![
            Message::user("what did we decide about the cabin?"),
            Message::assistant("we're booking it for june", None),
        ];

        runtime.spawn(request, Some(resources));
        let event = next_event("the fork's result to be published", &mut sub).await;
        assert_eq!(event.summary, "forked answer");

        let seen = calls.lock().unwrap().first().cloned().unwrap();
        let carried_at = seen
            .iter()
            .position(|m| m == "what did we decide about the cabin?")
            .expect("the model sees main's conversation");
        let task_at = seen
            .iter()
            .position(|m| m.contains("do the thing"))
            .expect("the model sees the task");
        assert!(
            carried_at < task_at,
            "main's conversation comes before the task: {seen:?}"
        );

        let record: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(event.transcript_path.unwrap())
                .await
                .unwrap(),
        )
        .unwrap();
        let transcript = record.get("transcript").unwrap().to_string();
        assert!(transcript.contains("do the thing"), "{transcript}");
        assert!(transcript.contains("forked answer"), "{transcript}");
        assert!(
            !transcript.contains("cabin") && !transcript.contains("june"),
            "main's conversation stays out of the fork's own transcript: {transcript}"
        );
    }

    struct CacheHintsRecorder {
        seen: Arc<std::sync::Mutex<Vec<residuum_inference::CacheHints>>>,
    }

    #[async_trait]
    impl residuum_inference::InferenceProvider for CacheHintsRecorder {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.seen.lock().unwrap().push(options.cache.clone());
            Ok(InferenceResponse::new("done".to_string(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "cache-hints-recorder"
        }
    }

    #[tokio::test]
    async fn a_session_s_calls_are_keyed_to_its_run_with_short_retention() {
        let (runtime, mut sub) = test_runtime(3).await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut resources = make_resources("unused");
        resources.provider = Box::new(CacheHintsRecorder {
            seen: Arc::clone(&seen),
        });

        runtime.spawn(sample_request("spawned-cache-0001"), Some(resources));
        let event = next_event("the session's result to be published", &mut sub).await;

        let seen = seen.lock().unwrap().clone();
        assert!(!seen.is_empty(), "the session called the model");
        for hints in seen {
            assert_eq!(
                hints,
                residuum_inference::CacheHints {
                    conversation_key: Some(event.run_id.clone()),
                    retention: residuum_inference::CacheRetention::Short,
                }
            );
        }
    }

    #[tokio::test]
    async fn missing_resources_produces_failed_result() {
        let (runtime, mut sub) = test_runtime(3).await;
        runtime.spawn(sample_request("spawned-researcher-0003"), None);

        let event = next_event("the run's result to be published", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Failed { .. }));
    }

    #[tokio::test]
    async fn failed_turn_auto_reports_through_tracing_service_when_enabled() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bug-report"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "public_id": "RR-SESSION-FAILURE",
                "submitted_at": "2026-04-16T14:23:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (_, span_handle) = residuum_core::util::telemetry::SpanBufferLayer::new(
            &residuum_core::util::telemetry::SpanBufferConfig::default(),
        );
        let tracing_service = Arc::new(TracingService::new(
            residuum_config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..residuum_config::TracingConfig::default()
            },
            span_handle,
        ));

        let (runtime, mut sub) = test_runtime_with_tracing_service(3, tracing_service).await;
        runtime.spawn(sample_request("spawned-researcher-report1"), None);

        let event = next_event("the run's result to be published", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Failed { .. }));
    }

    #[tokio::test]
    async fn completed_turn_never_auto_reports_even_when_enabled() {
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

        let (_, span_handle) = residuum_core::util::telemetry::SpanBufferLayer::new(
            &residuum_core::util::telemetry::SpanBufferConfig::default(),
        );
        let tracing_service = Arc::new(TracingService::new(
            residuum_config::TracingConfig {
                feedback_endpoint: server.uri(),
                auto_error_reporting: true,
                ..residuum_config::TracingConfig::default()
            },
            span_handle,
        ));

        let (runtime, mut sub) = test_runtime_with_tracing_service(3, tracing_service).await;
        runtime.spawn(
            sample_request("spawned-researcher-report2"),
            Some(make_resources("all good")),
        );

        let event = next_event("the run's result to be published", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Completed));
    }

    async fn test_runtime_with_bus(
        max_concurrent: usize,
    ) -> (
        SessionRuntime,
        residuum_bus::Subscriber<AgentResultEvent>,
        residuum_bus::BusHandle,
    ) {
        let bus_handle = residuum_bus::spawn_broker();
        let sub = bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let idle_timeouts = IdleTimeouts {
            scheduled: Duration::from_millis(20),
            spawned: Duration::from_millis(20),
            external: Duration::from_millis(20),
            artifact: Duration::from_millis(20),
        };
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(max_concurrent)),
            idle_timeouts,
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );
        (runtime, sub, bus_handle)
    }

    #[tokio::test]
    async fn a_failed_turn_still_relays_a_status_line_to_the_spawner() {
        let (runtime, mut sub, bus_handle) = test_runtime_with_bus(3).await;
        let mut main_sub: residuum_bus::Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();
        runtime.spawn(sample_request("spawned-researcher-fail1"), None);

        let event = next_event("the run's result to be published", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Failed { .. }));

        let relayed = next_event("a failed turn's relay to its spawner", &mut main_sub).await;
        assert!(
            relayed.content.contains("status: failed"),
            "the relay must say clearly that the turn failed, got: {}",
            relayed.content
        );
        assert_eq!(
            relayed
                .origin
                .agent_sender
                .as_deref()
                .and_then(|sender| sender.run_id.as_deref()),
            Some(event.run_id.as_str()),
            "the relay names the run that sent it"
        );
    }

    #[tokio::test]
    async fn an_artifact_session_streams_its_output_but_never_reaches_main_or_the_inbox() {
        let (runtime, mut sub, bus_handle) = test_runtime_with_bus(3).await;
        let mut main_sub: residuum_bus::Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();
        let mut inbox_sub: residuum_bus::Subscriber<residuum_bus::NotificationEvent> =
            bus_handle.subscribe(topics::Inbox).await.unwrap();
        let mut session_sub: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let router = residuum_notify::router::spawn_notification_router(
            &bus_handle,
            residuum_bus::EndpointRegistry::from_entries(std::iter::empty()),
            bus_handle.publisher(),
        )
        .await
        .unwrap();

        let address = SessionAddress::from("artifact-wiki-0001");
        let mut request = sample_request(address.as_ref());
        request.trigger = EventTrigger::Artifact("wiki".to_string());
        request.source_label = "artifact:wiki".to_string();
        request.spawner = None;
        runtime.spawn(
            request,
            Some(make_resources(&format!(
                "wrote the page {HEARTBEAT_URGENT}"
            ))),
        );
        let info = runtime.registry.get(&address).unwrap();
        assert_eq!(info.category, SessionCategory::Artifact);
        assert_eq!(info.spawner, None);

        let event = next_event("the artifact run's result to be published", &mut sub).await;
        assert!(matches!(event.source, EventTrigger::Artifact(ref name) if name == "wiki"));
        assert!(matches!(event.status, AgentResultStatus::Completed));

        bus_barrier(&bus_handle).await;
        let mut streamed = None;
        for e in session_sub.drain() {
            if let SessionEventKind::Response { content, .. } = e.kind {
                streamed = Some(content);
            }
        }
        assert!(
            streamed.is_some_and(|c| c.contains("wrote the page")),
            "the turn's output is published on the session stream"
        );

        assert!(
            main_sub.drain().is_empty(),
            "an artifact session's output must never be relayed to main"
        );
        assert!(
            inbox_sub.drain().is_empty(),
            "an artifact session's completion must not be routed to the inbox"
        );
        router.abort();
    }

    #[tokio::test]
    async fn a_cancelled_turn_still_relays_a_status_line_to_the_spawner() {
        let (runtime, mut sub, bus_handle) = test_runtime_with_bus(0).await;
        let mut main_sub: residuum_bus::Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-cancel1");
        runtime.spawn(sample_request(address.as_ref()), None);
        assert!(runtime.registry.stop(&address));

        let event = next_event("the run's result to be published", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Cancelled));

        let relayed = next_event("a cancelled turn's relay to its spawner", &mut main_sub).await;
        assert!(
            relayed.content.contains("status: cancelled"),
            "the relay must say clearly that the turn was cancelled, got: {}",
            relayed.content
        );
    }

    #[tokio::test]
    async fn relay_to_an_unreachable_spawner_is_treated_as_a_failure_not_silent_success() {
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        );

        let info = SessionInfo {
            address: SessionAddress::from("spawned-researcher-unk-spawner"),
            run_id: "run-unk-spawner".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Completing,
            spawner: Some(SessionAddress::from("spawned-ghost-0000")),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        store.begin_run(&info).await;
        let mut session_events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();

        let publisher = bus_handle.publisher();
        let relay_env = RelayEnv {
            messenger: &messenger,
            store: &store,
            publisher: &publisher,
        };
        let note = relay_result_to_spawner(
            &info,
            &SessionAddress::from("spawned-ghost-0000"),
            "done",
            1,
            &relay_env,
        )
        .await;
        assert!(
            note.is_some(),
            "an unreachable spawner must be reported as a relay failure"
        );

        let event = session_events.recv().await.unwrap().unwrap();
        assert_eq!(event.address, info.address);
        assert_eq!(event.run_id, info.run_id);
        assert!(
            matches!(&event.kind, SessionEventKind::Error { message, .. } if message.contains("Result Relay Failed")),
            "the relay failure must appear as an error on the session's own stream, got {:?}",
            event.kind
        );

        let transcript = store
            .read_incremental_transcript(&info.run_id, info.started_at)
            .await;
        assert!(
            transcript
                .iter()
                .any(|m| m.content.contains("Result Relay Failed")),
            "the failure must be recorded in the run's own transcript, got {transcript:?}"
        );
    }

    struct BlockingProvider;

    #[async_trait]
    impl residuum_inference::InferenceProvider for BlockingProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            std::future::pending().await
        }

        fn model_name(&self) -> &'static str {
            "blocking"
        }
    }

    #[tokio::test]
    async fn stop_during_running_turn_produces_cancelled_result() {
        let (runtime, mut sub) = test_runtime(3).await;
        let address = SessionAddress::from("spawned-researcher-0005");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(BlockingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        wait_for(
            &runtime,
            &address,
            "the session to reach running while blocked on the model call",
            |info| info.state == SessionState::Running,
        )
        .await;

        assert!(runtime.registry.stop(&address));

        let event = next_event("the stopped running turn's result", &mut sub).await;
        assert!(
            matches!(event.status, AgentResultStatus::Cancelled),
            "a session stopped mid-turn must report cancelled, not completed"
        );
    }

    #[tokio::test]
    async fn a_running_session_already_has_a_resume_point() {
        let (runtime, _sub) = test_runtime(3).await;
        let address = SessionAddress::from("spawned-researcher-0009");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(BlockingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        wait_for(
            &runtime,
            &address,
            "the session to reach running while blocked on the model call",
            |info| info.state == SessionState::Running,
        )
        .await;
        let info = runtime.registry.get(&address).unwrap();

        let point = runtime
            .registry
            .resume_point(&address)
            .expect("a crash mid-run must still leave a resume point for the next message");
        assert_eq!(point.previous_run_id, info.run_id);
        assert!(runtime.registry.stop(&address));
    }

    #[tokio::test(start_paused = true)]
    async fn stop_with_a_pending_message_resumes_the_session_to_deliver_it() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut result_sub: residuum_bus::Subscriber<AgentResultEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let mut spawn_sub: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(3)),
            IdleTimeouts {
                scheduled: Duration::from_mins(1),
                spawned: Duration::from_mins(1),
                external: Duration::from_mins(1),
                artifact: Duration::from_mins(1),
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        let address = SessionAddress::from("spawned-researcher-000t");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(BlockingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        wait_for(
            &runtime,
            &address,
            "the session to reach running while blocked on the model call",
            |info| info.state == SessionState::Running,
        )
        .await;

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "don't forget this",
                    0,
                )),
            ),
            crate::registry::DeliverOutcome::Delivered
        );

        assert!(runtime.registry.stop(&address));

        let result_event = next_event("the stopped run's result", &mut result_sub).await;
        assert!(matches!(result_event.status, AgentResultStatus::Cancelled));

        let spawn_event =
            next_event("the pending message's resume as a new run", &mut spawn_sub).await;
        assert_eq!(spawn_event.address, address);
        assert_eq!(
            spawn_event
                .delivered_messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["don't forget this"]
        );
    }

    async fn next_memory_merged(
        sub: &mut residuum_bus::Subscriber<residuum_bus::SessionEvent>,
    ) -> Option<String> {
        loop {
            let event = next_event("a run's memory merge", sub).await;
            if let SessionEventKind::MemoryMerged { episode_id } = event.kind {
                return episode_id;
            }
        }
    }

    const GATED_OBSERVER_RESPONSE: &str = r#"{
        "observations": [
            {"content": "the run finished its work", "timestamp": "2026-02-21T14:30", "visibility": "background"}
        ],
        "narrative": "the session finished its work"
    }"#;

    #[tokio::test]
    async fn a_message_to_a_finished_run_resumes_it_while_its_memory_merge_is_still_blocked() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut sessions: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let mut spawn_sub: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            Arc::clone(&store),
            Arc::new(tokio::sync::Semaphore::new(3)),
            IdleTimeouts {
                scheduled: Duration::from_millis(20),
                spawned: Duration::from_millis(20),
                external: Duration::from_millis(20),
                artifact: Duration::from_millis(20),
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger: Arc::clone(&messenger),
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        let gate = Gate::closed("observer");
        let mut resources = make_resources("all done");
        resources.observer = Arc::new(residuum_memory::observer::Observer::new(
            Box::new(GatedProvider::new(&gate, GATED_OBSERVER_RESPONSE)),
            residuum_memory::observer::ObserverConfig::default(),
        ));
        resources.episode_skip_token_floor = 1;

        let address = SessionAddress::from("spawned-researcher-0009");
        runtime.spawn(sample_request(address.as_ref()), Some(resources));

        let completed = loop {
            let event = next_event("the run's completion", &mut sessions).await;
            if let SessionEventKind::Completed { memory_pending, .. } = event.kind {
                assert!(memory_pending, "the run's merge has started and is held");
                break event;
            }
        };
        let run_id = completed.run_id;
        gate.until_held(1).await;
        let (held, _) = store.read_run(&run_id).await.unwrap().unwrap();
        assert!(
            held.memory_pending,
            "the record shows the merge pending while it is held"
        );
        assert!(
            runtime.registry.get(&address).is_none(),
            "the finished run must leave the registry while its merge is held"
        );

        messenger
            .send(
                address.as_ref(),
                AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "a message for the finished run",
                    0,
                ),
            )
            .await
            .unwrap();
        let resume = next_event("the message's resume as a new run", &mut spawn_sub).await;
        assert_eq!(resume.address, address);
        assert_eq!(
            resume
                .delivered_messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["a message for the finished run"]
        );

        gate.open_all();
        let episode_id = next_memory_merged(&mut sessions).await;
        assert!(episode_id.is_some(), "the merge produced an episode");
        let (record, _) = store.read_run(&run_id).await.unwrap().unwrap();
        assert_eq!(record.episode_id, episode_id);
        assert!(!record.memory_pending);
    }

    struct PanickingProvider;

    #[async_trait]
    impl residuum_inference::InferenceProvider for PanickingProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            panic!("intentional provider panic for test coverage");
        }

        fn model_name(&self) -> &'static str {
            "panicking"
        }
    }

    #[tokio::test]
    async fn panicking_turn_still_publishes_failed_result_and_frees_registry() {
        let (runtime, mut sub) = test_runtime(3).await;
        let address = SessionAddress::from("spawned-researcher-0006");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(PanickingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        let event = next_event("the panicked session's result", &mut sub).await;
        assert!(
            matches!(event.status, AgentResultStatus::Failed { .. }),
            "a panicked turn should be reported as failed, got {:?}",
            event.status
        );
        assert!(
            runtime.registry.get(&address).is_none(),
            "a panicked session must not linger in the registry forever"
        );

        let transcript_path = event
            .transcript_path
            .expect("a panicked run should still record a transcript path");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let transcript = record.get("transcript").unwrap().as_array().unwrap();
        assert!(
            !transcript.is_empty(),
            "the pre-panic transcript (the initial user message, durably \
             sink-appended before the provider call panicked) must be \
             backfilled from the transcript sidecar rather than discarded"
        );
    }

    #[tokio::test]
    async fn a_panicked_turn_still_relays_a_status_line_to_the_spawner() {
        let (runtime, mut sub, bus_handle) = test_runtime_with_bus(3).await;
        let mut main_sub: residuum_bus::Subscriber<MessageEvent> =
            bus_handle.subscribe(topics::UserMessage).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-panic-relay");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(PanickingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        let event = next_event("the panicked session's result", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Failed { .. }));

        let relayed = next_event("a panicked turn's relay to its spawner", &mut main_sub).await;
        assert!(
            relayed.content.contains("status: failed"),
            "the relay must say clearly that the turn failed, got: {}",
            relayed.content
        );
        assert!(
            relayed.content.contains("panicked"),
            "the relay should surface that this was a panic, got: {}",
            relayed.content
        );
        assert_eq!(
            relayed
                .origin
                .agent_sender
                .as_deref()
                .and_then(|sender| sender.run_id.as_deref()),
            Some(event.run_id.as_str()),
            "a panicked run's relay names the run that sent it"
        );
    }

    fn eager_panic_mem_extras(dir: &std::path::Path) -> PanicMemoryExtras {
        let layout = residuum_config::WorkspaceLayout::new(dir);
        let search_index = Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = Arc::new(residuum_memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let observer = Arc::new(residuum_memory::observer::Observer::new(
            Box::new(residuum_memory::testing::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered from a panic", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            residuum_memory::observer::ObserverConfig::default(),
        ));
        PanicMemoryExtras {
            handles: MergeHandles {
                observer,
                merge_writer,
                layout,
                episode_skip_token_floor: 2000,
            },
            hop_counter: residuum_agent::HopCounter::new(0),
        }
    }

    #[tokio::test]
    async fn recover_from_panic_backfills_and_merges_a_substantial_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let info = SessionInfo {
            address: SessionAddress::from("spawned-researcher-0006b"),
            run_id: "run-panic-substantial".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research the thing".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        store.begin_run(&info).await;

        let big_content = "a".repeat(9000);
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[
                    Message::user("investigate the issue"),
                    Message::assistant(big_content, None),
                ],
            )
            .await;

        let mem_extras = eager_panic_mem_extras(dir.path());

        let registry = Arc::new(SessionRegistry::new());
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub: residuum_bus::Subscriber<AgentResultEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let mut sessions_sub: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let publisher = bus_handle.publisher();
        let messenger = Arc::new(AgentMessenger::new(
            Arc::new(SessionRegistry::new()),
            publisher.clone(),
            Arc::new(SessionStore::new(dir.path().to_path_buf())),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let panic_env = PanicRecoveryEnv {
            registry: Arc::clone(&registry),
            store: Arc::clone(&store),
            publisher,
            messenger,
            tz: chrono_tz::UTC,
            merges: MergeTasks::default(),
        };

        recover_from_panic(
            &info,
            &panic_env,
            "intentional panic for test coverage",
            Some(mem_extras),
        )
        .await;

        let event = next_event("panic recovery's result", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Failed { .. }));
        assert!(
            registry.get(&info.address).is_none(),
            "panic recovery must remove the session from the registry"
        );

        let transcript_path = event
            .transcript_path
            .expect("panic recovery should still record a transcript path");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let transcript = record.get("transcript").unwrap().as_array().unwrap();
        assert_eq!(
            transcript.len(),
            2,
            "the pre-panic transcript must be backfilled from the sidecar file"
        );
        let merged = next_memory_merged(&mut sessions_sub).await;
        let merged_contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let merged_record: serde_json::Value = serde_json::from_str(&merged_contents).unwrap();
        assert_eq!(
            merged_record
                .get("episode_id")
                .and_then(serde_json::Value::as_str),
            merged.as_deref(),
            "a substantial pre-panic transcript should still be merged into an episode, got {merged_record:?}"
        );
        assert_eq!(
            merged_record
                .get("memory_pending")
                .and_then(serde_json::Value::as_bool),
            Some(false),
            "the record must no longer be pending once its merge is recorded"
        );
    }

    #[tokio::test]
    async fn shutdown_stops_live_sessions_and_waits_for_completion() {
        let (runtime, mut sub) = test_runtime(3).await;
        let address = SessionAddress::from("spawned-researcher-0007");
        let (layout, observer, merge_writer) = test_memory_extras();
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(BlockingProvider),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        wait_for(
            &runtime,
            &address,
            "the session to reach running before shutdown stops it",
            |info| info.state == SessionState::Running,
        )
        .await;

        wait::guarded(
            "shutdown to wait out the stopped session's teardown",
            runtime.shutdown(wait::HANG_GUARD * 2),
        )
        .await;

        assert!(
            runtime.registry.get(&address).is_none(),
            "shutdown should wait for the stopped session to finish completing"
        );
        let event = next_event("the session's result after shutdown", &mut sub).await;
        assert!(matches!(event.status, AgentResultStatus::Cancelled));
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_finished_runs_memory_merge() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut sessions: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let gate = Gate::closed("observer");
        let mut resources = make_resources("all done");
        resources.observer = Arc::new(residuum_memory::observer::Observer::new(
            Box::new(GatedProvider::new(&gate, GATED_OBSERVER_RESPONSE)),
            residuum_memory::observer::ObserverConfig::default(),
        ));
        resources.episode_skip_token_floor = 1;
        let mut request = sample_request("spawned-researcher-drain");
        request.spawner = None;
        runtime.spawn(request, Some(resources));

        let run_id = loop {
            let event = next_event("the run's completion", &mut sessions).await;
            if let SessionEventKind::Completed { .. } = event.kind {
                break event.run_id;
            }
        };
        gate.until_held(1).await;

        let shutdown = runtime.shutdown(wait::HANG_GUARD * 2);
        tokio::pin!(shutdown);
        assert!(
            shutdown.as_mut().now_or_never().is_none(),
            "shutdown must wait while the run's merge is held"
        );

        gate.open_all();
        wait::guarded("shutdown to return once the merge finishes", shutdown).await;
        let (record, _) = runtime.store.read_run(&run_id).await.unwrap().unwrap();
        assert!(
            !record.memory_pending,
            "the merge is recorded before shutdown returns"
        );
        assert!(record.episode_id.is_some(), "the merge produced an episode");
    }

    #[tokio::test]
    async fn shutdown_with_no_live_sessions_returns_immediately() {
        let (runtime, _sub) = test_runtime(3).await;
        wait::guarded(
            "shutdown with nothing to stop to return",
            runtime.shutdown(Duration::from_secs(5)),
        )
        .await;
    }

    #[tokio::test]
    async fn idle_session_releases_its_permit_for_a_new_run() {
        let idle_window = Duration::from_mins(10);
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(1)),
            IdleTimeouts {
                scheduled: idle_window,
                spawned: idle_window,
                external: idle_window,
                artifact: idle_window,
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        let first = SessionAddress::from("spawned-first-0001");
        runtime.spawn(
            sample_request(first.as_ref()),
            Some(make_resources("first done")),
        );

        wait_for(
            &runtime,
            &first,
            "the first session to reach idle",
            |info| info.state == SessionState::Idle,
        )
        .await;

        let second = SessionAddress::from("spawned-second-0002");
        runtime.spawn(
            sample_request(second.as_ref()),
            Some(make_resources("second done")),
        );
        wait_for(
            &runtime,
            &second,
            "the second session to start while the first is idle",
            |info| info.state != SessionState::Forking,
        )
        .await;
        assert_eq!(
            runtime.registry.get(&first).map(|info| info.state),
            Some(SessionState::Idle),
            "the first session must still be idle: the second started without it ending"
        );
    }

    async fn wait_for(
        runtime: &SessionRuntime,
        address: &SessionAddress,
        what: &str,
        pred: impl Fn(&SessionInfo) -> bool,
    ) {
        residuum_core::testing::wait::until_true(what, || {
            runtime
                .registry
                .get(address)
                .is_some_and(|info| pred(&info))
        })
        .await;
    }

    #[test]
    fn idle_timeouts_use_scheduled_timeout_for_webhooks() {
        let timeouts = IdleTimeouts {
            scheduled: Duration::from_mins(2),
            spawned: Duration::from_mins(10),
            external: Duration::from_mins(30),
            artifact: Duration::from_mins(30),
        };
        assert_eq!(
            timeouts.for_trigger(&EventTrigger::Webhook("gh".into())),
            Duration::from_mins(2),
            "a webhook call is one-shot, so it uses the scheduled timeout"
        );
        assert_eq!(
            timeouts.for_trigger(&EventTrigger::Pulse),
            Duration::from_mins(2)
        );
        assert_eq!(
            timeouts.for_trigger(&EventTrigger::Agent),
            Duration::from_mins(10)
        );
    }

    #[test]
    fn idle_timeouts_give_artifact_sessions_their_own_setting() {
        let timeouts = IdleTimeouts::from(&BackgroundConfig {
            idle_timeout_artifact: Duration::from_mins(7),
            ..BackgroundConfig::default()
        });
        assert_eq!(
            timeouts.for_trigger(&EventTrigger::Artifact("wiki".into())),
            Duration::from_mins(7)
        );
        assert_eq!(
            IdleTimeouts::from(&BackgroundConfig::default())
                .for_trigger(&EventTrigger::Artifact("wiki".into())),
            Duration::from_mins(10),
            "artifact sessions default to a 10 minute idle timeout"
        );
    }

    #[test]
    fn build_result_event_silences_only_pulse_heartbeat_ok() {
        let info = SessionInfo {
            address: SessionAddress::from("scheduled-check-0001"),
            run_id: "run-1".to_string(),
            category: SessionCategory::Scheduled,
            trigger: EventTrigger::Pulse,
            source_label: "pulse:check".to_string(),
            state: SessionState::Completing,
            spawner: None,
            depth: 1,
            purpose: "check things".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let event = build_result_event(
            &info,
            AgentResultStatus::Completed,
            HEARTBEAT_OK.to_string(),
            None,
            chrono_tz::UTC,
        );
        assert_eq!(event.disposition, ResultDisposition::Silent);

        let mut action_info = info.clone();
        action_info.trigger = EventTrigger::Action;
        let action_event = build_result_event(
            &action_info,
            AgentResultStatus::Completed,
            HEARTBEAT_OK.to_string(),
            None,
            chrono_tz::UTC,
        );
        assert_eq!(
            action_event.disposition,
            ResultDisposition::Normal,
            "HEARTBEAT_OK only silences pulse-triggered runs"
        );
    }

    #[test]
    fn build_result_event_does_not_silence_a_summary_merely_mentioning_heartbeat_ok() {
        let info = SessionInfo {
            address: SessionAddress::from("scheduled-check-0001"),
            run_id: "run-1".to_string(),
            category: SessionCategory::Scheduled,
            trigger: EventTrigger::Pulse,
            source_label: "pulse:check".to_string(),
            state: SessionState::Completing,
            spawner: None,
            depth: 1,
            purpose: "check things".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let summary = "Found something worth flagging. The instruction to omit \
             HEARTBEAT_OK was honored, so this note does not end with it."
            .to_string();
        let event = build_result_event(
            &info,
            AgentResultStatus::Completed,
            summary,
            None,
            chrono_tz::UTC,
        );
        assert_eq!(
            event.disposition,
            ResultDisposition::Normal,
            "a summary that merely mentions HEARTBEAT_OK mid-text must not be silenced"
        );
    }

    #[test]
    fn build_result_event_tags_session_address_and_run_id() {
        let info = SessionInfo {
            address: SessionAddress::from("spawned-researcher-0007"),
            run_id: "run-7".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Completing,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let event = build_result_event(
            &info,
            AgentResultStatus::Completed,
            "found it".to_string(),
            None,
            chrono_tz::UTC,
        );
        assert_eq!(event.session_address, info.address);
        assert_eq!(event.run_id, "run-7");
    }

    #[tokio::test]
    async fn wait_idle_treats_a_message_racing_the_timeout_as_a_wake() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.try_send(Interrupt::AgentMessage(AgentMessageEvent::new(
            SessionAddress::from(MAIN_ADDRESS),
            "main",
            "just in time",
            0,
        )))
        .unwrap();

        let stop_token = CancellationToken::new();
        match wait_idle(&stop_token, Duration::ZERO, &mut rx).await {
            IdleOutcome::Woken(msg) => assert_eq!(msg.content, "just in time"),
            IdleOutcome::TimedOut => panic!(
                "a message already queued when the timeout fired must still wake the session"
            ),
            IdleOutcome::Stopped => panic!("stop token was never cancelled"),
            IdleOutcome::WokenExternal(_) => {
                panic!("this test only ever queues an agent message")
            }
        }
    }

    #[tokio::test]
    async fn wait_idle_treats_a_conversation_message_racing_the_timeout_as_a_wake() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.try_send(Interrupt::UserMessage(sample_inbound_message(
            "still here?",
        )))
        .unwrap();

        let stop_token = CancellationToken::new();
        match wait_idle(&stop_token, Duration::ZERO, &mut rx).await {
            IdleOutcome::WokenExternal(msg) => assert_eq!(msg.content, "still here?"),
            IdleOutcome::TimedOut => panic!(
                "a message already queued when the timeout fired must still wake the session"
            ),
            IdleOutcome::Stopped => panic!("stop token was never cancelled"),
            IdleOutcome::Woken(_) => panic!("this test only ever queues a conversation message"),
        }
    }

    #[tokio::test]
    async fn wait_idle_times_out_when_the_channel_is_genuinely_empty() {
        let (_tx, mut rx) = mpsc::channel(4);
        let stop_token = CancellationToken::new();
        assert!(matches!(
            wait_idle(&stop_token, Duration::from_millis(5), &mut rx).await,
            IdleOutcome::TimedOut
        ));
    }

    #[tokio::test]
    async fn idle_session_wakes_for_agent_message_and_runs_a_second_turn() {
        let (runtime, mut sub, bus_handle) = test_runtime_held_idle().await;
        let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-0008");
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(make_sequenced_resources(vec!["first done", "second done"])),
        );

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after its first turn",
        )
        .await;

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "any updates?",
                    0,
                )),
            ),
            crate::registry::DeliverOutcome::Delivered,
            "the idle session should still be live in the registry"
        );

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after the woken second turn",
        )
        .await;
        assert!(
            runtime.registry.stop(&address),
            "the session should still be live after its second turn"
        );

        let event = next_event("the run to complete", &mut sub).await;
        assert_eq!(
            event.summary, "second done",
            "the run should complete with its second turn's own summary"
        );

        let transcript_path = event
            .transcript_path
            .expect("completed run should have a transcript");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let transcript = record.get("transcript").unwrap().as_array().unwrap();
        let joined: String = transcript
            .iter()
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("first done"),
            "transcript should keep the first turn's output: {joined}"
        );
        assert!(
            joined.contains("any updates?"),
            "transcript should record the delivered agent message: {joined}"
        );
        assert!(
            joined.contains("second done"),
            "transcript should include the second turn's output: {joined}"
        );

        wait::until_true("the run to leave the registry", || {
            runtime.registry.get(&address).is_none()
        })
        .await;
        bus_barrier(&bus_handle).await;
        assert!(
            sub.drain().is_empty(),
            "a single message-driven wake should not produce a second run"
        );
    }

    #[tokio::test]
    async fn idle_conversation_session_wakes_for_a_new_message_and_runs_a_second_turn() {
        let (runtime, mut sub, bus_handle) = test_runtime_held_idle().await;
        let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("external-discord-idle-wake");
        runtime.spawn(
            sample_conversation_request(address.as_ref()),
            Some(make_sequenced_resources(vec!["first done", "second done"])),
        );

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after its first turn",
        )
        .await;

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::UserMessage(sample_inbound_message("any updates?")),
            ),
            crate::registry::DeliverOutcome::Delivered,
            "the idle session should still be live in the registry"
        );

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after the woken second turn",
        )
        .await;
        assert!(
            runtime.registry.stop(&address),
            "the session should still be live after its second turn"
        );

        let event = next_event("the run to complete", &mut sub).await;
        assert_eq!(
            event.summary, "second done",
            "the run should complete with its second turn's own summary"
        );
    }

    async fn conversation_output_runtime() -> (
        SessionRuntime,
        residuum_bus::Subscriber<residuum_bus::SessionResponseEvent>,
    ) {
        let bus_handle = residuum_bus::spawn_broker();
        let session_output: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> =
            bus_handle
                .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
                    "discord",
                )))
                .await
                .unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(3)),
            IdleTimeouts {
                scheduled: Duration::from_millis(50),
                spawned: Duration::from_millis(50),
                external: Duration::from_millis(50),
                artifact: Duration::from_millis(50),
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        (runtime, session_output)
    }

    #[tokio::test]
    async fn a_conversation_session_s_turns_post_nothing_to_its_chat_on_their_own() {
        let log = residuum_core::testing::log::EventLog::default();
        let _capture = log.capture();
        let (runtime, _results, bus_handle) = test_runtime_held_idle().await;
        let mut chat: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> = bus_handle
            .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
                "discord",
            )))
            .await
            .unwrap();
        let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("external-discord-teammate-reply");
        runtime.spawn(
            sample_conversation_request(address.as_ref()),
            Some(make_sequenced_resources(vec![
                "I'll ask Ivan",
                "relayed Ivan's answer to the group",
            ])),
        );
        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after the group member's turn",
        )
        .await;

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(AgentMessageEvent::new(
                    SessionAddress::from("agent:ivan"),
                    residuum_bus::session::TEAMMATE_SENDER_CATEGORY.to_string(),
                    "Here's your answer.",
                    1,
                )),
            ),
            crate::registry::DeliverOutcome::Delivered,
        );
        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after the teammate's turn",
        )
        .await;
        bus_barrier(&bus_handle).await;
        let posted: Vec<String> = chat.drain().into_iter().map(|e| e.content).collect();
        assert!(
            posted.is_empty(),
            "neither the person's turn nor the teammate's posts its final text: {posted:?}"
        );
        let unposted = log
            .events()
            .into_iter()
            .filter(|e| {
                e.level == tracing::Level::INFO
                    && e.text.contains("without a send_message to its chat")
            })
            .count();
        assert_eq!(
            unposted, 2,
            "each turn that told its chat nothing is logged"
        );
        assert!(runtime.registry.stop(&address));
    }

    #[test]
    fn a_chat_session_is_told_only_send_message_reaches_its_chat() {
        let mut info = sample_live_info_for_note();
        let note = chat_session_note(&info).expect("a chat conversation session gets the note");
        assert!(
            note.contains(r#"endpoint="discord" and conversation="chan-1""#),
            "{note}"
        );

        info.conversation_target = Some(ConversationTarget {
            endpoint: A2A_ENDPOINT.to_string(),
            conversation_id: "caller/ctx".to_string(),
        });
        assert_eq!(
            chat_session_note(&info),
            None,
            "an A2A caller hears the session's responses, so it gets no note"
        );

        info.category = SessionCategory::Spawned;
        info.conversation_target = Some(ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "owner-dm".to_string(),
        });
        assert_eq!(
            chat_session_note(&info),
            None,
            "a /multitask fork posts its replies itself"
        );
    }

    #[test]
    fn the_chat_note_goes_ahead_of_the_kickoff_s_own_context() {
        let config = sample_conversation_request("external-discord-note").subagent_config;
        let mut with_context = config.clone();
        with_context.context = Some("earlier chatter".to_string());
        let TurnKickoff::Initial { context, .. } =
            initial_kickoff(with_context, Some("[note]".to_string()))
        else {
            panic!("the first turn is an initial kickoff");
        };
        assert_eq!(context.as_deref(), Some("[note]\n\nearlier chatter"));
        let TurnKickoff::Initial {
            context: without_note,
            ..
        } = initial_kickoff(config, None)
        else {
            panic!("the first turn is an initial kickoff");
        };
        assert_eq!(without_note, None);
    }

    fn sample_live_info_for_note() -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from("external-discord-note"),
            run_id: "run-note".to_string(),
            category: SessionCategory::External,
            trigger: EventTrigger::Conversation,
            source_label: "discord:#builds".to_string(),
            state: SessionState::Running,
            spawner: None,
            depth: 1,
            purpose: "chat".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: Some(ConversationTarget {
                endpoint: "discord".to_string(),
                conversation_id: "chan-1".to_string(),
            }),
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    #[tokio::test]
    async fn a_fork_posting_into_the_owner_s_chat_names_itself() {
        let (runtime, mut session_output) = conversation_output_runtime().await;
        let mut request = sample_request("spawned-multitask-0001");
        request.spawner = None;
        request.conversation_target = Some(ConversationTarget {
            endpoint: "discord".to_string(),
            conversation_id: "owner-dm".to_string(),
        });

        runtime.spawn(request, Some(make_resources("paint is $40 a gallon")));

        let event = next_event("the fork's reply to be posted", &mut session_output).await;
        assert_eq!(event.conversation_id, "owner-dm");
        assert_eq!(
            event.content, "spawned-multitask-0001:\npaint is $40 a gallon",
            "a fork's reply shares main's chat, so it says whose it is"
        );
    }

    #[tokio::test]
    async fn non_conversation_session_publishes_no_session_response_event() {
        let bus_handle = residuum_bus::spawn_broker();
        let mut session_output: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> =
            bus_handle
                .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
                    "discord",
                )))
                .await
                .unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(3)),
            IdleTimeouts {
                scheduled: Duration::from_millis(50),
                spawned: Duration::from_millis(50),
                external: Duration::from_millis(50),
                artifact: Duration::from_millis(50),
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        let mut results: residuum_bus::Subscriber<AgentResultEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-no-output");
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(make_resources("found the answer")),
        );

        next_event("the spawned session's result", &mut results).await;
        bus_barrier(&bus_handle).await;
        assert!(
            session_output.drain().is_empty(),
            "a spawned session with no conversation target must never publish session output"
        );
    }

    fn make_sequenced_resources_with_eager_observer(
        responses: Vec<&str>,
    ) -> (SubAgentResources, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
        let dir = tempfile::tempdir().unwrap();
        let layout = residuum_config::WorkspaceLayout::new(dir.path()).with_tempdir_guard(dir);
        let search_index = Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = Arc::new(residuum_memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        ));
        let observer_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = Arc::new(residuum_memory::observer::Observer::new(
            Box::new(RecordingProvider {
                response: r#"{"observations": [{"content": "staged a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#.to_string(),
                calls: Arc::clone(&observer_calls),
            }),
            residuum_memory::observer::ObserverConfig {
                threshold_tokens: 1,
                force_threshold_tokens: 1,
                ..residuum_memory::observer::ObserverConfig::default()
            },
        ));
        let resources = SubAgentResources {
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
            provider: Box::new(SequencedProvider::new(responses)),
            tools: residuum_agent::ToolRegistry::new(),
            mcp_registry: McpRegistry::new_shared(),
            skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout: layout.clone(),
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
            hop_counter: residuum_agent::HopCounter::new(0),
        };
        (resources, observer_calls)
    }

    fn take_observer_calls(calls: &std::sync::Mutex<Vec<Vec<String>>>) -> Vec<String> {
        std::mem::take(&mut *calls.lock().unwrap())
            .into_iter()
            .map(|texts| texts.join("\n"))
            .collect()
    }

    #[tokio::test]
    async fn staging_runs_after_every_turn_not_just_once() {
        let (runtime, mut sub, bus_handle) = test_runtime_held_idle().await;
        let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-0009");
        let (resources, observer_calls) = make_sequenced_resources_with_eager_observer(vec![
            "first turn done",
            "second turn done",
        ]);
        runtime.spawn(sample_request(address.as_ref()), Some(resources));

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after its first turn",
        )
        .await;

        let first_turn = take_observer_calls(&observer_calls);
        assert!(
            first_turn.iter().any(|call| call.contains("do the thing"))
                && first_turn
                    .iter()
                    .any(|call| call.contains("first turn done")),
            "the first turn's messages should be staged once it goes idle, got {first_turn:?}"
        );
        assert!(
            first_turn
                .iter()
                .all(|call| call.contains("do the thing") || call.contains("first turn done")),
            "staging after the first turn should send only the first turn's messages, got {first_turn:?}"
        );

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(AgentMessageEvent::new(
                    SessionAddress::from(MAIN_ADDRESS),
                    "main",
                    "keep going",
                    0,
                )),
            ),
            crate::registry::DeliverOutcome::Delivered
        );

        next_idle(
            &mut session_events,
            &address,
            "the session to go idle after the woken second turn",
        )
        .await;

        let second_turn = take_observer_calls(&observer_calls);
        assert!(
            second_turn.iter().any(|call| call.contains("keep going"))
                && second_turn
                    .iter()
                    .any(|call| call.contains("second turn done")),
            "the second turn's messages should be staged once it goes idle, got {second_turn:?}"
        );
        assert!(
            second_turn
                .iter()
                .all(|call| !call.contains("do the thing") && !call.contains("first turn done")),
            "staging the second turn should not resend the first turn's messages, got {second_turn:?}"
        );

        assert!(
            runtime.registry.stop(&address),
            "the session should still be live after its second turn"
        );

        let event = next_event("the run to complete", &mut sub).await;
        assert_eq!(event.summary, "second turn done");
    }

    struct ToolThenAnswerProvider {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl residuum_inference::InferenceProvider for ToolThenAnswerProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            let idx = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if idx == 0 {
                Ok(InferenceResponse::new(
                    "checking".to_string(),
                    vec![residuum_inference::ToolCall {
                        id: "tc-1".to_string(),
                        name: "no_such_tool".to_string(),
                        arguments: serde_json::json!({"q": 1}),
                        server: None,
                    }],
                ))
            } else {
                Ok(InferenceResponse::new("all done".to_string(), vec![]))
            }
        }

        fn model_name(&self) -> &'static str {
            "tool-then-answer"
        }
    }

    async fn collect_until_completed(
        sub: &mut residuum_bus::Subscriber<residuum_bus::SessionEvent>,
    ) -> Vec<residuum_bus::SessionEvent> {
        let mut events = Vec::new();
        loop {
            let event = next_event("the run to complete", sub).await;
            let done = matches!(event.kind, SessionEventKind::Completed { .. });
            events.push(event);
            if done {
                return events;
            }
        }
    }

    fn event_label(kind: &SessionEventKind) -> String {
        match kind {
            SessionEventKind::Started(info) => format!("started:{}", info.state),
            SessionEventKind::StateChanged(state) => format!("state:{state}"),
            SessionEventKind::Completed { status, .. } => format!("completed:{status}"),
            SessionEventKind::MemoryMerged { episode_id } => {
                format!("memory_merged:{}", episode_id.as_deref().unwrap_or("none"))
            }
            SessionEventKind::TurnStarted { .. } => "turn_started".to_string(),
            SessionEventKind::TurnEnded { .. } => "turn_ended".to_string(),
            SessionEventKind::MessageRead { content, .. } => format!("message_read:{content}"),
            SessionEventKind::ToolCall(call) => format!("tool_call:{}", call.name),
            SessionEventKind::ToolResult(result) => {
                format!("tool_result:{}:{}", result.name, result.is_error)
            }
            SessionEventKind::Intermediate { content } => format!("intermediate:{content}"),
            SessionEventKind::TurnUsage { .. } => "turn_usage".to_string(),
            SessionEventKind::Response { content, .. } => format!("response:{content}"),
            SessionEventKind::Error { .. } => "error".to_string(),
        }
    }

    #[tokio::test]
    async fn a_session_whose_fire_was_stopped_ends_before_its_first_turn() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let fire_stop = CancellationToken::new();
        let mut request = sample_request("scheduled-watch-stopped");
        request.spawner = None;
        request.stop = Some(fire_stop.child_token());
        fire_stop.cancel();

        runtime.spawn(request, Some(make_resources("never asked")));

        let collected = collect_until_completed(&mut events).await;
        let labels: Vec<String> = collected.iter().map(|e| event_label(&e.kind)).collect();
        assert!(
            !labels.iter().any(|label| label == "turn_started"),
            "no turn starts, so no model is called: {labels:?}"
        );
        assert_eq!(
            labels.last().map(String::as_str),
            Some("completed:cancelled"),
            "{labels:?}"
        );
    }

    #[tokio::test]
    async fn a_run_publishes_lifecycle_and_turn_events_in_order() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-events");
        let (layout, observer, merge_writer) = test_memory_extras();
        let mut request = sample_request(address.as_ref());
        request.spawner = None;
        runtime.spawn(
            request,
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(ToolThenAnswerProvider {
                    calls: std::sync::atomic::AtomicUsize::new(0),
                }),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        let collected = collect_until_completed(&mut events).await;
        let labels: Vec<String> = collected.iter().map(|e| event_label(&e.kind)).collect();
        assert_eq!(
            labels,
            vec![
                "started:forking",
                "state:queued",
                "state:running",
                "turn_started",
                "intermediate:checking",
                "tool_call:no_such_tool",
                "tool_result:no_such_tool:true",
                "turn_usage",
                "turn_usage",
                "response:all done",
                "turn_ended",
                "state:idle",
                "state:completing",
                "completed:completed",
            ]
        );

        let run_id = &collected.first().unwrap().run_id;
        assert!(
            collected
                .iter()
                .all(|e| &e.run_id == run_id && e.address == address),
            "every event must be tagged with the session's address and run id"
        );
        let turn_ids: Vec<&str> = collected
            .iter()
            .filter_map(|e| match &e.kind {
                SessionEventKind::TurnStarted { turn_id }
                | SessionEventKind::TurnEnded { turn_id }
                | SessionEventKind::MessageRead { turn_id, .. }
                | SessionEventKind::Response { turn_id, .. } => Some(turn_id.as_str()),
                SessionEventKind::Started(_)
                | SessionEventKind::StateChanged(_)
                | SessionEventKind::Completed { .. }
                | SessionEventKind::MemoryMerged { .. }
                | SessionEventKind::ToolCall(_)
                | SessionEventKind::ToolResult(_)
                | SessionEventKind::Intermediate { .. }
                | SessionEventKind::TurnUsage { .. }
                | SessionEventKind::Error { .. } => None,
            })
            .collect();
        let expected_turn_id = format!("{run_id}-t1");
        assert!(
            turn_ids.iter().all(|id| *id == expected_turn_id),
            "turn events should share the turn's id, got {turn_ids:?}"
        );
    }

    fn resources_with_provider(
        provider: Box<dyn residuum_inference::InferenceProvider>,
    ) -> SubAgentResources {
        let (layout, observer, merge_writer) = test_memory_extras();
        SubAgentResources {
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
            provider,
            tools: residuum_agent::ToolRegistry::new(),
            mcp_registry: McpRegistry::new_shared(),
            skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
            hop_counter: residuum_agent::HopCounter::new(0),
        }
    }

    struct GatedToolThenAnswerProvider {
        entry: residuum_core::testing::gate::GateEntry,
        inner: ToolThenAnswerProvider,
    }

    #[async_trait]
    impl residuum_inference::InferenceProvider for GatedToolThenAnswerProvider {
        async fn complete(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
            options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            if self.inner.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                self.entry.pass().await;
            }
            self.inner.complete(messages, tools, options).await
        }

        fn model_name(&self) -> &'static str {
            "gated-tool-then-answer"
        }
    }

    fn owner_message(id: &str, content: &str) -> AgentMessageEvent {
        AgentMessageEvent::new(
            SessionAddress::from(residuum_bus::session::OWNER_ADDRESS),
            "owner",
            content,
            0,
        )
        .with_id(id)
    }

    fn read_of(kind: &SessionEventKind) -> Option<(&str, &str)> {
        if let SessionEventKind::MessageRead { id, turn_id, .. } = kind {
            Some((id.as_str(), turn_id.as_str()))
        } else {
            None
        }
    }

    #[tokio::test]
    async fn a_message_read_mid_turn_is_reported_with_the_running_turn_s_id() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-mid-read");
        let mut request = sample_request(address.as_ref());
        request.spawner = None;
        let gate = Gate::closed("the turn's first model call");
        runtime.spawn(
            request,
            Some(resources_with_provider(Box::new(
                GatedToolThenAnswerProvider {
                    entry: gate.entry(),
                    inner: ToolThenAnswerProvider {
                        calls: std::sync::atomic::AtomicUsize::new(0),
                    },
                },
            ))),
        );

        gate.until_held(1).await;
        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(owner_message("m-mid", "also check the logs")),
            ),
            crate::registry::DeliverOutcome::Delivered,
        );
        gate.release(1);

        let collected = collect_until_completed(&mut events).await;
        let run_id = &collected.first().unwrap().run_id;
        let labels: Vec<String> = collected.iter().map(|e| event_label(&e.kind)).collect();
        let read_at = labels
            .iter()
            .position(|l| l == "message_read:also check the logs")
            .unwrap_or_else(|| panic!("the read should be reported: {labels:?}"));
        let position = |label: &str| labels.iter().position(|l| l == label);
        assert!(
            position("tool_result:no_such_tool:true") < Some(read_at)
                && Some(read_at) < position("response:all done"),
            "the message is read at the checkpoint after the tool call: {labels:?}"
        );
        let (id, turn_id) = collected
            .get(read_at)
            .and_then(|e| read_of(&e.kind))
            .unwrap();
        assert_eq!(id, "m-mid");
        assert_eq!(
            turn_id,
            format!("{run_id}-t1"),
            "the turn in flight read it"
        );
        assert_eq!(
            labels.iter().filter(|l| *l == "turn_started").count(),
            1,
            "the message joined the running turn rather than starting another: {labels:?}"
        );
    }

    #[tokio::test]
    async fn a_message_that_wakes_an_idle_session_is_read_right_after_its_turn_starts() {
        let (runtime, _results, bus_handle) = test_runtime_held_idle().await;
        let mut events = bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-wake-read");
        runtime.spawn(
            sample_request(address.as_ref()),
            Some(make_sequenced_resources(vec!["first done", "second done"])),
        );
        next_idle(&mut events, &address, "the session to go idle").await;

        assert_eq!(
            runtime.registry.deliver(
                &address,
                Interrupt::AgentMessage(owner_message("m-wake", "one more thing")),
            ),
            crate::registry::DeliverOutcome::Delivered,
        );

        let started = loop {
            let event = next_event("the woken turn to start", &mut events).await;
            if let SessionEventKind::TurnStarted { turn_id } = event.kind {
                break turn_id;
            }
        };
        let event = next_event("the woken turn's read of its message", &mut events).await;
        assert_eq!(
            read_of(&event.kind),
            Some(("m-wake", started.as_str())),
            "the read follows the TurnStarted of the turn the message started: {:?}",
            event.kind
        );
        assert_eq!(started, format!("{}-t2", event.run_id));

        next_idle(&mut events, &address, "the woken turn to go idle").await;
        assert!(runtime.registry.stop(&address));
    }

    #[tokio::test]
    async fn a_resumed_run_reads_the_messages_that_resumed_it_on_its_first_turn() {
        let (runtime, mut results, bus_handle) = test_runtime_with_bus(3).await;
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-resumed-read");
        let mut request = sample_request(address.as_ref());
        request.spawner = None;
        let first = owner_message("m-first", "are you there?");
        let second = AgentMessageEvent::new(
            SessionAddress::from("agent:ivan"),
            residuum_bus::session::TEAMMATE_SENDER_CATEGORY,
            "here's the file",
            1,
        )
        .with_id("m-second");
        request.subagent_config.prompt = String::new();
        request.subagent_config.context = Some("[Resumed session] previous run".to_string());
        request.subagent_config.delivered_messages = vec![first, second];
        runtime.spawn(request, Some(make_resources("resumed")));

        let collected = collect_until_completed(&mut events).await;
        let run_id = &collected.first().unwrap().run_id;
        let labels: Vec<String> = collected.iter().map(|e| event_label(&e.kind)).collect();
        let started_at = labels
            .iter()
            .position(|l| l == "turn_started")
            .unwrap_or_else(|| panic!("the run should start a turn: {labels:?}"));
        let reads: Vec<(&str, &str)> = collected
            .iter()
            .skip(started_at + 1)
            .take(2)
            .filter_map(|e| read_of(&e.kind))
            .collect();
        let turn_id = format!("{run_id}-t1");
        assert_eq!(
            reads,
            vec![
                ("m-first", turn_id.as_str()),
                ("m-second", turn_id.as_str())
            ],
            "both reads follow the first turn's TurnStarted: {labels:?}"
        );

        let result = next_event("the run's result", &mut results).await;
        let transcript_path = result.transcript_path.expect("a transcript is written");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let transcript = record
            .get("transcript")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap();
        let opening: Vec<(Option<&str>, Option<&str>, Option<bool>)> = transcript
            .iter()
            .take(3)
            .map(|entry| {
                (
                    entry.get("id").and_then(|id| id.as_str()),
                    entry
                        .pointer("/agent_sender/address")
                        .and_then(|a| a.as_str()),
                    entry.get("context").and_then(serde_json::Value::as_bool),
                )
            })
            .collect();
        assert_eq!(
            opening,
            vec![
                (None, None, Some(true)),
                (Some("m-first"), None, None),
                (Some("m-second"), Some("agent:ivan"), None)
            ],
            "the run opens with its context, marked as context, then an entry per message \
             with its own id and sender: {transcript:?}"
        );
        assert_eq!(
            transcript
                .first()
                .and_then(|entry| entry.get("content"))
                .and_then(serde_json::Value::as_str),
            Some("[Resumed session] previous run"),
            "the session reads the pointer as it always has"
        );
    }

    #[tokio::test]
    async fn a_run_resumed_by_chat_messages_alone_opens_with_its_pointer_as_its_own_entry() {
        let (runtime, mut results, _bus_handle) = test_runtime_with_bus(3).await;
        let address = SessionAddress::from("external-discord-resumed-by-chat");
        let mut request = sample_conversation_request(address.as_ref());
        request.subagent_config.sender = None;
        request.subagent_config.prompt = "[From: Jane via discord (#builds)] any news?".to_string();
        request.subagent_config.context = Some("[Resumed session] previous run".to_string());
        request.subagent_config.delivered_chat_messages =
            vec![jane_chat_message("m-chat", "any news?")];
        runtime.spawn(request, Some(make_resources("resumed")));

        let result = next_event("the run's result", &mut results).await;
        let transcript_path = result.transcript_path.expect("a transcript is written");
        let contents = tokio::fs::read_to_string(&transcript_path).await.unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let transcript = record
            .get("transcript")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap();
        let opening: Vec<(Option<&str>, Option<&str>, Option<bool>)> = transcript
            .iter()
            .take(2)
            .map(|entry| {
                (
                    entry.get("role").and_then(serde_json::Value::as_str),
                    entry.get("content").and_then(serde_json::Value::as_str),
                    entry.get("context").and_then(serde_json::Value::as_bool),
                )
            })
            .collect();
        let [(_, pointer, pointer_mark), chat] = opening.as_slice() else {
            panic!("the run opens with two entries: {transcript:?}");
        };
        assert!(
            pointer.is_some_and(|text| text.ends_with("[Resumed session] previous run")),
            "the session reads the pointer, after any chat note, in an entry of its own: \
             {transcript:?}"
        );
        assert_eq!(
            *pointer_mark,
            Some(true),
            "the pointer is marked as context"
        );
        assert_eq!(
            *chat,
            (
                Some("user"),
                Some("[From: Jane via discord (#builds)] any news?"),
                None
            ),
            "the chat messages follow as an ordinary entry: {transcript:?}"
        );
    }

    #[tokio::test]
    async fn two_owner_messages_to_a_completing_session_are_each_read_under_their_own_id() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut spawns: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let address = SessionAddress::from("spawned-researcher-two-sends");
        let completing = SessionInfo {
            address: address.clone(),
            run_id: "run-old".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Completing,
            spawner: None,
            depth: MAIN_DEPTH + 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        };
        let _old_rx = runtime
            .registry
            .register(completing, CancellationToken::new())
            .unwrap();
        runtime
            .registry
            .record_resume_point(
                &address,
                ResumePoint {
                    previous_run_id: "run-old".to_string(),
                    trigger: EventTrigger::Agent,
                    source_label: "agent:researcher".to_string(),
                    agent_skill: None,
                    model: residuum_config::ModelSelector::MEDIUM,
                    spawner: None,
                    depth: MAIN_DEPTH + 1,
                    conversation_target: None,
                    recorded_at: Utc::now(),
                },
            )
            .await;
        let dir = tempfile::tempdir().unwrap();
        let messenger = AgentMessenger::new(
            Arc::clone(&runtime.registry),
            bus_handle.publisher(),
            Arc::new(SessionStore::new(dir.path().to_path_buf())),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        );

        for (id, content) in [("m-one", "first thing"), ("m-two", "second thing")] {
            let outcome = messenger
                .send(address.as_ref(), owner_message(id, content))
                .await
                .unwrap();
            assert!(matches!(outcome, DeliveryOutcome::Queued(_)), "{outcome:?}");
        }
        runtime.registry.remove(&address, "run-old");
        let winner = next_event("the first send's resume", &mut spawns).await;
        let loser = next_event("the second send's resume", &mut spawns).await;

        let gate = Gate::closed("the resumed run's first model call");
        runtime.spawn(
            crate::listener::session_spawn_request(winner, None, None),
            Some(make_gated_resources("done", &gate)),
        );
        gate.until_held(1).await;
        crate::listener::deliver_race_guard_content(
            &runtime.registry,
            &bus_handle.publisher(),
            &runtime.store,
            loser,
        )
        .unwrap();
        gate.open_all();

        let collected = collect_until_completed(&mut events).await;
        let mut reads: Vec<(String, String)> = collected
            .iter()
            .filter_map(|event| {
                if let SessionEventKind::MessageRead { id, from, .. } = &event.kind {
                    Some((id.clone(), from.clone()))
                } else {
                    None
                }
            })
            .collect();
        reads.sort();
        let owner = residuum_bus::session::OWNER_ADDRESS.to_string();
        assert_eq!(
            reads,
            vec![
                ("m-one".to_string(), owner.clone()),
                ("m-two".to_string(), owner)
            ],
            "each send is read under its own id, from the owner"
        );
    }

    #[tokio::test]
    async fn stopping_an_idle_session_reports_cancelled() {
        let (runtime, mut results, _bus_handle) = test_runtime_held_idle().await;
        let address = SessionAddress::from("spawned-researcher-idle-stop");
        let (layout, observer, merge_writer) = test_memory_extras();
        let mut request = sample_request(address.as_ref());
        request.spawner = None;
        runtime.spawn(
            request,
            Some(SubAgentResources {
                max_tool_iterations: None,
                repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
                intermediate_delivery: residuum_agent::context::IntermediateDelivery::default(),
                provider: Box::new(ToolThenAnswerProvider {
                    calls: std::sync::atomic::AtomicUsize::new(0),
                }),
                tools: residuum_agent::ToolRegistry::new(),
                mcp_registry: McpRegistry::new_shared(),
                skill_state: SkillState::new_shared(SkillIndex::default(), vec![]),
                identity: IdentityFiles::default(),
                options: CompletionOptions::default(),
                skills_index: None,
                observations: None,
                recent_context: None,
                layout,
                observer,
                merge_writer,
                episode_skip_token_floor: 2000,
                hop_counter: residuum_agent::HopCounter::new(0),
            }),
        );

        wait_for(
            &runtime,
            &address,
            "the session to go idle after its first turn",
            |info| info.state == SessionState::Idle,
        )
        .await;
        assert!(runtime.registry.stop(&address));

        let result = next_event("the stopped session's result", &mut results).await;
        assert!(
            matches!(result.status, AgentResultStatus::Cancelled),
            "an explicit stop while idle is a cancellation, got {:?}",
            result.status
        );
    }

    #[tokio::test]
    async fn a_failed_turn_publishes_an_error_event_and_a_failed_completion() {
        let (runtime, _results, bus_handle) = test_runtime_with_bus(3).await;
        let mut events: residuum_bus::Subscriber<residuum_bus::SessionEvent> =
            bus_handle.subscribe(topics::Sessions).await.unwrap();
        let mut request = sample_request("spawned-researcher-fails");
        request.spawner = None;
        runtime.spawn(request, None);

        let collected = collect_until_completed(&mut events).await;
        let labels: Vec<String> = collected.iter().map(|e| event_label(&e.kind)).collect();
        assert_eq!(
            labels,
            vec![
                "started:forking",
                "state:queued",
                "state:running",
                "turn_started",
                "error",
                "turn_ended",
                "state:idle",
                "state:completing",
                "completed:failed: Something went wrong while the agent was working. Try \
                 again; if it keeps happening, check Residuum's logs for details.",
            ],
            "an unclassified session failure gets a plain-language message, never the raw cause"
        );
    }

    #[tokio::test]
    async fn a_run_waiting_on_the_concurrency_permit_shows_as_queued() {
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_path_buf();
        let store = Arc::new(SessionStore::new(dir_path.clone()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
        ));
        let runtime = SessionRuntime::new(
            registry,
            store,
            Arc::new(tokio::sync::Semaphore::new(1)),
            IdleTimeouts {
                scheduled: Duration::from_secs(5),
                spawned: Duration::from_secs(5),
                external: Duration::from_secs(5),
                artifact: Duration::from_secs(5),
            },
            SessionRuntimeHandles {
                publisher: bus_handle.publisher(),
                tz: chrono_tz::UTC,
                messenger,
                checkpoints: test_checkpoints(&dir_path),
                tracing_service: test_tracing_service(),
                tracing_client_context: test_tracing_client_context(),
            },
        );

        let first = SessionAddress::from("spawned-slow-first-0001");
        let gate = Gate::closed("the first turn's model call");
        runtime.spawn(
            sample_request(first.as_ref()),
            Some(make_gated_resources("first done", &gate)),
        );
        wait_for(
            &runtime,
            &first,
            "the first session to reach running and hold the only permit",
            |info| info.state == SessionState::Running,
        )
        .await;
        gate.until_held(1).await;

        let second = SessionAddress::from("spawned-second-waiting-0002");
        runtime.spawn(
            sample_request(second.as_ref()),
            Some(make_resources("second done")),
        );

        wait_for(
            &runtime,
            &second,
            "the second session to show as queued while the first holds the only concurrency permit",
            |info| info.state == SessionState::Queued,
        )
        .await;

        gate.release(1);
        wait_for(
            &runtime,
            &second,
            "the second session to proceed once the permit is released",
            |info| info.state == SessionState::Running || info.state == SessionState::Idle,
        )
        .await;
    }
}

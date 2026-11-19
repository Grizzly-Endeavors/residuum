//! The A2A [`a2a_server::AgentExecutor`]: turns an inbound A2A message into
//! an `a2a` conversation session and maps that session's lifecycle back onto
//! A2A task states. See `docs/systems-usage/a2a.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2a::{
    A2AError, Artifact, Message as A2aMessage, Part, PartContent, StreamResponse,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use a2a_server::{AgentExecutor, ExecutorContext};
use futures_util::stream::BoxStream;

use residuum_background::messaging::{AgentMessenger, ConversationSpawn};
use residuum_background::registry::{SessionRegistry, conversation_session_address};
use residuum_background::{SessionCloseFlush, SpawnContext};
use residuum_bus::MessageEvent;
use residuum_bus::origin::{A2A_ENDPOINT, ConversationContext, ConversationKind, MessageOrigin};
use residuum_bus::session::SessionState;
use residuum_bus::{
    A2aTaskSignalEvent, A2aTaskSignalState, AgentResultStatus, BusHandle, SessionEvent,
    SessionEventKind, topics,
};
use residuum_config::ModelSelector;
use residuum_inference::{ImageData, MessageAttachment, MessageSender};
use residuum_skills::SharedSkillState;
use residuum_workspace::inbox::attachment::{
    self, AttachmentInfo, MAX_IMAGE_INLINE_SIZE, is_supported_image,
};

use super::auth::CALLER_HEADER;
use super::card::SharedCardState;

type ExecutorSender = tokio::sync::mpsc::Sender<Result<StreamResponse, A2AError>>;

/// Everything the executor needs to deliver a message into a session and
/// follow that session's lifecycle back to an A2A task state.
#[derive(Clone)]
pub struct SessionExecutor {
    messenger: Arc<AgentMessenger>,
    session_registry: Arc<SessionRegistry>,
    bus_handle: BusHandle,
    skill_state: SharedSkillState,
    card_state: SharedCardState,
    agent_inbox_dir: PathBuf,
    tz: chrono_tz::Tz,
    spawn_context: Option<tokio::sync::watch::Receiver<Arc<SpawnContext>>>,
}

impl SessionExecutor {
    /// Create an executor. `agent_inbox_dir` is where non-image inbound
    /// attachments are saved, same as every other interface.
    #[must_use]
    pub fn new(
        messenger: Arc<AgentMessenger>,
        session_registry: Arc<SessionRegistry>,
        bus_handle: BusHandle,
        skill_state: SharedSkillState,
        card_state: SharedCardState,
        agent_inbox_dir: PathBuf,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            messenger,
            session_registry,
            bus_handle,
            skill_state,
            card_state,
            agent_inbox_dir,
            tz,
            spawn_context: None,
        }
    }

    /// Attach the live [`SpawnContext`] receiver so sibling room-close payloads
    /// can flush transcript-only completion runs for silent attendees.
    #[must_use]
    pub fn with_spawn_context(
        mut self,
        spawn_context: Option<tokio::sync::watch::Receiver<Arc<SpawnContext>>>,
    ) -> Self {
        self.spawn_context = spawn_context;
        self
    }

    /// The caller identity the auth layer injected for this request, or an
    /// internal error if it's missing (it always should be present — every
    /// request that reaches the executor already passed the auth layer).
    fn caller_of(params: &a2a_server::ServiceParams) -> Result<String, A2AError> {
        params
            .get(CALLER_HEADER)
            .and_then(|v| v.first())
            .cloned()
            .ok_or_else(|| {
                A2AError::internal("a2a request reached the executor with no caller identity")
            })
    }

    /// The conversation session address a `(caller, context_id)` pair maps
    /// to. Shared with [`super::handler::ResiduumA2aHandler`] so the task
    /// metadata it records matches exactly what the executor delivers into.
    #[must_use]
    pub fn address_for(caller: &str, context_id: &str) -> residuum_bus::SessionAddress {
        conversation_session_address(A2A_ENDPOINT, &format!("{caller}/{context_id}"))
    }

    /// Skill to activate for a new session, from the message's
    /// `metadata.skill` when it names both a card skill id and a workspace
    /// skill. Returns the note to prepend to content when a skill was named
    /// but didn't map to anything runnable.
    async fn resolve_skill(
        &self,
        message: &A2aMessage,
    ) -> (Option<residuum_bus::SkillName>, Option<String>) {
        let Some(requested) = message
            .metadata
            .as_ref()
            .and_then(|m| m.get("skill"))
            .and_then(|v| v.as_str())
        else {
            return (None, None);
        };

        let card_has_skill = self
            .card_state
            .current()
            .skills
            .iter()
            .any(|skill| skill.id == requested);
        if !card_has_skill {
            return (None, Some(format!("[Requested skill: {requested}]")));
        }

        let skill_state = self.skill_state.lock().await;
        if skill_state.index().find_by_name(requested).is_some() {
            (Some(residuum_bus::SkillName::from(requested)), None)
        } else {
            (None, Some(format!("[Requested skill: {requested}]")))
        }
    }
}

impl AgentExecutor for SessionExecutor {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let executor = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        residuum_core::util::spawn_in_span(async move {
            run_execution(executor, ctx, tx).await;
        });
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }))
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let session_registry = Arc::clone(&self.session_registry);
        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();
        let caller = Self::caller_of(&ctx.service_params);
        Box::pin(futures_util::stream::once(async move {
            match caller {
                Ok(caller) => {
                    let address = Self::address_for(&caller, &context_id);
                    session_registry.stop(&address);
                    Ok(status_update(
                        &task_id,
                        &context_id,
                        TaskState::Canceled,
                        None,
                    ))
                }
                Err(e) => Err(e),
            }
        }))
    }
}

/// Build a `StatusUpdate` [`StreamResponse`] for `state`, optionally carrying
/// an agent message with `text`.
fn status_update(
    task_id: &str,
    context_id: &str,
    state: TaskState,
    text: Option<&str>,
) -> StreamResponse {
    let message = text.map(|text| {
        let mut message = A2aMessage::new(a2a::Role::Agent, vec![Part::text(text)]);
        message.task_id = Some(task_id.to_string());
        message.context_id = Some(context_id.to_string());
        message
    });
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.to_string(),
        context_id: context_id.to_string(),
        status: TaskStatus {
            state,
            message,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

/// Build an `ArtifactUpdate` [`StreamResponse`] for one artifact.
fn artifact_update(task_id: &str, context_id: &str, artifact: Artifact) -> StreamResponse {
    StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
        task_id: task_id.to_string(),
        context_id: context_id.to_string(),
        artifact,
        append: None,
        last_chunk: Some(true),
        metadata: None,
    })
}

/// Live subscriptions the executor follows for one execution, all opened
/// before delivery so nothing the session does in response is missed.
struct ExecutionSubscriptions {
    sessions: residuum_bus::Subscriber<SessionEvent>,
    signals: residuum_bus::Subscriber<A2aTaskSignalEvent>,
}

async fn subscribe_before_delivery(
    executor: &SessionExecutor,
) -> Result<ExecutionSubscriptions, A2AError> {
    let sessions = executor
        .bus_handle
        .subscribe(topics::Sessions)
        .await
        .map_err(|e| A2AError::internal(format!("failed to subscribe to session events: {e}")))?;
    let signals = executor
        .bus_handle
        .subscribe(topics::A2aTaskSignal)
        .await
        .map_err(|e| A2AError::internal(format!("failed to subscribe to task signals: {e}")))?;
    Ok(ExecutionSubscriptions { sessions, signals })
}

/// The whole lifecycle of one `execute()` call: subscribe, deliver, then
/// follow the session's activity until it reaches a terminal (or
/// input-required) A2A state.
async fn run_execution(executor: SessionExecutor, ctx: ExecutorContext, tx: ExecutorSender) {
    let task_id = ctx.task_id.clone();
    let context_id = ctx.context_id.clone();

    let caller = match SessionExecutor::caller_of(&ctx.service_params) {
        Ok(caller) => caller,
        Err(e) => {
            tx.send(Err(e)).await.ok();
            return;
        }
    };
    let address = SessionExecutor::address_for(&caller, &context_id);

    let Some(message) = ctx.message.clone() else {
        tx.send(Err(A2AError::internal("execute() called with no message")))
            .await
            .ok();
        return;
    };

    if is_sibling_room_close(&caller, &message) {
        handle_sibling_room_close(&executor, &caller, &context_id, &address, &message).await;
        let update = status_update(&task_id, &context_id, TaskState::Completed, None);
        tx.send(Ok(update)).await.ok();
        return;
    }

    // Subscribe BEFORE delivering the message, so nothing the session does
    // in response can be published and missed before we start listening.
    let mut subs = match subscribe_before_delivery(&executor).await {
        Ok(subs) => subs,
        Err(e) => {
            tx.send(Err(e)).await.ok();
            return;
        }
    };

    if let Err(e) = deliver_inbound(&executor, &address, &caller, &context_id, &message).await {
        tx.send(Err(e)).await.ok();
        return;
    }

    stream_until_terminal(
        &executor,
        &address,
        &task_id,
        &context_id,
        is_room_turn(&message),
        &mut subs,
        &tx,
    )
    .await;
}

/// Whether an inbound A2A message originates from a meeting room turn.
fn is_room_turn(message: &A2aMessage) -> bool {
    message
        .metadata
        .as_ref()
        .and_then(|m| m.get("residuum.room"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

/// Whether an inbound A2A message is a sibling room-close notification that
/// should stop the room session directly without running an LLM turn.
fn is_sibling_room_close(caller: &str, message: &A2aMessage) -> bool {
    caller.starts_with("sibling:")
        && message
            .metadata
            .as_ref()
            .and_then(|m| m.get("residuum.room_close"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

async fn handle_sibling_room_close(
    executor: &SessionExecutor,
    caller: &str,
    context_id: &str,
    address: &residuum_bus::SessionAddress,
    message: &A2aMessage,
) {
    let meta = message.metadata.as_ref();
    let room_id = meta
        .and_then(|m| m.get("residuum.room_id"))
        .and_then(serde_json::Value::as_str);
    let room_name = meta
        .and_then(|m| m.get("residuum.room_name"))
        .and_then(serde_json::Value::as_str)
        .or(room_id)
        .unwrap_or(context_id);
    let minutes_md = meta
        .and_then(|m| m.get("residuum.room_minutes"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let shared_path = meta
        .and_then(|m| m.get("residuum.room_minutes_path"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let has_unseen = meta
        .and_then(|m| m.get("residuum.room_has_unseen"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let (caller_slug, _) = caller_display(caller);

    if let Some(markdown) = minutes_md {
        let title = format!("Meeting minutes: {room_name}");
        let body = match shared_path {
            Some(path) => format!("{markdown}\n\n---\nSaved on `{caller_slug}` at `{path}`."),
            None => markdown.to_owned(),
        };
        let source = format!("room:{}", room_id.unwrap_or(context_id));
        if let Err(err) = residuum_workspace::inbox::add_item(
            &executor.agent_inbox_dir,
            residuum_workspace::inbox::NewInboxItem {
                title: &title,
                body: &body,
                source: &source,
                in_reply_to: None,
                attachments: &[],
            },
            executor.tz,
        )
        .await
        {
            tracing::warn!(
                caller,
                context_id,
                error = %err,
                "failed to deliver sibling meeting minutes to agent inbox"
            );
        }
    }

    let unseen = if has_unseen {
        let text = message
            .parts
            .iter()
            .filter_map(|p| match &p.content {
                PartContent::Text(t) => Some(t.as_str()),
                PartContent::Raw(_) | PartContent::Url(_) | PartContent::Data(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    } else {
        None
    };

    let flush = SessionCloseFlush {
        user_content: unseen,
        assistant_note: format!("[Meeting wrapped up — \"{room_name}\".]"),
        bypass_token_floor: true,
    };
    if let Some(rx) = &executor.spawn_context {
        let ctx = Arc::clone(&*rx.borrow());
        ctx.flush_and_close_conversation(
            address,
            format!("a2a:{caller_slug}"),
            Some(residuum_bus::ConversationTarget {
                endpoint: A2A_ENDPOINT.to_owned(),
                conversation_id: format!("{caller}/{context_id}"),
            }),
            flush,
        );
    } else {
        executor.session_registry.stop_with_flush(address, flush);
    }
}

/// A caller's bare name for display (`laptop` for `sibling:laptop`,
/// `research` for `key:research`), and a note distinguishing the user's own
/// other instance from an external caller-key holder. `caller` itself keeps
/// its fully namespaced form (`key:<name>`/`sibling:<slug>`) everywhere it's
/// used for the session address or task ownership — this is display-only.
fn caller_display(caller: &str) -> (String, Option<&'static str>) {
    if let Some(slug) = caller.strip_prefix("sibling:") {
        (slug.to_string(), Some("your own other Residuum instance"))
    } else if let Some(name) = caller.strip_prefix("key:") {
        (
            name.to_string(),
            Some("an external agent with a caller key"),
        )
    } else {
        (caller.to_string(), None)
    }
}

/// Choose the model selector for an inbound A2A turn: trusted sibling rooms
/// run on the agent's primary `main` persona chain, while all other inbound
/// A2A traffic stays on `medium`.
fn inbound_room_model(
    caller: &str,
    metadata: Option<&HashMap<String, serde_json::Value>>,
) -> ModelSelector {
    let is_room = metadata
        .and_then(|m| m.get("residuum.room"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if caller.starts_with("sibling:") && is_room {
        ModelSelector::MAIN
    } else {
        ModelSelector::MEDIUM
    }
}

/// Build the session's inbound content from the A2A message and deliver it
/// into the conversation session, per the skill-mapping and part-handling
/// rules in `docs/systems-usage/a2a.md`.
async fn deliver_inbound(
    executor: &SessionExecutor,
    address: &residuum_bus::SessionAddress,
    caller: &str,
    context_id: &str,
    message: &A2aMessage,
) -> Result<(), A2AError> {
    let (skill, skill_note) = executor.resolve_skill(message).await;
    let (mut content, images, attachments) =
        build_inbound_content(&message.parts, &executor.agent_inbox_dir, executor.tz).await;
    if let Some(note) = skill_note {
        content = format!("{note}\n{content}");
    }

    let (display_name, location) = caller_display(caller);
    let inbound = MessageEvent {
        id: message.message_id.clone(),
        content,
        origin: MessageOrigin {
            endpoint: A2A_ENDPOINT.to_string(),
            sender: Some(MessageSender {
                name: display_name.clone(),
                id: caller.to_string(),
                interface: A2A_ENDPOINT.to_string(),
                location: location.map(str::to_string),
            }),
            conversation: Some(ConversationContext {
                id: format!("{caller}/{context_id}"),
                kind: ConversationKind::Personal,
                is_owner: false,
            }),
            agent_sender: None,
        },
        timestamp: residuum_core::time::now_local(executor.tz),
        images,
        attachments,
        context: None,
    };
    let spawn = ConversationSpawn {
        source_label: format!("a2a:{display_name}"),
        model: inbound_room_model(caller, message.metadata.as_ref()),
        skill,
    };

    executor
        .messenger
        .deliver_conversation(address, inbound, spawn)
        .await
        .map_err(|e| A2AError::internal(format!("failed to deliver to session: {e}")))?;
    Ok(())
}

/// Identity of the task one [`stream_until_terminal`] call follows, bundled
/// so the per-event helpers below take one argument for it instead of four.
struct StreamCtx<'a> {
    executor: &'a SessionExecutor,
    address: &'a residuum_bus::SessionAddress,
    task_id: &'a str,
    context_id: &'a str,
    is_room_turn: bool,
    tx: &'a ExecutorSender,
}

/// Mutable tracking state across events in [`stream_until_terminal`].
struct StreamState {
    last_final_text: String,
    last_error: Option<String>,
    sent_working: bool,
    // Turns that started while this execution was following the session.
    // A turn already running when the execution began may be the one that
    // signaled the previous task's outcome; its closing text belongs to that
    // task, so only responses from turns started here are relayed.
    own_turns: HashSet<String>,
}

/// Follow the session's activity on the bus until it reaches a terminal (or
/// input-required) A2A state, translating each relevant event into a
/// [`StreamResponse`] sent to `tx`. Returns once the stream should end —
/// either because a terminal outcome was sent, or because a subscription (or
/// the channel to the caller) closed.
async fn stream_until_terminal(
    executor: &SessionExecutor,
    address: &residuum_bus::SessionAddress,
    task_id: &str,
    context_id: &str,
    is_room_turn: bool,
    subs: &mut ExecutionSubscriptions,
    tx: &ExecutorSender,
) {
    let stream = StreamCtx {
        executor,
        address,
        task_id,
        context_id,
        is_room_turn,
        tx,
    };
    let mut state = StreamState {
        last_final_text: String::new(),
        last_error: None,
        sent_working: false,
        own_turns: HashSet::new(),
    };

    loop {
        // Biased, with the session branch first: a task-outcome signal is
        // always published from inside a turn that has already published
        // its own `TurnStarted`/`StateChanged(Running)`, so when both
        // channels have something ready in the same poll, the session
        // event is the earlier one causally and must be drained first —
        // otherwise `tokio::select!`'s default random tie-break can act on
        // the signal first and end the stream before `sent_working` was
        // ever set, skipping the WORKING status entirely.
        tokio::select! {
            biased;
            event = subs.sessions.recv() => {
                let Ok(Some(event)) = event else { return };
                if event.address != *stream.address {
                    continue;
                }
                if handle_session_event(&stream, event.kind, &mut state, &mut subs.signals).await {
                    return;
                }
            }
            signal = subs.signals.recv() => {
                let Ok(Some(signal)) = signal else { return };
                if signal.address != *stream.address {
                    continue;
                }
                emit_signal_outcome(&stream, signal).await;
                return;
            }
        }
    }
}

/// Handle one [`SessionEventKind`] for `stream.address` within
/// [`stream_until_terminal`]'s loop. Returns `true` when the stream should
/// end (a terminal outcome was sent, or the caller's channel closed).
async fn handle_session_event(
    stream: &StreamCtx<'_>,
    kind: SessionEventKind,
    state: &mut StreamState,
    signals: &mut residuum_bus::Subscriber<A2aTaskSignalEvent>,
) -> bool {
    match kind {
        SessionEventKind::TurnStarted { turn_id } => {
            state.own_turns.insert(turn_id);
            // `SessionEvent` is a lossless bus route (`bus::topics::Carries`
            // for `Sessions`), so both this and the `StateChanged(Running)`
            // arm below are guaranteed delivery — neither can be dropped
            // under load. Kept as two independent triggers anyway: they're
            // idempotent (guarded by `sent_working`), `TurnStarted` is the
            // stronger, earlier signal that the session is working, and
            // `own_turns` already depends on this event being delivered.
            if state.sent_working {
                return false;
            }
            state.sent_working = true;
            let update = status_update(stream.task_id, stream.context_id, TaskState::Working, None);
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::Response { turn_id, content } => {
            if !state.own_turns.contains(&turn_id) || content.is_empty() {
                return false;
            }
            state.last_final_text.clone_from(&content);
            state.sent_working = true;
            let update = status_update(
                stream.task_id,
                stream.context_id,
                TaskState::Working,
                Some(&content),
            );
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::Error { message, .. } => {
            if !state.own_turns.is_empty() {
                state.last_error = Some(message);
            }
            false
        }
        SessionEventKind::TurnEnded { turn_id }
            if stream.is_room_turn && state.own_turns.contains(&turn_id) =>
        {
            handle_room_turn_ended(stream, state, signals).await
        }
        SessionEventKind::StateChanged(SessionState::Running) if !state.sent_working => {
            state.sent_working = true;
            let update = status_update(stream.task_id, stream.context_id, TaskState::Working, None);
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::Completed { status, .. } => {
            handle_run_completed(stream, status, &state.last_final_text).await
        }
        SessionEventKind::StateChanged(_)
        | SessionEventKind::Started(_)
        | SessionEventKind::TurnEnded { .. }
        | SessionEventKind::MessageRead { .. }
        | SessionEventKind::ToolCall(_)
        | SessionEventKind::ToolResult(_)
        | SessionEventKind::Intermediate { .. }
        // A2A has no notion of token usage/elapsed time in its task
        // protocol, and this must never reach the agent either way — see
        // `docs/systems-usage/turn-control.md`.
        | SessionEventKind::TurnUsage { .. } => false,
    }
}

/// Complete an inbound A2A room turn when the turn finishes (`TurnEnded`) so
/// external room sessions do not stall waiting for the 30-minute idle timeout.
async fn handle_room_turn_ended(
    stream: &StreamCtx<'_>,
    state: &mut StreamState,
    signals: &mut residuum_bus::Subscriber<A2aTaskSignalEvent>,
) -> bool {
    if let Some(signal) = signals
        .drain()
        .into_iter()
        .find(|s| s.address == *stream.address)
    {
        emit_signal_outcome(stream, signal).await;
        return true;
    }
    let has_live_children = stream
        .executor
        .session_registry
        .list_live()
        .into_iter()
        .any(|s| s.spawner.as_ref() == Some(stream.address));
    if has_live_children {
        return false;
    }
    if let Some(err) = state.last_error.take() {
        let update = status_update(
            stream.task_id,
            stream.context_id,
            TaskState::Failed,
            Some(&err),
        );
        stream.tx.send(Ok(update)).await.ok();
        return true;
    }
    let text = (!state.last_final_text.is_empty()).then_some(state.last_final_text.as_str());
    let update = status_update(
        stream.task_id,
        stream.context_id,
        TaskState::Completed,
        text,
    );
    stream.tx.send(Ok(update)).await.ok();
    true
}

/// Map a session run's completion onto a terminal A2A state, per the "no
/// signal" rules in `docs/systems-usage/a2a.md`. Returns `true` once a
/// terminal outcome has been sent (the stream should end); `false` if the
/// run completed but a live spawned child means the task stays `WORKING`.
async fn handle_run_completed(
    stream: &StreamCtx<'_>,
    status: AgentResultStatus,
    last_final_text: &str,
) -> bool {
    match status {
        AgentResultStatus::Cancelled => {
            let update =
                status_update(stream.task_id, stream.context_id, TaskState::Canceled, None);
            stream.tx.send(Ok(update)).await.ok();
            true
        }
        AgentResultStatus::Failed { error, .. } => {
            let update = status_update(
                stream.task_id,
                stream.context_id,
                TaskState::Failed,
                Some(&error),
            );
            stream.tx.send(Ok(update)).await.ok();
            true
        }
        AgentResultStatus::Completed => {
            let has_live_children = stream
                .executor
                .session_registry
                .list_live()
                .into_iter()
                .any(|s| s.spawner.as_ref() == Some(stream.address));
            if has_live_children {
                // A spawned child is still running; its result relay resumes
                // this session, and the resumed run reappears at the same
                // address — keep streaming.
                return false;
            }
            let text = (!last_final_text.is_empty()).then_some(last_final_text);
            let update = status_update(
                stream.task_id,
                stream.context_id,
                TaskState::Completed,
                text,
            );
            stream.tx.send(Ok(update)).await.ok();
            true
        }
    }
}

/// Map an explicit `a2a_task_update` signal onto the terminal (or
/// input-required) A2A stream events that end this task's execution.
async fn emit_signal_outcome(stream: &StreamCtx<'_>, signal: A2aTaskSignalEvent) {
    for artifact in signal.artifacts {
        let update = artifact_update(stream.task_id, stream.context_id, artifact);
        if stream.tx.send(Ok(update)).await.is_err() {
            return;
        }
    }
    let state = match signal.state {
        A2aTaskSignalState::Completed => TaskState::Completed,
        A2aTaskSignalState::InputRequired => TaskState::InputRequired,
        A2aTaskSignalState::Failed => TaskState::Failed,
    };
    let update = status_update(
        stream.task_id,
        stream.context_id,
        state,
        Some(&signal.message),
    );
    stream.tx.send(Ok(update)).await.ok();
}

/// Turn an inbound A2A message's parts into a session's `content` string and
/// any inline images, per `docs/systems-usage/a2a.md`: text becomes content,
/// image raw parts become `ImageData`, other raw parts are saved to the
/// agent inbox and referenced, `http(s)` url parts are downloaded, and data
/// parts are pretty-printed inline. A failure on any one part never drops
/// the message — it adds the existing failed-attachment line instead.
async fn build_inbound_content(
    parts: &[Part],
    agent_inbox_dir: &Path,
    tz: chrono_tz::Tz,
) -> (String, Vec<ImageData>, Vec<MessageAttachment>) {
    let mut content = String::new();
    let mut images = Vec::new();
    let mut attachments = Vec::new();

    for part in parts {
        match &part.content {
            PartContent::Text(text) => append_text(&mut content, text),
            PartContent::Raw(bytes) => {
                append_raw_part(
                    &mut content,
                    &mut images,
                    &mut attachments,
                    part,
                    bytes,
                    agent_inbox_dir,
                    tz,
                )
                .await;
            }
            PartContent::Url(url) => {
                append_url_part(
                    &mut content,
                    &mut images,
                    &mut attachments,
                    part,
                    url,
                    agent_inbox_dir,
                    tz,
                )
                .await;
            }
            PartContent::Data(value) => append_data_part(&mut content, value),
        }
    }

    (content, images, attachments)
}

fn append_text(content: &mut String, text: &str) {
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(text);
}

fn append_data_part(content: &mut String, value: &serde_json::Value) {
    content.push('\n');
    content.push_str("[Data part]\n");
    content.push_str(&serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()));
}

/// A raw (inline byte) part: an image within the inline size limit becomes
/// an [`ImageData`] directly; anything else is saved to the agent inbox and
/// referenced in `content`.
async fn append_raw_part(
    content: &mut String,
    images: &mut Vec<ImageData>,
    attachments: &mut Vec<MessageAttachment>,
    part: &Part,
    bytes: &[u8],
    agent_inbox_dir: &Path,
    tz: chrono_tz::Tz,
) {
    let filename = part
        .filename
        .clone()
        .unwrap_or_else(|| "attachment".to_string());
    let content_type = part.media_type.clone();
    let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    let info = AttachmentInfo {
        filename,
        size,
        content_type: content_type.clone(),
    };
    let is_inline_image =
        content_type.as_deref().is_some_and(is_supported_image) && size <= MAX_IMAGE_INLINE_SIZE;
    if is_inline_image {
        images.push(ImageData {
            media_type: content_type.unwrap_or_default(),
            data: base64_encode(bytes),
        });
        return;
    }
    let item = match attachment::reserve_attachment_item(
        agent_inbox_dir,
        &info,
        "A2A",
        attachment::AttachmentKind::Attachment,
        tz,
    )
    .await
    {
        Ok(item) => item,
        Err(e) => {
            append_failed_attachment(content, &info, &e);
            return;
        }
    };
    let saved = item.save_bytes(&info, bytes).await;
    match saved {
        Ok(saved) => {
            finalize_and_append(&saved, &info, content, item, images, attachments).await;
        }
        Err(e) => {
            item.discard().await;
            append_failed_attachment(content, &info, &e);
        }
    }
}

/// A url part naming an `http(s)` resource: downloaded and handled the same
/// way as a raw attachment part. Any other scheme is reported as a failed
/// attachment rather than attempted.
async fn append_url_part(
    content: &mut String,
    images: &mut Vec<ImageData>,
    attachments: &mut Vec<MessageAttachment>,
    part: &Part,
    url: &str,
    agent_inbox_dir: &Path,
    tz: chrono_tz::Tz,
) {
    let filename = part
        .filename
        .clone()
        .unwrap_or_else(|| "attachment".to_string());
    let info = AttachmentInfo {
        filename,
        size: 0,
        content_type: part.media_type.clone(),
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        append_failed_attachment(content, &info, "only http(s) urls are supported");
        return;
    }
    let item = match attachment::reserve_attachment_item(
        agent_inbox_dir,
        &info,
        "A2A",
        attachment::AttachmentKind::Attachment,
        tz,
    )
    .await
    {
        Ok(item) => item,
        Err(e) => {
            append_failed_attachment(content, &info, &e);
            return;
        }
    };
    let downloaded = item.download(&info, url).await;
    match downloaded {
        Ok(saved) => {
            finalize_and_append(&saved, &info, content, item, images, attachments).await;
        }
        Err(e) => {
            item.discard().await;
            append_failed_attachment(content, &info, &e);
        }
    }
}

async fn finalize_and_append(
    saved: &attachment::SavedAttachment,
    info: &AttachmentInfo,
    content: &mut String,
    item: attachment::AttachmentItem,
    images: &mut Vec<ImageData>,
    attachments: &mut Vec<MessageAttachment>,
) {
    let finalized = item.finalize(saved, info, content, "a2a caller").await;
    if let Some(image) = finalized.image {
        images.push(image);
    }
    attachments.push(finalized.attachment);
}

/// Name a failed attachment in the message and log why it failed, once for
/// every failure path (reserve, save, download, unsupported url).
fn append_failed_attachment(content: &mut String, info: &AttachmentInfo, reason: &str) {
    tracing::warn!(
        filename = %info.filename,
        error = %reason,
        "failed to save a2a attachment"
    );
    content.push('\n');
    content.push_str(&attachment::format_failed_attachment_line(info, reason));
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_caller_displays_as_the_bare_slug() {
        let (name, location) = caller_display("sibling:laptop");
        assert_eq!(name, "laptop");
        assert_eq!(location, Some("your own other Residuum instance"));
    }

    #[test]
    fn key_caller_displays_as_the_bare_key_name() {
        let (name, location) = caller_display("key:research_buddy");
        assert_eq!(name, "research_buddy");
        assert_eq!(location, Some("an external agent with a caller key"));
    }

    #[test]
    fn unrecognized_caller_shape_passes_through_unchanged() {
        let (name, location) = caller_display("unexpected");
        assert_eq!(name, "unexpected");
        assert_eq!(location, None);
    }

    #[test]
    fn inbound_room_model_escalates_only_for_trusted_sibling_rooms() {
        let mut room_meta = HashMap::new();
        room_meta.insert("residuum.room".to_string(), serde_json::json!(true));

        assert_eq!(
            inbound_room_model("sibling:team-b", Some(&room_meta)),
            ModelSelector::MAIN
        );
        assert_eq!(
            inbound_room_model("key:partner", Some(&room_meta)),
            ModelSelector::MEDIUM
        );
        assert_eq!(
            inbound_room_model("sibling:team-b", None),
            ModelSelector::MEDIUM
        );

        let mut false_meta = HashMap::new();
        false_meta.insert("residuum.room".to_string(), serde_json::json!(false));
        assert_eq!(
            inbound_room_model("sibling:team-b", Some(&false_meta)),
            ModelSelector::MEDIUM
        );

        let mut string_meta = HashMap::new();
        string_meta.insert("residuum.room".to_string(), serde_json::json!("true"));
        assert_eq!(
            inbound_room_model("sibling:team-b", Some(&string_meta)),
            ModelSelector::MEDIUM
        );
    }

    async fn test_executor_setup() -> (
        SessionExecutor,
        Arc<SessionRegistry>,
        residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent>,
    ) {
        use residuum_agent::hop::HopLimits;
        use residuum_config::A2aConfig;
        use residuum_skills::{SkillIndex, SkillState};

        use crate::card::{CardRuntime, CardState};

        let bus_handle = residuum_bus::spawn_broker();
        let spawn_sub: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            residuum_background::store::SessionStore::new(dir.path().to_path_buf())
                .with_tempdir_guard(dir),
        );
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            Arc::clone(&store),
            HopLimits { soft: 8, hard: 32 },
        ));
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let card_runtime =
            CardRuntime::from_config(&A2aConfig::default(), "127.0.0.1:0", "planner", None);
        let inbox_dir = tempfile::tempdir().unwrap();
        let card_state =
            CardState::load_or_default(&inbox_dir.path().join("card.json"), &card_runtime);
        let executor = SessionExecutor::new(
            messenger,
            Arc::clone(&registry),
            bus_handle,
            skill_state,
            card_state,
            inbox_dir.path().to_path_buf(),
            chrono_tz::UTC,
        );
        (executor, registry, spawn_sub)
    }

    #[tokio::test]
    async fn sibling_room_close_stops_session_without_delivering_turn() {
        use futures_util::StreamExt as _;
        use residuum_bus::EventTrigger;
        use residuum_bus::session::{SessionCategory, SessionInfo};
        use tokio_util::sync::CancellationToken;

        let (executor, registry, mut spawn_sub) = test_executor_setup().await;

        let caller = "sibling:team-b";
        let context_id = "room-room_01jcv8k9m2p4q7";
        let address = SessionExecutor::address_for(caller, context_id);
        let stop_token = CancellationToken::new();
        let mut interrupt_rx = registry
            .register(
                SessionInfo {
                    address: address.clone(),
                    run_id: "run-room-1".to_string(),
                    category: SessionCategory::External,
                    trigger: EventTrigger::Conversation,
                    source_label: "a2a:team-b".to_string(),
                    state: SessionState::Idle,
                    spawner: None,
                    depth: 0,
                    purpose: "room session".to_string(),
                    agent_skill: None,
                    model: ModelSelector::MAIN,
                    resolved_model: None,
                    conversation_target: None,
                    started_at: chrono::Utc::now(),
                    usage: residuum_bus::usage::SessionUsageTotals::default(),
                    overlap: None,
                },
                stop_token.clone(),
            )
            .unwrap();

        let mut msg = A2aMessage::new(a2a::Role::User, vec![Part::text("[room closed]")]);
        msg.metadata = Some(HashMap::from([
            ("residuum.room".to_string(), serde_json::json!(true)),
            ("residuum.room_close".to_string(), serde_json::json!(true)),
        ]));

        let mut service_params = a2a_server::ServiceParams::new();
        service_params.insert(CALLER_HEADER.to_string(), vec![caller.to_string()]);

        let ctx = ExecutorContext {
            message: Some(msg.clone()),
            task_id: "task-close-1".to_string(),
            stored_task: None,
            context_id: context_id.to_string(),
            metadata: None,
            user: None,
            service_params,
            tenant: None,
        };

        let events: Vec<_> = executor.execute(ctx).collect().await;
        assert_eq!(
            events.len(),
            1,
            "sibling room close must emit exactly one status update"
        );
        match events.first() {
            Some(Ok(StreamResponse::StatusUpdate(ev))) => {
                assert_eq!(
                    ev.status.state,
                    TaskState::Completed,
                    "sibling room close must complete immediately"
                );
                assert!(
                    ev.status.message.is_none(),
                    "sibling room close must not emit an agent reply message"
                );
            }
            other => panic!("expected Completed status update, got {other:?}"),
        }

        assert!(
            stop_token.is_cancelled(),
            "sibling room close must cancel the registered session's stop token"
        );
        assert!(
            interrupt_rx.try_recv().is_err(),
            "sibling room close must not deliver an inbound conversation message"
        );
        assert!(
            futures_util::FutureExt::now_or_never(spawn_sub.recv()).is_none(),
            "sibling room close must not publish a spawn request"
        );
        assert!(
            !is_sibling_room_close("key:partner", &msg),
            "non-sibling key callers must not trigger the sibling room-close shortcut"
        );
    }

    async fn publish_room_turn_events(
        publisher: &residuum_bus::Publisher,
        address: &residuum_bus::SessionAddress,
        run_id: &str,
        turn_id: &str,
        reply: &str,
    ) {
        for kind in [
            SessionEventKind::TurnStarted {
                turn_id: turn_id.to_string(),
            },
            SessionEventKind::Response {
                turn_id: turn_id.to_string(),
                content: reply.to_string(),
            },
            SessionEventKind::TurnEnded {
                turn_id: turn_id.to_string(),
            },
        ] {
            publisher
                .publish(
                    topics::Sessions,
                    SessionEvent {
                        address: address.clone(),
                        run_id: run_id.to_string(),
                        kind,
                    },
                )
                .await
                .unwrap();
        }
    }

    fn room_context_msg(text: &str, close: bool) -> A2aMessage {
        let mut msg = A2aMessage::new(a2a::Role::User, vec![Part::text(text)]);
        let mut meta = HashMap::from([("residuum.room".to_string(), serde_json::json!(true))]);
        if close {
            meta.insert("residuum.room_close".to_string(), serde_json::json!(true));
        }
        msg.metadata = Some(meta);
        msg
    }

    fn room_executor_ctx(
        msg: A2aMessage,
        task_id: &str,
        context_id: &str,
        caller: &str,
    ) -> ExecutorContext {
        let mut service_params = a2a_server::ServiceParams::new();
        service_params.insert(CALLER_HEADER.to_string(), vec![caller.to_string()]);
        ExecutorContext {
            message: Some(msg),
            task_id: task_id.to_string(),
            stored_task: None,
            context_id: context_id.to_string(),
            metadata: None,
            user: None,
            service_params,
            tenant: None,
        }
    }

    fn register_idle_room_session(
        registry: &SessionRegistry,
        address: &residuum_bus::SessionAddress,
        run_id: &str,
        stop_token: tokio_util::sync::CancellationToken,
    ) -> tokio::sync::mpsc::Receiver<residuum_bus::interrupt::Interrupt> {
        use residuum_bus::EventTrigger;
        use residuum_bus::session::{SessionCategory, SessionInfo};

        registry
            .register(
                SessionInfo {
                    address: address.clone(),
                    run_id: run_id.to_string(),
                    category: SessionCategory::External,
                    trigger: EventTrigger::Conversation,
                    source_label: "a2a:team-b".to_string(),
                    state: SessionState::Idle,
                    spawner: None,
                    depth: 0,
                    purpose: "room session".to_string(),
                    agent_skill: None,
                    model: ModelSelector::MAIN,
                    resolved_model: None,
                    conversation_target: None,
                    started_at: chrono::Utc::now(),
                    usage: residuum_bus::usage::SessionUsageTotals::default(),
                    overlap: None,
                },
                stop_token,
            )
            .unwrap()
    }

    #[tokio::test]
    async fn inbound_room_turn_completes_on_turn_ended_across_multiple_turns() {
        use futures_util::StreamExt as _;
        use tokio_util::sync::CancellationToken;

        let (executor, registry, mut spawn_sub) = test_executor_setup().await;
        let publisher = executor.bus_handle.publisher();

        let caller = "sibling:team-b";
        let context_id = "room-room_01jcv8k9m2p4q7";
        let address = SessionExecutor::address_for(caller, context_id);

        let stream1 = executor.execute(room_executor_ctx(
            room_context_msg("First room prompt", false),
            "task-room-1",
            context_id,
            caller,
        ));

        let spawn_req =
            residuum_core::testing::wait::guarded("room spawn request", spawn_sub.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(spawn_req.address, address);

        let stop_token = CancellationToken::new();
        let mut interrupt_rx = register_idle_room_session(
            &registry,
            &address,
            "run-room-turn-test",
            stop_token.clone(),
        );

        publish_room_turn_events(
            &publisher,
            &address,
            "run-room-turn-test",
            "turn-1",
            "Here is my room reply",
        )
        .await;

        let events1: Vec<_> = residuum_core::testing::wait::guarded(
            "turn 1 stream completion",
            stream1.collect::<Vec<_>>(),
        )
        .await;
        assert_completed_text(events1.last(), "Here is my room reply");
        assert!(
            !stop_token.is_cancelled(),
            "session stays idle after turn 1"
        );

        let stream2 = executor.execute(room_executor_ctx(
            room_context_msg("Second room prompt", false),
            "task-room-2",
            context_id,
            caller,
        ));

        let delivered =
            residuum_core::testing::wait::guarded("turn 2 interrupt", interrupt_rx.recv())
                .await
                .expect("turn 2 must deliver into idle session");
        assert!(matches!(
            delivered,
            residuum_bus::interrupt::Interrupt::UserMessage(_)
        ));

        publish_room_turn_events(
            &publisher,
            &address,
            "run-room-turn-test",
            "turn-2",
            "Second room reply",
        )
        .await;

        let events2: Vec<_> = residuum_core::testing::wait::guarded(
            "turn 2 stream completion",
            stream2.collect::<Vec<_>>(),
        )
        .await;
        assert_completed_text(events2.last(), "Second room reply");

        let close_events: Vec<_> = executor
            .execute(room_executor_ctx(
                room_context_msg("[room closed]", true),
                "task-room-close",
                context_id,
                caller,
            ))
            .collect()
            .await;
        assert_eq!(close_events.len(), 1);
        assert!(stop_token.is_cancelled());
    }

    async fn test_executor_with_inbox() -> (
        tempfile::TempDir,
        PathBuf,
        SessionExecutor,
        Arc<SessionRegistry>,
    ) {
        use residuum_agent::hop::HopLimits;
        use residuum_config::A2aConfig;
        use residuum_skills::{SkillIndex, SkillState};

        use crate::card::{CardRuntime, CardState};

        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox/agent");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let store = Arc::new(residuum_background::store::SessionStore::new(
            dir.path().join("sessions"),
        ));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus_handle.publisher(),
            store,
            HopLimits { soft: 8, hard: 32 },
        ));
        let card_runtime =
            CardRuntime::from_config(&A2aConfig::default(), "127.0.0.1:0", "sloane", None);
        let card_state = CardState::load_or_default(&dir.path().join("card.json"), &card_runtime);
        let executor = SessionExecutor::new(
            messenger,
            Arc::clone(&registry),
            bus_handle,
            SkillState::new_shared(SkillIndex::default(), vec![]),
            card_state,
            inbox_dir.clone(),
            chrono_tz::UTC,
        );
        (dir, inbox_dir, executor, registry)
    }

    #[tokio::test]
    async fn sibling_room_close_delivers_minutes_inbox_and_attaches_unseen_flush() {
        use futures_util::StreamExt as _;
        use residuum_bus::EventTrigger;
        use residuum_bus::session::{SessionCategory, SessionInfo};
        use tokio_util::sync::CancellationToken;

        let (_dir, inbox_dir, executor, registry) = test_executor_with_inbox().await;
        let caller = "sibling:aether";
        let context_id = "room-room_01jcv8k9m2p4q7";
        let address = SessionExecutor::address_for(caller, context_id);
        let stop_token = CancellationToken::new();
        let _interrupt_rx = registry
            .register(
                SessionInfo {
                    address: address.clone(),
                    run_id: "run-sibling-close-1".to_string(),
                    category: SessionCategory::External,
                    trigger: EventTrigger::Conversation,
                    source_label: "a2a:aether".to_string(),
                    state: SessionState::Idle,
                    spawner: None,
                    depth: 1,
                    purpose: "room session".to_string(),
                    agent_skill: None,
                    model: ModelSelector::MAIN,
                    resolved_model: None,
                    conversation_target: None,
                    started_at: chrono::Utc::now(),
                    usage: residuum_bus::usage::SessionUsageTotals::default(),
                    overlap: None,
                },
                stop_token.clone(),
            )
            .unwrap();

        let mut msg = A2aMessage::new(
            a2a::Role::User,
            vec![Part::text("[Owner]: Final decision locked.")],
        );
        msg.context_id = Some(context_id.to_string());
        msg.metadata = Some(HashMap::from([
            ("residuum.room".to_string(), serde_json::json!(true)),
            ("residuum.room_close".to_string(), serde_json::json!(true)),
            (
                "residuum.room_id".to_string(),
                serde_json::json!("room_01jcv8k9m2p4q7"),
            ),
            (
                "residuum.room_name".to_string(),
                serde_json::json!("Ventures WBR"),
            ),
            (
                "residuum.room_has_unseen".to_string(),
                serde_json::json!(true),
            ),
            (
                "residuum.room_minutes".to_string(),
                serde_json::json!("# Meeting Minutes\n\n- Approved Q4 plan."),
            ),
            (
                "residuum.room_minutes_path".to_string(),
                serde_json::json!("team/minutes/2026-11-19-ventures-wbr.md"),
            ),
        ]));

        let events: Vec<_> = executor
            .execute(room_executor_ctx(msg, "task-close-2", context_id, caller))
            .collect()
            .await;
        assert_eq!(
            events.len(),
            1,
            "sibling close must complete in one status update"
        );
        assert!(
            stop_token.is_cancelled(),
            "sibling close must cancel the live room session stop token"
        );

        let items = residuum_workspace::inbox::list_items(&inbox_dir)
            .await
            .unwrap();
        assert_eq!(
            items.len(),
            1,
            "sibling close must write meeting minutes to remote agent inbox"
        );
        let first = items.first().unwrap();
        assert_eq!(first.1.title, "Meeting minutes: Ventures WBR");
        assert!(
            first.1.body.contains("Approved Q4 plan.")
                && first
                    .1
                    .body
                    .contains("team/minutes/2026-11-19-ventures-wbr.md"),
            "remote inbox body must contain minutes markdown and shared path: {}",
            first.1.body
        );
    }

    fn assert_completed_text(last: Option<&Result<StreamResponse, A2AError>>, expected: &str) {
        match last {
            Some(Ok(StreamResponse::StatusUpdate(ev))) => {
                assert_eq!(ev.status.state, TaskState::Completed);
                let reply = ev
                    .status
                    .message
                    .as_ref()
                    .and_then(|m| m.parts.first())
                    .and_then(|p| match &p.content {
                        PartContent::Text(t) => Some(t.as_str()),
                        PartContent::Raw(_) | PartContent::Url(_) | PartContent::Data(_) => None,
                    });
                assert_eq!(reply, Some(expected));
            }
            other => panic!("expected Completed status update, got {other:?}"),
        }
    }
}

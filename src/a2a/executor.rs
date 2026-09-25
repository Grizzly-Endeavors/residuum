//! The A2A [`a2a_server::AgentExecutor`]: turns an inbound A2A message into
//! an `a2a` conversation session and maps that session's lifecycle back onto
//! A2A task states. See `docs/systems-usage/a2a.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2a::{
    A2AError, Artifact, Message as A2aMessage, Part, PartContent, StreamResponse,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use a2a_server::{AgentExecutor, ExecutorContext};
use futures_util::stream::BoxStream;

use crate::background::messaging::{AgentMessenger, ConversationSpawn};
use crate::background::registry::{SessionRegistry, SessionState, conversation_session_address};
use crate::bus::{
    A2aTaskSignalEvent, A2aTaskSignalState, AgentResultStatus, BusHandle, SessionEvent,
    SessionEventKind, topics,
};
use crate::config::BackgroundModelTier;
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::{
    self, AttachmentInfo, MAX_IMAGE_INLINE_SIZE, is_supported_image,
};
use crate::interfaces::types::{
    ConversationContext, ConversationKind, InboundMessage, MessageOrigin,
};
use crate::skills::SharedSkillState;

use super::auth::CALLER_HEADER;
use super::card::SharedCardState;

/// Bounds the `a2a` endpoint name used for the conversation-turn stream and
/// the endpoint registry entry.
const A2A_ENDPOINT: &str = "a2a";

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
        }
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
    pub fn address_for(caller: &str, context_id: &str) -> crate::bus::SessionAddress {
        conversation_session_address(A2A_ENDPOINT, &format!("{caller}/{context_id}"))
    }

    /// Skill to activate for a new session, from the message's
    /// `metadata.skill` when it names both a card skill id and a workspace
    /// skill. Returns the note to prepend to content when a skill was named
    /// but didn't map to anything runnable.
    async fn resolve_skill(
        &self,
        message: &A2aMessage,
    ) -> (Option<crate::bus::SkillName>, Option<String>) {
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
            (Some(crate::bus::SkillName::from(requested)), None)
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
        tokio::spawn(async move {
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
    sessions: crate::bus::Subscriber<SessionEvent>,
    signals: crate::bus::Subscriber<A2aTaskSignalEvent>,
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

    // Subscribe BEFORE delivering the message, so nothing the session does
    // in response can be published and missed before we start listening.
    let mut subs = match subscribe_before_delivery(&executor).await {
        Ok(subs) => subs,
        Err(e) => {
            tx.send(Err(e)).await.ok();
            return;
        }
    };

    let Some(message) = ctx.message.clone() else {
        tx.send(Err(A2AError::internal("execute() called with no message")))
            .await
            .ok();
        return;
    };

    if let Err(e) = deliver_inbound(&executor, &address, &caller, &context_id, &message).await {
        tx.send(Err(e)).await.ok();
        return;
    }

    stream_until_terminal(&executor, &address, &task_id, &context_id, &mut subs, &tx).await;
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

/// Build the session's inbound content from the A2A message and deliver it
/// into the conversation session, per the skill-mapping and part-handling
/// rules in `docs/systems-usage/a2a.md`.
async fn deliver_inbound(
    executor: &SessionExecutor,
    address: &crate::bus::SessionAddress,
    caller: &str,
    context_id: &str,
    message: &A2aMessage,
) -> Result<(), A2AError> {
    let (skill, skill_note) = executor.resolve_skill(message).await;
    let (mut content, images) =
        build_inbound_content(&message.parts, &executor.agent_inbox_dir, executor.tz).await;
    if let Some(note) = skill_note {
        content = format!("{note}\n{content}");
    }

    let (display_name, location) = caller_display(caller);
    let inbound = InboundMessage {
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
        timestamp: chrono::Utc::now(),
        images,
        context: None,
    };
    let spawn = ConversationSpawn {
        source_label: format!("a2a:{display_name}"),
        model_tier: BackgroundModelTier::Medium,
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
    address: &'a crate::bus::SessionAddress,
    task_id: &'a str,
    context_id: &'a str,
    tx: &'a ExecutorSender,
}

/// Follow the session's activity on the bus until it reaches a terminal (or
/// input-required) A2A state, translating each relevant event into a
/// [`StreamResponse`] sent to `tx`. Returns once the stream should end —
/// either because a terminal outcome was sent, or because a subscription (or
/// the channel to the caller) closed.
async fn stream_until_terminal(
    executor: &SessionExecutor,
    address: &crate::bus::SessionAddress,
    task_id: &str,
    context_id: &str,
    subs: &mut ExecutionSubscriptions,
    tx: &ExecutorSender,
) {
    let stream = StreamCtx {
        executor,
        address,
        task_id,
        context_id,
        tx,
    };
    let mut last_final_text = String::new();
    let mut sent_working = false;
    // Turns that started while this execution was following the session.
    // A turn already running when the execution began may be the one that
    // signaled the previous task's outcome; its closing text belongs to that
    // task, so only responses from turns started here are relayed.
    let mut own_turns: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            signal = subs.signals.recv() => {
                let Ok(Some(signal)) = signal else { return };
                if signal.address != *stream.address {
                    continue;
                }
                emit_signal_outcome(&stream, signal).await;
                return;
            }
            event = subs.sessions.recv() => {
                let Ok(Some(event)) = event else { return };
                if event.address != *stream.address {
                    continue;
                }
                if handle_session_event(&stream, event.kind, &mut last_final_text, &mut sent_working, &mut own_turns).await {
                    return;
                }
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
    last_final_text: &mut String,
    sent_working: &mut bool,
    own_turns: &mut HashSet<String>,
) -> bool {
    match kind {
        SessionEventKind::TurnStarted { turn_id } => {
            own_turns.insert(turn_id);
            // The turn actually starting is a stronger, earlier signal that
            // the session is working than waiting on a separate
            // `StateChanged(Running)` event below: both are published in
            // the same burst when a run begins, so relying on only one of
            // them left this racy under load — a subscriber's bounded bus
            // channel (`bus::broker::SUBSCRIBER_CAPACITY`) can drop either
            // one individually if it's momentarily full, and `TurnStarted`
            // is also the event `own_turns` itself already depends on
            // being delivered reliably.
            if *sent_working {
                return false;
            }
            *sent_working = true;
            let update = status_update(stream.task_id, stream.context_id, TaskState::Working, None);
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::Response { turn_id, content } => {
            if !own_turns.contains(&turn_id) || content.is_empty() {
                return false;
            }
            last_final_text.clone_from(&content);
            *sent_working = true;
            let update = status_update(
                stream.task_id,
                stream.context_id,
                TaskState::Working,
                Some(&content),
            );
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::StateChanged(SessionState::Running) if !*sent_working => {
            *sent_working = true;
            let update = status_update(stream.task_id, stream.context_id, TaskState::Working, None);
            stream.tx.send(Ok(update)).await.is_err()
        }
        SessionEventKind::Completed { status, .. } => {
            handle_run_completed(stream, status, last_final_text).await
        }
        SessionEventKind::StateChanged(_)
        | SessionEventKind::Started(_)
        | SessionEventKind::TurnEnded { .. }
        | SessionEventKind::ToolCall(_)
        | SessionEventKind::ToolResult(_)
        | SessionEventKind::Intermediate { .. }
        | SessionEventKind::Error { .. }
        | SessionEventKind::MessageToMain { .. }
        // A2A has no notion of token usage/elapsed time in its task
        // protocol, and this must never reach the agent either way — see
        // `docs/systems-usage/turn-control.md`.
        | SessionEventKind::TurnUsage { .. } => false,
    }
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
) -> (String, Vec<ImageData>) {
    let mut content = String::new();
    let mut images = Vec::new();

    for part in parts {
        match &part.content {
            PartContent::Text(text) => append_text(&mut content, text),
            PartContent::Raw(bytes) => {
                append_raw_part(&mut content, &mut images, part, bytes, agent_inbox_dir, tz).await;
            }
            PartContent::Url(url) => {
                append_url_part(&mut content, &mut images, part, url, agent_inbox_dir, tz).await;
            }
            PartContent::Data(value) => append_data_part(&mut content, value),
        }
    }

    (content, images)
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
    match attachment::save_attachment_bytes(&info, bytes, agent_inbox_dir).await {
        Ok(saved) => {
            if let Some(image) =
                finalize_and_append(&saved, &info, content, agent_inbox_dir, tz).await
            {
                images.push(image);
            }
        }
        Err(e) => append_failed_attachment(content, &info, &e),
    }
}

/// A url part naming an `http(s)` resource: downloaded and handled the same
/// way as a raw attachment part. Any other scheme is reported as a failed
/// attachment rather than attempted.
async fn append_url_part(
    content: &mut String,
    images: &mut Vec<ImageData>,
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
    match attachment::download_attachment(&info, url, agent_inbox_dir).await {
        Ok(saved) => {
            if let Some(image) =
                finalize_and_append(&saved, &info, content, agent_inbox_dir, tz).await
            {
                images.push(image);
            }
        }
        Err(e) => append_failed_attachment(content, &info, &e),
    }
}

async fn finalize_and_append(
    saved: &attachment::SavedAttachment,
    info: &AttachmentInfo,
    content: &mut String,
    agent_inbox_dir: &Path,
    tz: chrono_tz::Tz,
) -> Option<ImageData> {
    attachment::finalize_attachment(
        saved,
        info,
        content,
        "a2a caller",
        agent_inbox_dir,
        tz,
        "A2A",
    )
    .await
}

fn append_failed_attachment(content: &mut String, info: &AttachmentInfo, reason: &str) {
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
}

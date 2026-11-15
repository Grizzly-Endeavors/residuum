//! Local and remote A2A room participant drivers.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a2a::{
    Artifact, CancelTaskRequest, GetTaskRequest, Message as A2aMessage, Part, PartContent, Role,
    SendMessageConfiguration, SendMessageRequest, SendMessageResponse, StreamResponse, Task,
    TaskState,
};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use serde_json::json;
use tokio::sync::{Mutex, mpsc};

use residuum_a2a::{A2aClientHub, AgentSnapshot, AgentSource, AgentStatus};

use crate::types::{RoomAttachment, RoomMessage, RoomRecord, RoomRosterCandidate, RoomSeatKind};

/// Metadata key marking an outbound A2A turn as originating from a meeting room.
pub const ROOM_METADATA_FLAG: &str = "residuum.room";
/// Metadata key carrying the room ID.
pub const ROOM_METADATA_ID: &str = "residuum.room_id";
/// Metadata key carrying the room display name.
pub const ROOM_METADATA_NAME: &str = "residuum.room_name";
/// Metadata key sent on archive close cancel hints.
pub const ROOM_METADATA_CLOSE: &str = "residuum.room_close";

/// Maximum polling iterations for a non-streaming remote A2A task.
const MAX_NON_STREAMING_POLLS: usize = 120;
const POLL_BASE_MS: u64 = 250;
const POLL_MAX_MS: u64 = 2_000;
/// Maximum persisted inbound A2A attachment size (50 MiB).
const MAX_ROOM_ATTACHMENT_BYTES: usize = 50 * 1024 * 1024;
/// Bounded timeout for best-effort sibling room session close notifications.
const CLOSE_SESSION_TIMEOUT: Duration = Duration::from_secs(5);

/// Input passed to a local agent's isolated `external-room-<room_id>` session.
#[derive(Debug, Clone)]
pub struct LocalRoomTurnRequest {
    /// Room ID (`room_...`).
    pub room_id: String,
    /// Room display name.
    pub room_name: String,
    /// Optional room topic.
    pub room_topic: Option<String>,
    /// Catch-up delta prompt containing messages since this seat's last turn.
    pub prompt: String,
    /// Attachments carried on unseen messages since this seat's last turn.
    pub attachments: Vec<RoomAttachment>,
    /// Channel for streaming live tool-call status strings (`Some("tool")` / `None`).
    pub tool_status_tx: mpsc::UnboundedSender<Option<String>>,
}

/// Completed output from a local or remote room participant turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomTurnOutput {
    /// Final assistant reply markdown text.
    pub content: String,
    /// Any files/images returned by the participant.
    pub attachments: Vec<RoomAttachment>,
}

/// Inversion-of-control interface implemented by `residuum-gateway` for each
/// running local agent and registered with [`crate::RoomService`].
#[async_trait]
pub trait LocalRoomParticipant: Send + Sync {
    /// Run one room turn in isolated session `external-room-<room_id>` on
    /// `ModelSelector::MAIN` and return the completed reply and attachments.
    async fn run_room_turn(&self, req: LocalRoomTurnRequest) -> Result<RoomTurnOutput, String>;

    /// Cancel an in-flight turn in `external-room-<room_id>` without closing
    /// or merging the room session.
    async fn cancel_room_turn(&self, room_id: &str) -> Result<(), String>;

    /// Close `external-room-<room_id>` on room archive so normal episode
    /// extraction and memory merging run.
    async fn close_room_session(&self, room_id: &str) -> Result<(), String>;

    /// Generate structured markdown meeting minutes using this agent's `main`
    /// provider chain (one-shot completion, no tools, no session history mutation).
    async fn generate_minutes(&self, prompt: String) -> Result<String, String>;

    /// Announce a newly written user-inbox item on this agent's event bus so
    /// inbox counts and Web Push update immediately.
    async fn announce_user_inbox(&self, _item_id: &str) {}
}

/// Input passed to a remote A2A room participant driver.
#[derive(Debug, Clone)]
pub struct RemoteRoomTurnRequest {
    /// Canonical remote seat ID (`a2a:<instance>/<agent>` or `a2a:<name>`).
    pub seat_id: String,
    /// Room ID (`room_...`).
    pub room_id: String,
    /// Room display name.
    pub room_name: String,
    /// Catch-up delta prompt containing messages since this seat's last turn.
    pub prompt: String,
}

/// Abstracts remote A2A participant execution for [`crate::RoomService`].
#[async_trait]
pub trait RemoteRoomDriver: Send + Sync {
    /// List known remote A2A candidate seats from the client hub.
    async fn candidates(&self) -> Vec<RoomRosterCandidate>;

    /// Resolve a specific remote seat ID (`a2a:...`) into its candidate entry.
    async fn resolve_seat(&self, seat_id: &str) -> Option<RoomRosterCandidate>;

    /// Execute a single remote turn and return the completed output.
    async fn run_remote_turn(&self, req: RemoteRoomTurnRequest) -> Result<RoomTurnOutput, String>;

    /// Cancel an in-flight remote turn (`CancelTask`) for `(room_id, seat_id)`.
    async fn cancel_remote_turn(&self, room_id: &str, seat_id: &str) -> Result<(), String>;

    /// Best-effort close notification for a sibling A2A seat on archive.
    async fn close_remote_session(&self, room_id: &str, seat_id: &str) -> Result<(), String>;
}

/// Remote room participant driver backed by [`A2aClientHub`].
pub struct A2aRoomDriver {
    a2a_hub: Arc<A2aClientHub>,
    rooms_root: PathBuf,
    in_flight: Arc<Mutex<HashMap<(String, String), String>>>,
    open_input_tasks: Mutex<HashMap<(String, String), String>>,
}

impl A2aRoomDriver {
    /// Construct a new [`A2aRoomDriver`] backed by `a2a_hub` and persisting
    /// inbound attachments under `rooms_root`.
    #[must_use]
    pub fn new(a2a_hub: Arc<A2aClientHub>, rooms_root: PathBuf) -> Self {
        Self {
            a2a_hub,
            rooms_root,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            open_input_tasks: Mutex::new(HashMap::new()),
        }
    }

    async fn record_in_flight(&self, key: &(String, String), task_id: &str) {
        self.in_flight
            .lock()
            .await
            .insert(key.clone(), task_id.to_owned());
    }

    async fn clear_in_flight(&self, key: &(String, String)) {
        self.in_flight.lock().await.remove(key);
    }

    async fn update_open_input_task(
        &self,
        key: &(String, String),
        task_id: &str,
        state: &TaskState,
    ) {
        let mut guard = self.open_input_tasks.lock().await;
        if matches!(state, TaskState::InputRequired | TaskState::AuthRequired) {
            guard.insert(key.clone(), task_id.to_owned());
        } else if state.is_terminal() {
            guard.remove(key);
        }
    }

    async fn run_streaming_turn(
        &self,
        client: &residuum_a2a::client::NegotiatedClient,
        send_req: &SendMessageRequest,
        key: &(String, String),
    ) -> Result<RoomTurnOutput, String> {
        let mut stream = client
            .send_streaming_message(send_req)
            .await
            .map_err(|e| e.to_string())?;
        let mut pending_artifacts: Vec<Artifact> = Vec::new();
        let mut observed_task_id: Option<String> = None;
        let mut latest_working_text: Option<String> = None;

        while let Some(item) = stream.next().await {
            match item.map_err(|e| e.to_string())? {
                StreamResponse::Message(msg) => {
                    return Ok(self.message_to_output(&key.0, &msg).await);
                }
                StreamResponse::ArtifactUpdate(ev) => {
                    observed_task_id = Some(ev.task_id);
                    pending_artifacts.push(ev.artifact);
                }
                StreamResponse::Task(mut task) => {
                    observed_task_id = Some(task.id.clone());
                    self.record_in_flight(key, &task.id).await;
                    if !pending_artifacts.is_empty() {
                        task.artifacts
                            .get_or_insert_with(Vec::new)
                            .append(&mut pending_artifacts);
                    }
                    if is_turn_done(&task.status.state) {
                        self.update_open_input_task(key, &task.id, &task.status.state)
                            .await;
                        let mut out = self.task_to_output(&key.0, &task).await?;
                        if out.content.is_empty()
                            && let Some(working) = latest_working_text
                        {
                            out.content = working;
                        }
                        return Ok(out);
                    }
                    if task.status.state == TaskState::Working
                        && let Some(msg) = &task.status.message
                    {
                        let text = extract_message_text(msg);
                        if !text.is_empty() {
                            latest_working_text = Some(text);
                        }
                    }
                }
                StreamResponse::StatusUpdate(ev) => {
                    observed_task_id = Some(ev.task_id.clone());
                    self.record_in_flight(key, &ev.task_id).await;
                    if is_turn_done(&ev.status.state) {
                        self.update_open_input_task(key, &ev.task_id, &ev.status.state)
                            .await;
                        let mut out = self
                            .status_to_output(&key.0, &ev.status, &pending_artifacts)
                            .await?;
                        if out.content.is_empty()
                            && let Some(working) = latest_working_text
                        {
                            out.content = working;
                        }
                        return Ok(out);
                    }
                    if ev.status.state == TaskState::Working
                        && let Some(msg) = &ev.status.message
                    {
                        let text = extract_message_text(msg);
                        if !text.is_empty() {
                            latest_working_text = Some(text);
                        }
                    }
                }
            }
        }

        if let Some(task_id) = observed_task_id {
            let mut out = self.poll_task_until_done(client, key, &task_id).await?;
            if out.content.is_empty()
                && let Some(working) = latest_working_text
            {
                out.content = working;
            }
            return Ok(out);
        }
        Err("remote A2A stream ended without a reply".to_owned())
    }

    async fn poll_task_until_done(
        &self,
        client: &residuum_a2a::client::NegotiatedClient,
        key: &(String, String),
        task_id: &str,
    ) -> Result<RoomTurnOutput, String> {
        let mut delay_ms = POLL_BASE_MS;
        for _ in 0..MAX_NON_STREAMING_POLLS {
            let task = client
                .get_task(&GetTaskRequest {
                    id: task_id.to_owned(),
                    history_length: None,
                    tenant: None,
                })
                .await
                .map_err(|e| e.to_string())?;
            if is_turn_done(&task.status.state) {
                self.update_open_input_task(key, &task.id, &task.status.state)
                    .await;
                return self.task_to_output(&key.0, &task).await;
            }
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            delay_ms = (delay_ms * 2).min(POLL_MAX_MS);
        }
        Err(format!("remote A2A task '{task_id}' timed out"))
    }

    async fn message_to_output(&self, room_id: &str, msg: &A2aMessage) -> RoomTurnOutput {
        let mut attachments = Vec::new();
        collect_raw_attachments(
            &self.rooms_root,
            room_id,
            &msg.parts,
            None,
            &mut attachments,
        )
        .await;
        RoomTurnOutput {
            content: extract_message_text(msg),
            attachments,
        }
    }

    async fn task_to_output(&self, room_id: &str, task: &Task) -> Result<RoomTurnOutput, String> {
        let artifacts = task.artifacts.as_deref().unwrap_or(&[]);
        self.status_to_output(room_id, &task.status, artifacts)
            .await
    }

    async fn status_to_output(
        &self,
        room_id: &str,
        status: &a2a::TaskStatus,
        artifacts: &[Artifact],
    ) -> Result<RoomTurnOutput, String> {
        let status_text = status
            .message
            .as_ref()
            .map(extract_message_text)
            .unwrap_or_default();
        match status.state {
            TaskState::Completed | TaskState::InputRequired | TaskState::AuthRequired => {
                let mut combined = status_text;
                let mut attachments = Vec::new();
                if let Some(msg) = &status.message {
                    collect_raw_attachments(
                        &self.rooms_root,
                        room_id,
                        &msg.parts,
                        None,
                        &mut attachments,
                    )
                    .await;
                }
                for artifact in artifacts {
                    let art_text = extract_parts_text(&artifact.parts);
                    if !art_text.is_empty() {
                        if !combined.is_empty() {
                            combined.push_str("\n\n");
                        }
                        combined.push_str(&art_text);
                    }
                    collect_raw_attachments(
                        &self.rooms_root,
                        room_id,
                        &artifact.parts,
                        artifact.name.as_deref(),
                        &mut attachments,
                    )
                    .await;
                }
                Ok(RoomTurnOutput {
                    content: combined,
                    attachments,
                })
            }
            TaskState::Failed | TaskState::Rejected => {
                let detail = if status_text.is_empty() {
                    "remote agent failed to complete the turn".to_owned()
                } else {
                    status_text
                };
                Err(detail)
            }
            TaskState::Canceled => Err("turn canceled".to_owned()),
            TaskState::Unspecified | TaskState::Submitted | TaskState::Working => {
                Err("remote agent returned an incomplete task state".to_owned())
            }
        }
    }

    async fn send_cancel(
        &self,
        room_id: &str,
        seat_id: &str,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<(), String> {
        let key = (room_id.to_owned(), seat_id.to_owned());
        let task_id = {
            let active = self.in_flight.lock().await.remove(&key);
            if active.is_some() {
                self.open_input_tasks.lock().await.remove(&key);
                active
            } else {
                self.open_input_tasks.lock().await.remove(&key)
            }
        };
        let Some(task_id) = task_id else {
            return Ok(());
        };
        let agent_name = seat_id.strip_prefix("a2a:").unwrap_or(seat_id);
        let (client, _) = self
            .a2a_hub
            .client_for(agent_name)
            .await
            .map_err(|e| e.to_string())?;
        client
            .cancel_task(&CancelTaskRequest {
                id: task_id,
                metadata,
                tenant: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[async_trait]
impl RemoteRoomDriver for A2aRoomDriver {
    async fn candidates(&self) -> Vec<RoomRosterCandidate> {
        self.a2a_hub
            .snapshot()
            .await
            .iter()
            .map(snapshot_to_candidate)
            .collect()
    }

    async fn resolve_seat(&self, seat_id: &str) -> Option<RoomRosterCandidate> {
        if !seat_id.starts_with("a2a:") {
            return None;
        }
        self.candidates()
            .await
            .into_iter()
            .find(|cand| cand.seat_id == seat_id)
    }

    async fn run_remote_turn(&self, req: RemoteRoomTurnRequest) -> Result<RoomTurnOutput, String> {
        let agent_name = req.seat_id.strip_prefix("a2a:").unwrap_or(&req.seat_id);
        let (client, card) = self
            .a2a_hub
            .client_for(agent_name)
            .await
            .map_err(|e| e.to_string())?;
        let key = (req.room_id.clone(), req.seat_id.clone());
        let open_task_id = self.open_input_tasks.lock().await.get(&key).cloned();

        let mut msg = A2aMessage::new(Role::User, vec![Part::text(req.prompt)]);
        msg.context_id = Some(format!("room-{}", req.room_id));
        msg.task_id = open_task_id;
        msg.metadata = Some(HashMap::from([
            (ROOM_METADATA_FLAG.to_owned(), json!(true)),
            (ROOM_METADATA_ID.to_owned(), json!(req.room_id)),
            (ROOM_METADATA_NAME.to_owned(), json!(req.room_name)),
        ]));

        let send_req = SendMessageRequest {
            message: msg,
            configuration: Some(SendMessageConfiguration {
                accepted_output_modes: None,
                task_push_notification_config: None,
                history_length: None,
                return_immediately: Some(false),
            }),
            metadata: None,
            tenant: None,
        };

        let result = if card.capabilities.streaming == Some(true) {
            self.run_streaming_turn(&client, &send_req, &key).await
        } else {
            match client
                .send_message(&send_req)
                .await
                .map_err(|e| e.to_string())?
            {
                SendMessageResponse::Message(reply) => {
                    Ok(self.message_to_output(&key.0, &reply).await)
                }
                SendMessageResponse::Task(task) => {
                    self.record_in_flight(&key, &task.id).await;
                    if is_turn_done(&task.status.state) {
                        self.update_open_input_task(&key, &task.id, &task.status.state)
                            .await;
                        self.task_to_output(&key.0, &task).await
                    } else {
                        self.poll_task_until_done(&client, &key, &task.id).await
                    }
                }
            }
        };
        self.clear_in_flight(&key).await;
        result
    }

    async fn cancel_remote_turn(&self, room_id: &str, seat_id: &str) -> Result<(), String> {
        self.send_cancel(room_id, seat_id, None).await
    }

    async fn close_remote_session(&self, room_id: &str, seat_id: &str) -> Result<(), String> {
        let cancel_meta = HashMap::from([(ROOM_METADATA_CLOSE.to_owned(), json!(true))]);
        if let Err(err) = tokio::time::timeout(
            CLOSE_SESSION_TIMEOUT,
            self.send_cancel(room_id, seat_id, Some(cancel_meta)),
        )
        .await
        .unwrap_or_else(|_| Err(format!("timed out canceling remote task for '{seat_id}'")))
        {
            tracing::debug!(
                room_id,
                seat_id,
                error = %err,
                "best-effort CancelTask during room archive close failed; proceeding with room_close message"
            );
        }

        let agent_name = seat_id.strip_prefix("a2a:").unwrap_or(seat_id);
        let (client, _) =
            tokio::time::timeout(CLOSE_SESSION_TIMEOUT, self.a2a_hub.client_for(agent_name))
                .await
                .map_err(|elapsed| {
                    format!("timed out resolving remote A2A client '{seat_id}': {elapsed}")
                })?
                .map_err(|e| e.to_string())?;

        let mut msg = A2aMessage::new(Role::User, vec![Part::text("[room closed]")]);
        msg.context_id = Some(format!("room-{room_id}"));
        msg.metadata = Some(HashMap::from([
            (ROOM_METADATA_FLAG.to_owned(), json!(true)),
            (ROOM_METADATA_ID.to_owned(), json!(room_id)),
            (ROOM_METADATA_CLOSE.to_owned(), json!(true)),
        ]));
        let send_req = SendMessageRequest {
            message: msg,
            configuration: Some(SendMessageConfiguration {
                accepted_output_modes: None,
                task_push_notification_config: None,
                history_length: None,
                return_immediately: Some(false),
            }),
            metadata: None,
            tenant: None,
        };

        tokio::time::timeout(CLOSE_SESSION_TIMEOUT, client.send_message(&send_req))
            .await
            .map_err(|elapsed| {
                format!("timed out closing remote room session '{seat_id}': {elapsed}")
            })?
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn snapshot_to_candidate(snap: &AgentSnapshot) -> RoomRosterCandidate {
    let (kind, instance) = match snap.source {
        AgentSource::Sibling => (
            RoomSeatKind::SiblingA2a,
            snap.name.split_once('/').map(|(inst, _)| inst.to_owned()),
        ),
        AgentSource::Config => (RoomSeatKind::ExternalA2a, None),
    };
    let card = snap.card();
    let fallback_name = snap
        .name
        .rsplit_once('/')
        .map_or(snap.name.as_str(), |(_, leaf)| leaf);
    let display_name = card
        .map(|c| c.name.trim())
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback_name)
        .to_owned();
    let role = card
        .map(|c| c.description.trim())
        .filter(|d| !d.is_empty())
        .map(str::to_owned);
    let online = matches!(snap.status, AgentStatus::Ok(_));

    RoomRosterCandidate {
        seat_id: format!("a2a:{}", snap.name),
        kind,
        display_name,
        role,
        instance,
        online,
    }
}

fn is_turn_done(state: &TaskState) -> bool {
    state.is_terminal() || matches!(state, TaskState::InputRequired | TaskState::AuthRequired)
}

fn sanitize_attachment_filename(
    part_filename: Option<&str>,
    artifact_name: Option<&str>,
) -> String {
    let raw = part_filename
        .filter(|s| !s.trim().is_empty())
        .or_else(|| artifact_name.filter(|s| !s.trim().is_empty()))
        .unwrap_or("artifact.bin");
    let tail = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = tail
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return "artifact.bin".to_owned();
    }
    let safe = trimmed.replace("..", "_");
    if safe.is_empty() || safe == "." {
        "artifact.bin".to_owned()
    } else {
        safe
    }
}

async fn collect_raw_attachments(
    rooms_root: &Path,
    room_id: &str,
    parts: &[Part],
    artifact_name: Option<&str>,
    out: &mut Vec<RoomAttachment>,
) {
    if crate::types::validate_room_id(room_id).is_err() {
        tracing::warn!(
            room_id,
            "skipping A2A room attachment persistence for invalid room id"
        );
        return;
    }
    for part in parts {
        let PartContent::Raw(bytes) = &part.content else {
            continue;
        };
        if bytes.len() > MAX_ROOM_ATTACHMENT_BYTES {
            tracing::warn!(
                room_id,
                size = bytes.len(),
                max = MAX_ROOM_ATTACHMENT_BYTES,
                "skipping oversized A2A room attachment"
            );
            continue;
        }
        let att_id = format!("{:032x}", rand::random::<u128>());
        let filename = sanitize_attachment_filename(part.filename.as_deref(), artifact_name);
        let dir = rooms_root.join(room_id).join("attachments").join(&att_id);
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!(
                room_id,
                path = %dir.display(),
                error = %err,
                "failed to create A2A room attachment directory"
            );
            continue;
        }
        let path = dir.join(&filename);
        if let Err(err) = tokio::fs::write(&path, bytes).await {
            tracing::warn!(
                room_id,
                path = %path.display(),
                error = %err,
                "failed to write A2A room attachment"
            );
            continue;
        }
        out.push(RoomAttachment {
            id: att_id.clone(),
            filename,
            mime_type: part
                .media_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            url: format!("/api/hub/rooms/{room_id}/attachments/{att_id}"),
        });
    }
}

fn extract_message_text(msg: &A2aMessage) -> String {
    extract_parts_text(&msg.parts)
}

fn extract_parts_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|part| match &part.content {
            PartContent::Text(text) => Some(text.clone()),
            PartContent::Data(value) => Some(value.to_string()),
            PartContent::Raw(_) | PartContent::Url(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format the catch-up prompt for a participant whose `last_seen_seq` is behind
/// `missed_messages`.
#[must_use]
pub(crate) fn format_catchup_prompt(room: &RoomRecord, missed_messages: &[RoomMessage]) -> String {
    let topic = room
        .topic
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("none");
    let roster_names = room
        .roster
        .iter()
        .map(|s| s.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!(
        "[Meeting Room: \"{}\" | Topic: {topic} | Roster: {roster_names}]\n\
         Messages since your last turn:\n",
        room.name
    );
    if missed_messages.is_empty() {
        out.push_str("(no new messages)\n");
    } else {
        for msg in missed_messages {
            _ = writeln!(out, "[{}]: {}", msg.author_name, msg.content);
            if !msg.attachments.is_empty() {
                let names = msg
                    .attachments
                    .iter()
                    .map(|a| a.filename.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                _ = writeln!(out, "  [attachments: {names}]");
            }
        }
    }
    out.push_str(
        "\nYou have been selected to speak next in this room. Reply directly to the room \
         (concise, in character; use @Name only if handing a specific follow-up to another roster member).",
    );
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;

    use super::*;
    use crate::types::{
        RoomId, RoomMessageKind, RoomMinutesMeta, RoomSeat, RoomStatus, RoomWindowState,
    };

    #[test]
    fn catchup_prompt_formats_header_and_delta_messages() {
        let now = Utc::now();
        let room_id = RoomId::parse("room_01jcv8k9m2p4q7").unwrap();
        let room = RoomRecord {
            id: room_id.clone(),
            name: "Design Sync".to_owned(),
            topic: Some("Turn loop".to_owned()),
            status: RoomStatus::Active,
            pinned: false,
            created_at: now,
            updated_at: now,
            archived_at: None,
            roster: vec![
                RoomSeat {
                    seat_id: "agent:scout".to_owned(),
                    kind: RoomSeatKind::Local,
                    display_name: "Scout".to_owned(),
                    role: None,
                    instance: None,
                },
                RoomSeat {
                    seat_id: "a2a:ops/planner".to_owned(),
                    kind: RoomSeatKind::SiblingA2a,
                    display_name: "Planner".to_owned(),
                    role: None,
                    instance: Some("ops".to_owned()),
                },
            ],
            historical_participants: vec![],
            last_seq: 2,
            last_read_seq: 2,
            last_seen_seq: BTreeMap::new(),
            window: RoomWindowState::fresh(2),
            minutes: RoomMinutesMeta::default(),
        };
        let missed = vec![
            RoomMessage {
                seq: 1,
                id: "rmsg_00000000000001".to_owned(),
                room_id: room_id.clone(),
                kind: RoomMessageKind::Owner,
                seat_id: None,
                author_name: "Owner".to_owned(),
                seat_kind: None,
                content: "Ready?".to_owned(),
                attachments: Vec::new(),
                timestamp: now,
            },
            RoomMessage {
                seq: 2,
                id: "rmsg_00000000000002".to_owned(),
                room_id,
                kind: RoomMessageKind::Agent,
                seat_id: Some("agent:scout".to_owned()),
                author_name: "Scout".to_owned(),
                seat_kind: Some(RoomSeatKind::Local),
                content: "Yes, @Planner?".to_owned(),
                attachments: Vec::new(),
                timestamp: now,
            },
        ];

        let prompt = format_catchup_prompt(&room, &missed);
        assert!(
            prompt.contains(
                "[Meeting Room: \"Design Sync\" | Topic: Turn loop | Roster: Scout, Planner]"
            ),
            "header must match contract: {prompt}"
        );
        assert!(
            prompt.contains("[Owner]: Ready?\n[Scout]: Yes, @Planner?\n"),
            "delta messages must include speaker tags: {prompt}"
        );
        assert!(
            prompt.contains("You have been selected to speak next in this room."),
            "prompt must end with speaking instruction"
        );
    }

    #[derive(Clone, Copy)]
    enum MockReplyMode {
        CompletedTask,
        InputRequiredTask,
        WorkingUntilCanceled,
    }

    fn build_mock_rpc_response(
        parsed: &serde_json::Value,
        mode: MockReplyMode,
        canceled: &std::sync::atomic::AtomicBool,
    ) -> serde_json::Value {
        let rpc_id = parsed.get("id").cloned().unwrap_or(json!(1));
        let rpc_method = parsed.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let is_room_close = parsed
            .get("params")
            .and_then(|p| p.get("message"))
            .and_then(|m| m.get("metadata"))
            .and_then(|meta| meta.get(ROOM_METADATA_CLOSE))
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let context_id = parsed
            .get("params")
            .and_then(|p| p.get("message"))
            .and_then(|m| m.get("contextId"))
            .and_then(|c| c.as_str())
            .unwrap_or("room-room_01jcv8k9m2p4q7")
            .to_owned();

        let result_value = match rpc_method {
            "CancelTask" => {
                canceled.store(true, std::sync::atomic::Ordering::SeqCst);
                serde_json::to_value(Task {
                    id: "task-1".to_owned(),
                    context_id,
                    status: a2a::TaskStatus {
                        state: TaskState::Canceled,
                        message: None,
                        timestamp: None,
                    },
                    artifacts: None,
                    history: None,
                    metadata: None,
                })
                .unwrap()
            }
            "GetTask" => {
                let state = if canceled.load(std::sync::atomic::Ordering::SeqCst) {
                    TaskState::Canceled
                } else {
                    TaskState::Working
                };
                serde_json::to_value(Task {
                    id: "task-1".to_owned(),
                    context_id,
                    status: a2a::TaskStatus {
                        state,
                        message: None,
                        timestamp: None,
                    },
                    artifacts: None,
                    history: None,
                    metadata: None,
                })
                .unwrap()
            }
            _ if is_room_close => serde_json::to_value(SendMessageResponse::Task(Task {
                id: "task-close".to_owned(),
                context_id,
                status: a2a::TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: None,
                },
                artifacts: None,
                history: None,
                metadata: None,
            }))
            .unwrap(),
            _ => {
                let state = match mode {
                    MockReplyMode::CompletedTask => TaskState::Completed,
                    MockReplyMode::InputRequiredTask => TaskState::InputRequired,
                    MockReplyMode::WorkingUntilCanceled => TaskState::Working,
                };
                let reply_msg = match mode {
                    MockReplyMode::CompletedTask | MockReplyMode::InputRequiredTask => Some(
                        A2aMessage::new(Role::Agent, vec![Part::text("Ops reply ready")]),
                    ),
                    MockReplyMode::WorkingUntilCanceled => None,
                };
                serde_json::to_value(SendMessageResponse::Task(Task {
                    id: "task-1".to_owned(),
                    context_id,
                    status: a2a::TaskStatus {
                        state,
                        message: reply_msg,
                        timestamp: None,
                    },
                    artifacts: None,
                    history: None,
                    metadata: None,
                }))
                .unwrap()
            }
        };
        json!({
            "jsonrpc": "2.0",
            "id": rpc_id,
            "result": result_value,
        })
    }

    async fn spawn_mock_a2a_server(
        captured: Arc<Mutex<Vec<serde_json::Value>>>,
        mode: MockReplyMode,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let srv_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let captured_conn = Arc::clone(&captured);
                let canceled_conn = Arc::clone(&canceled);
                tokio::spawn(async move {
                    let mut buf = vec![0_u8; 16_384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
                    let (status, body) = if req.starts_with("GET ") {
                        (
                            "200 OK",
                            format!(
                                r#"{{"name":"Planner","description":"Ops planner","version":"1.0",
                                "supportedInterfaces":[{{"url":"http://{addr}/","protocolBinding":"JSONRPC","protocolVersion":"1.0"}}],
                                "capabilities":{{"streaming":false}},"defaultInputModes":["text/plain"],
                                "defaultOutputModes":["text/plain"],"skills":[]}}"#
                            ),
                        )
                    } else {
                        let body_start = req.find("\r\n\r\n").map_or(req.len(), |i| i + 4);
                        let raw_json = req.get(body_start..).unwrap_or("");
                        let parsed: serde_json::Value =
                            serde_json::from_str(raw_json).unwrap_or_default();
                        let rpc_resp = build_mock_rpc_response(&parsed, mode, &canceled_conn);
                        captured_conn.lock().await.push(parsed);
                        ("200 OK", rpc_resp.to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    sock.write_all(resp.as_bytes()).await.ok();
                });
            }
        });
        (addr, srv_task)
    }

    fn assert_room_turn_metadata(req: &serde_json::Value, room_id: &str, room_name: &str) {
        let sent_msg = req.get("params").and_then(|p| p.get("message")).unwrap();
        assert_eq!(
            sent_msg.get("contextId").and_then(|v| v.as_str()),
            Some(format!("room-{room_id}").as_str()),
            "contextId must be room-<room_id>"
        );
        let meta = sent_msg.get("metadata").unwrap();
        assert_eq!(
            meta.get(ROOM_METADATA_FLAG)
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "residuum.room metadata must be true"
        );
        assert_eq!(
            meta.get(ROOM_METADATA_ID)
                .and_then(serde_json::Value::as_str),
            Some(room_id),
            "residuum.room_id metadata must match"
        );
        assert_eq!(
            meta.get(ROOM_METADATA_NAME)
                .and_then(serde_json::Value::as_str),
            Some(room_name),
            "residuum.room_name metadata must match"
        );
    }

    #[tokio::test]
    async fn a2a_driver_delivers_context_id_room_metadata_and_closes_completed_session() {
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let (addr, srv_task) =
            spawn_mock_a2a_server(Arc::clone(&captured), MockReplyMode::CompletedTask).await;

        let hub = A2aClientHub::new_shared();
        hub.register_external(
            "ops/planner".to_owned(),
            format!("http://{addr}"),
            HashMap::new(),
            AgentSource::Sibling,
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let driver = A2aRoomDriver::new(Arc::clone(&hub), dir.path().to_path_buf());
        let cands = driver.candidates().await;
        assert_eq!(cands.len(), 1, "expected 1 remote candidate");
        let first_cand = cands.first().unwrap();
        assert_eq!(first_cand.seat_id, "a2a:ops/planner", "seat_id must match");
        assert_eq!(
            first_cand.kind,
            RoomSeatKind::SiblingA2a,
            "kind must be sibling_a2a"
        );
        assert!(first_cand.online, "registered card must be online");

        let out = driver
            .run_remote_turn(RemoteRoomTurnRequest {
                seat_id: "a2a:ops/planner".to_owned(),
                room_id: "room_01jcv8k9m2p4q7".to_owned(),
                room_name: "Ops Review".to_owned(),
                prompt: "Status?".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(
            out.content, "Ops reply ready",
            "remote reply text must match"
        );

        driver
            .close_remote_session("room_01jcv8k9m2p4q7", "a2a:ops/planner")
            .await
            .unwrap();

        let reqs = captured.lock().await;
        assert_eq!(
            reqs.len(),
            2,
            "expected turn SendMessage + close_remote_session SendMessage"
        );
        assert_room_turn_metadata(reqs.first().unwrap(), "room_01jcv8k9m2p4q7", "Ops Review");

        let close_req = reqs.get(1).unwrap();
        assert_eq!(
            close_req.get("method").and_then(|v| v.as_str()),
            Some("SendMessage"),
            "close_remote_session after Completed turn must use SendMessage"
        );
        let close_msg = close_req
            .get("params")
            .and_then(|p| p.get("message"))
            .unwrap();
        assert_eq!(
            close_msg.get("contextId").and_then(|v| v.as_str()),
            Some("room-room_01jcv8k9m2p4q7"),
            "close SendMessage must target the room contextId"
        );
        let close_meta = close_msg.get("metadata").unwrap();
        assert_eq!(
            close_meta
                .get(ROOM_METADATA_FLAG)
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "close SendMessage must set residuum.room = true"
        );
        assert_eq!(
            close_meta
                .get(ROOM_METADATA_ID)
                .and_then(serde_json::Value::as_str),
            Some("room_01jcv8k9m2p4q7"),
            "close SendMessage must set residuum.room_id"
        );
        assert_eq!(
            close_meta
                .get(ROOM_METADATA_CLOSE)
                .and_then(serde_json::Value::as_bool),
            Some(true),
            "close SendMessage must set residuum.room_close = true"
        );
        srv_task.abort();
    }

    #[tokio::test]
    async fn a2a_driver_cancels_in_flight_and_input_required_tasks() {
        // 1. InputRequired task: cancel_remote_turn sends CancelTask.
        let captured_input = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let (addr_input, srv_input) = spawn_mock_a2a_server(
            Arc::clone(&captured_input),
            MockReplyMode::InputRequiredTask,
        )
        .await;
        let hub_input = A2aClientHub::new_shared();
        hub_input
            .register_external(
                "ops/planner".to_owned(),
                format!("http://{addr_input}"),
                HashMap::new(),
                AgentSource::Sibling,
            )
            .await;
        let dir_input = tempfile::tempdir().unwrap();
        let driver_input = A2aRoomDriver::new(hub_input, dir_input.path().to_path_buf());
        driver_input
            .run_remote_turn(RemoteRoomTurnRequest {
                seat_id: "a2a:ops/planner".to_owned(),
                room_id: "room_01jcv8k9m2p4q7".to_owned(),
                room_name: "Ops Review".to_owned(),
                prompt: "Need input?".to_owned(),
            })
            .await
            .unwrap();
        driver_input
            .cancel_remote_turn("room_01jcv8k9m2p4q7", "a2a:ops/planner")
            .await
            .unwrap();
        {
            let reqs = captured_input.lock().await;
            let cancel_req = reqs
                .iter()
                .find(|r| r.get("method").and_then(|v| v.as_str()) == Some("CancelTask"))
                .expect("CancelTask must be sent for InputRequired task");
            assert_eq!(
                cancel_req
                    .get("params")
                    .and_then(|p| p.get("id"))
                    .and_then(|v| v.as_str()),
                Some("task-1"),
                "CancelTask must target the open task id"
            );
        }
        srv_input.abort();

        // 2. In-flight Working task: cancel_remote_turn sends CancelTask while polling.
        let captured_work = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let (addr_work, srv_work) = spawn_mock_a2a_server(
            Arc::clone(&captured_work),
            MockReplyMode::WorkingUntilCanceled,
        )
        .await;
        let hub_work = A2aClientHub::new_shared();
        hub_work
            .register_external(
                "ops/planner".to_owned(),
                format!("http://{addr_work}"),
                HashMap::new(),
                AgentSource::Sibling,
            )
            .await;
        let dir_work = tempfile::tempdir().unwrap();
        let driver_work = Arc::new(A2aRoomDriver::new(hub_work, dir_work.path().to_path_buf()));
        let driver_bg = Arc::clone(&driver_work);
        let turn_handle = tokio::spawn(async move {
            driver_bg
                .run_remote_turn(RemoteRoomTurnRequest {
                    seat_id: "a2a:ops/planner".to_owned(),
                    room_id: "room_01jcv8k9m2p4q7".to_owned(),
                    room_name: "Ops Review".to_owned(),
                    prompt: "Long job".to_owned(),
                })
                .await
        });

        residuum_core::testing::wait::until("in_flight task recorded", || async {
            (!driver_work.in_flight.lock().await.is_empty()).then_some(())
        })
        .await;

        driver_work
            .cancel_remote_turn("room_01jcv8k9m2p4q7", "a2a:ops/planner")
            .await
            .unwrap();
        let turn_res = turn_handle.await.unwrap();
        assert!(
            turn_res.is_err(),
            "canceled remote turn must resolve with turn canceled error"
        );
        let reqs = captured_work.lock().await;
        assert!(
            reqs.iter()
                .any(|r| r.get("method").and_then(|v| v.as_str()) == Some("CancelTask")),
            "CancelTask must hit mock server for in-flight Working task"
        );
        srv_work.abort();
    }

    async fn spawn_streaming_artifact_mock_server()
    -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv_task = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0_u8; 16_384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
                    if req.starts_with("GET ") {
                        let body = format!(
                            r#"{{"name":"Planner","description":"Streaming planner","version":"1.0",
                            "supportedInterfaces":[{{"url":"http://{addr}/","protocolBinding":"JSONRPC","protocolVersion":"1.0"}}],
                            "capabilities":{{"streaming":true}},"defaultInputModes":["text/plain"],
                            "defaultOutputModes":["text/plain"],"skills":[]}}"#
                        );
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        sock.write_all(resp.as_bytes()).await.ok();
                        return;
                    }
                    let mut raw_part = Part::raw(b"artifact-bytes".to_vec());
                    raw_part.filename = Some("../nested/summary.txt".to_owned());
                    raw_part.media_type = Some("text/plain".to_owned());
                    let ev1 = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
                            task_id: "task-stream-1".to_owned(),
                            context_id: "room-room_01jcv8k9m2p4q7".to_owned(),
                            status: a2a::TaskStatus {
                                state: TaskState::Working,
                                message: Some(A2aMessage::new(
                                    Role::Agent,
                                    vec![Part::text("Streamed working reply")],
                                )),
                                timestamp: None,
                            },
                            metadata: None,
                        })
                    });
                    let ev2 = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
                            task_id: "task-stream-1".to_owned(),
                            context_id: "room-room_01jcv8k9m2p4q7".to_owned(),
                            artifact: Artifact {
                                artifact_id: "art-1".to_owned(),
                                name: Some("report.bin".to_owned()),
                                description: None,
                                parts: vec![raw_part],
                                metadata: None,
                                extensions: None,
                            },
                            append: None,
                            last_chunk: Some(true),
                            metadata: None,
                        })
                    });
                    let ev3 = json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
                            task_id: "task-stream-1".to_owned(),
                            context_id: "room-room_01jcv8k9m2p4q7".to_owned(),
                            status: a2a::TaskStatus {
                                state: TaskState::Completed,
                                message: None,
                                timestamp: None,
                            },
                            metadata: None,
                        })
                    });
                    let sse = format!("data: {ev1}\n\ndata: {ev2}\n\ndata: {ev3}\n\n");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                        sse.len()
                    );
                    sock.write_all(resp.as_bytes()).await.ok();
                });
            }
        });
        (addr, srv_task)
    }

    #[tokio::test]
    async fn a2a_driver_persists_raw_artifact_and_uses_working_fallback_when_streaming() {
        let (addr, srv_task) = spawn_streaming_artifact_mock_server().await;

        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        hub.register_external(
            "ops/planner".to_owned(),
            format!("http://{addr}"),
            HashMap::new(),
            AgentSource::Sibling,
        )
        .await;

        let driver = A2aRoomDriver::new(hub, dir.path().to_path_buf());
        let out = driver
            .run_remote_turn(RemoteRoomTurnRequest {
                seat_id: "a2a:ops/planner".to_owned(),
                room_id: "room_01jcv8k9m2p4q7".to_owned(),
                room_name: "Stream Sync".to_owned(),
                prompt: "Deliver summary".to_owned(),
            })
            .await
            .unwrap();

        assert_eq!(out.content, "Streamed working reply");
        assert_eq!(out.attachments.len(), 1);
        let att = out.attachments.first().unwrap();
        assert_eq!(att.filename, "summary.txt");
        assert_eq!(att.mime_type, "text/plain");
        assert_eq!(att.size, 14);
        assert_eq!(att.id.len(), 32);
        assert!(
            att.id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
        assert_eq!(
            att.url,
            format!("/api/hub/rooms/room_01jcv8k9m2p4q7/attachments/{}", att.id)
        );
        let saved_path = dir
            .path()
            .join("room_01jcv8k9m2p4q7")
            .join("attachments")
            .join(&att.id)
            .join("summary.txt");
        assert_eq!(std::fs::read(&saved_path).unwrap(), b"artifact-bytes");

        srv_task.abort();
    }
}

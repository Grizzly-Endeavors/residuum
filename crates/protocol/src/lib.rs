pub mod frames;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use residuum_bus::session::{SessionCategory, SessionInfo, SessionState};
use residuum_bus::usage::SessionUsageTotals;
use residuum_bus::workspace_change::{WorkspaceChange, WorkspaceResyncReason};
use residuum_bus::{
    PulseFireChange, PulseFireEvent, PulseFireOutcome, PulseFirePhase, PulseFireRun,
    PulseFireStatus, PulseOverlap, ReloadOutcome, ReloadedFiles, TurnOrigin,
};
use residuum_inference::{AgentSender, ImageData, MessageAttachment, MessageSender};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMessage {
    SendMessage {
        id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    SetVerbose {
        enabled: bool,
    },
    Ping,
    ResyncTurn,
    Reload,
    ServerCommand {
        name: String,
        args: Option<String>,
    },
    InboxAdd {
        body: String,
    },
    Cancel {
        reply_to: String,
    },
    SessionSendMessage {
        id: String,
        address: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        message_id: Option<String>,
    },
    SessionStop {
        id: String,
        address: String,
    },
    SendQueuedNow {
        id: String,
    },
    WithdrawQueued {
        id: String,
    },
    RetryTurn {
        id: String,
    },
    WatchWorkspace {
        prefixes: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueuedMessage {
    pub id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sender: Option<MessageSender>,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub address: String,
    pub run_id: String,
    pub category: SessionCategory,
    pub source_label: String,
    pub state: SessionState,
    pub spawner: Option<String>,
    pub depth: u32,
    pub purpose: String,
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    #[ts(type = "string | null")]
    pub completed_at: Option<DateTime<Utc>>,
    pub episode_id: Option<String>,
    #[serde(default)]
    pub memory_pending: bool,
    pub interrupted: bool,
    pub usage: SessionUsageTotals,
    pub outcome: Option<SessionRunStatus>,
    pub error: Option<String>,
    #[serde(default)]
    pub error_details: Option<String>,
    pub overlap: Option<residuum_bus::PulseOverlap>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub resolved_model: Option<String>,
}

/// `GET /api/agents/{name}/sessions` response: live sessions plus one page of completed
/// runs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionListResponse {
    pub live: Vec<SessionSummary>,
    pub completed: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OutboundA2aTaskSummary {
    pub task_id: String,
    pub agent: String,
    pub sender_address: String,
    pub state: String,
    pub status_text: Option<String>,
    pub open: bool,
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    #[ts(type = "string | null")]
    pub unreachable_since: Option<DateTime<Utc>>,
}

impl From<PulseFireEvent> for ServerMessage {
    fn from(event: PulseFireEvent) -> Self {
        let last_outcome = match &event.change {
            PulseFireChange::Finished {
                outcome,
                session_run_id,
            } => Some(ScheduledRunOutcome {
                run_id: session_run_id.clone(),
                ..outcome.into()
            }),
            PulseFireChange::Started
            | PulseFireChange::SessionRequested
            | PulseFireChange::SessionStarted
            | PulseFireChange::Ended => None,
        };
        Self::PulseFireChanged {
            pulse: event.pulse,
            fire_id: event.fire_id,
            current_run: event.current_run.map(Into::into),
            last_outcome,
        }
    }
}

impl From<&residuum_bus::remote_task::TrackedTask> for OutboundA2aTaskSummary {
    fn from(task: &residuum_bus::remote_task::TrackedTask) -> Self {
        Self {
            task_id: task.task_id.clone(),
            agent: task.agent.clone(),
            sender_address: task.sender_address.clone(),
            state: task.state.clone(),
            status_text: task.last_status_text.clone(),
            open: task.is_open(),
            started_at: task.created_at,
            unreachable_since: task.first_unreachable_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionRunStatus {
    Completed,
    Cancelled,
    Failed,
}

impl SessionRunStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

impl From<PulseFireStatus> for SessionRunStatus {
    fn from(status: PulseFireStatus) -> Self {
        match status {
            PulseFireStatus::Ok => Self::Completed,
            PulseFireStatus::Failed => Self::Failed,
            PulseFireStatus::Stopped => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduledRunOutcome {
    pub status: SessionRunStatus,
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<String>,
}

impl From<&PulseFireOutcome> for ScheduledRunOutcome {
    fn from(outcome: &PulseFireOutcome) -> Self {
        Self {
            status: outcome.status.into(),
            at: outcome.at,
            error: outcome.error.clone(),
            run_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduledCurrentRun {
    pub fire_id: Option<String>,
    pub phase: PulseFirePhase,
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    pub address: Option<String>,
    pub run_id: Option<String>,
    pub overlap: Option<PulseOverlap>,
}

impl From<PulseFireRun> for ScheduledCurrentRun {
    fn from(run: PulseFireRun) -> Self {
        Self {
            fire_id: run.fire_id,
            phase: run.phase,
            started_at: run.started_at,
            address: run.address.map(|address| address.to_string()),
            run_id: run.run_id,
            overlap: run.overlap,
        }
    }
}

impl From<&SessionInfo> for ScheduledCurrentRun {
    fn from(info: &SessionInfo) -> Self {
        Self {
            fire_id: None,
            phase: PulseFirePhase::Session,
            started_at: info.started_at,
            address: Some(info.address.to_string()),
            run_id: Some(info.run_id.clone()),
            overlap: info.overlap.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PulseInfo {
    pub name: String,
    pub enabled: bool,
    pub schedule: Option<String>,
    pub days: Option<Vec<String>>,
    pub active_hours: Option<String>,
    pub agent: Option<String>,
    pub model_tier: Option<String>,
    #[ts(type = "string | null")]
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<ScheduledRunOutcome>,
    pub current_run: Option<ScheduledCurrentRun>,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActionInfo {
    pub id: String,
    pub name: String,
    #[ts(type = "string")]
    pub run_at: DateTime<Utc>,
    pub agent: Option<String>,
    pub model_tier: Option<String>,
    pub problem: Option<String>,
    pub current_run: Option<ScheduledCurrentRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionDeliveryOutcome {
    Live,
    Resumed,
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionCommandErrorCode {
    InvalidRequest,
    UnknownAddress,
    Busy,
    NotLive,
    DeliveryFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkbenchInfo {
    pub port: Option<u16>,
    pub unavailable_reason: Option<String>,
    pub relay: Option<WorkbenchRelayOrigins>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkbenchRelayOrigins {
    pub ui_origin: String,
    pub artifacts_origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PostTurnActivityKind {
    Memory,
    Subconscious,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TurnInProgress {
    pub reply_to: String,
    pub started_at: String,
    pub now: String,
    pub frames: Vec<SnapshotFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnapshotFrame {
    pub at: String,
    pub frame: ServerMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMessage {
    TurnStarted {
        reply_to: String,
        origin: TurnOrigin,
    },
    TurnEnded {
        reply_to: String,
    },
    TurnUsage {
        reply_to: String,
        output_tokens: u32,
        has_usage: bool,
        #[serde(default)]
        tool_calls: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_totals: Option<SessionUsageTotals>,
    },
    PostTurnActivity {
        kind: PostTurnActivityKind,
        active: bool,
    },
    ToolCall {
        reply_to: String,
        call: u32,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        server: Option<String>,
    },
    ToolResult {
        reply_to: String,
        tool_call_id: String,
        name: String,
        output: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        auto_mode: Option<residuum_bus::auto_mode::AutoModeVerdict>,
    },
    Response {
        reply_to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        call: Option<u32>,
        endpoint: String,
        content: String,
    },
    FileAttachment {
        reply_to: String,
        filename: String,
        mime_type: String,
        #[ts(type = "number")]
        size: u64,
        /// URL to fetch the file, under the agent's own routes: a durable
        /// workspace-relative link (e.g.
        /// "/api/agents/scout/files/workspace?path=...") for a file inside
        /// the workspace, else an expiring token link (e.g.
        /// "/api/agents/scout/files/{id}"). Use it as given.
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    BroadcastResponse {
        reply_to: String,
        call: u32,
        content: String,
    },
    UserMessage {
        id: String,
        turn_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageData>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<MessageAttachment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        sender: Option<MessageSender>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent_sender: Option<AgentSender>,
        endpoint: String,
    },
    TextDelta {
        reply_to: String,
        call: u32,
        text: String,
    },
    ThinkingDelta {
        reply_to: String,
        call: u32,
        text: String,
    },
    Thinking {
        reply_to: String,
        call: u32,
        content: String,
    },
    StreamRestart {
        reply_to: String,
        call: u32,
    },
    Error {
        reply_to: Option<String>,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        details: Option<String>,
    },
    MessageRefused {
        id: String,
        message: String,
    },
    RetryRefused {
        id: String,
        message: String,
    },
    Pong,
    TurnSnapshot {
        turn: Option<TurnInProgress>,
        queued: Vec<QueuedMessage>,
    },
    MessageQueued {
        message: QueuedMessage,
    },
    MessageWithdrawn {
        id: String,
    },
    QueuedMessageUnavailable {
        id: String,
    },
    Reloading,
    Notice {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reload: Option<ReloadOutcome>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reload_of: Option<ReloadedFiles>,
    },
    InlineOutput {
        message: String,
    },
    SessionStarted {
        session: SessionSummary,
    },
    SessionStateChanged {
        address: String,
        run_id: String,
        state: SessionState,
    },
    SessionCompleted {
        address: String,
        run_id: String,
        status: SessionRunStatus,
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        error_details: Option<String>,
        memory_pending: bool,
    },
    SessionMemoryMerged {
        address: String,
        run_id: String,
        episode_id: Option<String>,
    },
    SessionStartFailed {
        address: String,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        error_details: Option<String>,
    },
    SessionTurnStarted {
        address: String,
        run_id: String,
        turn_id: String,
    },
    SessionTurnEnded {
        address: String,
        run_id: String,
        turn_id: String,
    },
    SessionUserMessage {
        address: String,
        run_id: String,
        id: String,
        turn_id: String,
        content: String,
        from: String,
        from_category: String,
    },
    SessionToolCall {
        address: String,
        run_id: String,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        server: Option<String>,
    },
    SessionToolResult {
        address: String,
        run_id: String,
        tool_call_id: String,
        name: String,
        output: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        auto_mode: Option<residuum_bus::auto_mode::AutoModeVerdict>,
    },
    SessionTurnUsage {
        address: String,
        run_id: String,
        output_tokens: u32,
        has_usage: bool,
        #[serde(default)]
        tool_calls: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_totals: Option<SessionUsageTotals>,
    },
    SessionBroadcastResponse {
        address: String,
        run_id: String,
        content: String,
    },
    SessionResponse {
        address: String,
        run_id: String,
        turn_id: String,
        content: String,
    },
    SessionError {
        address: String,
        run_id: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        details: Option<String>,
    },
    SessionMessageDelivered {
        id: String,
        address: String,
        message_id: String,
        outcome: SessionDeliveryOutcome,
    },
    SessionStopRequested {
        id: String,
        address: String,
    },
    SessionCommandFailed {
        id: String,
        address: String,
        code: SessionCommandErrorCode,
        message: String,
    },
    SessionOutboundA2aTask {
        task: OutboundA2aTaskSummary,
    },
    PulseFireChanged {
        pulse: String,
        fire_id: String,
        current_run: Option<ScheduledCurrentRun>,
        last_outcome: Option<ScheduledRunOutcome>,
    },
    ArtifactUpdated {
        name: String,
    },
    ArtifactRemoved {
        name: String,
    },
    WorkspaceChanged {
        changes: Vec<WorkspaceChange>,
    },
    WorkspaceResync {
        reason: WorkspaceResyncReason,
    },
    WorkspaceWatchUnavailable {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished_fire(session_run_id: Option<&str>) -> PulseFireEvent {
        PulseFireEvent {
            pulse: "watch".to_string(),
            fire_id: "fire-1".to_string(),
            change: PulseFireChange::Finished {
                outcome: PulseFireOutcome {
                    status: PulseFireStatus::Ok,
                    at: DateTime::parse_from_rfc3339("2099-01-01T12:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    error: None,
                },
                session_run_id: session_run_id.map(str::to_string),
            },
            current_run: None,
        }
    }

    #[test]
    fn a_finished_fire_s_frame_names_its_session_run_only_when_it_had_one() {
        let last_outcome = |id| {
            serde_json::to_value(ServerMessage::from(finished_fire(id)))
                .unwrap()
                .get("last_outcome")
                .cloned()
        };

        assert_eq!(
            last_outcome(Some("run-1")),
            Some(serde_json::json!({
                "status": "completed",
                "at": "2099-01-01T12:00:00Z",
                "error": null,
                "run_id": "run-1",
            }))
        );
        assert_eq!(
            last_outcome(None),
            Some(serde_json::json!({
                "status": "completed",
                "at": "2099-01-01T12:00:00Z",
                "error": null,
            })),
            "a fire that ran no session has no run_id"
        );
    }

    #[test]
    fn client_message_deserialize_send_message() {
        let json = r#"{"type":"send_message","id":"abc-123","content":"hello"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                &msg,
                ClientMessage::SendMessage { id, content, attachments }
                    if id == "abc-123" && content == "hello" && attachments.is_empty()
            ),
            "should deserialize to SendMessage with correct fields"
        );
    }

    #[test]
    fn client_message_deserialize_set_verbose() {
        let json = r#"{"type":"set_verbose","enabled":true}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&msg, ClientMessage::SetVerbose { enabled } if *enabled),
            "should deserialize to SetVerbose with enabled=true"
        );
    }

    #[test]
    fn client_message_deserialize_ping() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Ping),
            "should deserialize to Ping"
        );
    }

    #[test]
    fn server_message_serialize_response() {
        let msg = ServerMessage::Response {
            reply_to: "id-1".to_string(),
            call: Some(2),
            endpoint: "telegram".to_string(),
            content: "hello back".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "response",
                "reply_to": "id-1",
                "call": 2,
                "endpoint": "telegram",
                "content": "hello back",
            })
        );
    }

    #[test]
    fn a_response_no_model_call_wrote_has_no_call() {
        let msg = ServerMessage::Response {
            reply_to: String::new(),
            call: None,
            endpoint: "ws".to_string(),
            content: "posted with send_message".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("call").is_none(), "{json}");
    }

    #[test]
    fn server_message_serialize_turn_started_with_its_origin() {
        let msg = ServerMessage::TurnStarted {
            reply_to: "turn-1".to_string(),
            origin: TurnOrigin {
                endpoint: "telegram".to_string(),
                sender: Some(MessageSender {
                    name: "Bear".to_string(),
                    id: "42".to_string(),
                    interface: "telegram".to_string(),
                    location: None,
                }),
                visibility: residuum_memory::types::Visibility::User,
            },
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "turn_started",
                "reply_to": "turn-1",
                "origin": {
                    "endpoint": "telegram",
                    "sender": { "name": "Bear", "id": "42", "interface": "telegram" },
                    "visibility": "user",
                },
            })
        );
    }

    #[test]
    fn server_message_serialize_user_message() {
        let msg = ServerMessage::UserMessage {
            id: "m1".to_string(),
            turn_id: "turn-1".to_string(),
            content: "hello".to_string(),
            images: Vec::new(),
            attachments: Vec::new(),
            sender: None,
            agent_sender: None,
            endpoint: "ws".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "user_message",
                "id": "m1",
                "turn_id": "turn-1",
                "content": "hello",
                "endpoint": "ws",
            })
        );
    }

    #[test]
    fn server_message_serialize_user_message_attachments() {
        let msg = ServerMessage::UserMessage {
            id: "m2".to_string(),
            turn_id: "turn-2".to_string(),
            content: "see attached".to_string(),
            images: Vec::new(),
            attachments: vec![MessageAttachment {
                item_id: "item-1".to_string(),
                filename: "notes.md".to_string(),
                size: 12,
                content_type: "text/markdown".to_string(),
                path: "inbox/agent/attachments/item-1/notes.md".to_string(),
            }],
            sender: None,
            agent_sender: None,
            endpoint: "chat".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "user_message",
                "id": "m2",
                "turn_id": "turn-2",
                "content": "see attached",
                "attachments": [{
                    "item_id": "item-1",
                    "filename": "notes.md",
                    "size": 12,
                    "content_type": "text/markdown",
                    "path": "inbox/agent/attachments/item-1/notes.md",
                }],
                "endpoint": "chat",
            })
        );
    }

    #[test]
    fn queued_message_serializes_its_attachments() {
        let queued = QueuedMessage {
            id: "m4".to_string(),
            content: "see attached".to_string(),
            images: Vec::new(),
            attachments: vec![MessageAttachment {
                item_id: "item-1".to_string(),
                filename: "notes.md".to_string(),
                size: 12,
                content_type: "text/markdown".to_string(),
                path: "inbox/agent/attachments/item-1/notes.md".to_string(),
            }],
            sender: None,
            endpoint: "chat".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&queued).unwrap(),
            serde_json::json!({
                "id": "m4",
                "content": "see attached",
                "attachments": [{
                    "item_id": "item-1",
                    "filename": "notes.md",
                    "size": 12,
                    "content_type": "text/markdown",
                    "path": "inbox/agent/attachments/item-1/notes.md",
                }],
                "endpoint": "chat",
            })
        );
    }

    #[test]
    fn server_message_user_message_without_attachments_omits_the_field() {
        let msg = ServerMessage::UserMessage {
            id: "m3".to_string(),
            turn_id: "turn-3".to_string(),
            content: "plain".to_string(),
            images: Vec::new(),
            attachments: Vec::new(),
            sender: None,
            agent_sender: None,
            endpoint: "ws".to_string(),
        };
        let value = serde_json::to_value(&msg).unwrap();
        assert!(
            value.get("attachments").is_none(),
            "an empty attachments list should be skipped: {value}"
        );
    }

    #[test]
    fn server_message_serialize_teammate_user_message() {
        let msg = ServerMessage::UserMessage {
            id: "bg-1".to_string(),
            turn_id: "bg-1".to_string(),
            content: "[Message from teammate agent:scout, not the user.]\nhi".to_string(),
            images: Vec::new(),
            attachments: Vec::new(),
            sender: None,
            agent_sender: Some(AgentSender {
                address: "agent:scout".to_string(),
                category: "teammate".to_string(),
                run_id: None,
            }),
            endpoint: "background".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "user_message",
                "id": "bg-1",
                "turn_id": "bg-1",
                "content": "[Message from teammate agent:scout, not the user.]\nhi",
                "agent_sender": { "address": "agent:scout", "category": "teammate" },
                "endpoint": "background",
            })
        );
    }

    #[test]
    fn server_message_serialize_session_user_message_with_its_run() {
        let msg = ServerMessage::UserMessage {
            id: "bg-2".to_string(),
            turn_id: "bg-2".to_string(),
            content: "[Agent Message from spawned-research (spawned)]\nfound it".to_string(),
            images: Vec::new(),
            attachments: Vec::new(),
            sender: None,
            agent_sender: Some(AgentSender {
                address: "spawned-research".to_string(),
                category: "spawned".to_string(),
                run_id: Some("run-1".to_string()),
            }),
            endpoint: "background".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "user_message",
                "id": "bg-2",
                "turn_id": "bg-2",
                "content": "[Agent Message from spawned-research (spawned)]\nfound it",
                "agent_sender": {
                    "address": "spawned-research",
                    "category": "spawned",
                    "run_id": "run-1",
                },
                "endpoint": "background",
            })
        );
    }

    #[test]
    fn server_message_serialize_streaming_frames() {
        let frames = [
            (
                ServerMessage::TextDelta {
                    reply_to: "t".to_string(),
                    call: 1,
                    text: "Hel".to_string(),
                },
                serde_json::json!({ "type": "text_delta", "reply_to": "t", "call": 1, "text": "Hel" }),
            ),
            (
                ServerMessage::ThinkingDelta {
                    reply_to: "t".to_string(),
                    call: 1,
                    text: "hm".to_string(),
                },
                serde_json::json!({ "type": "thinking_delta", "reply_to": "t", "call": 1, "text": "hm" }),
            ),
            (
                ServerMessage::Thinking {
                    reply_to: "t".to_string(),
                    call: 1,
                    content: "hmm, so".to_string(),
                },
                serde_json::json!({ "type": "thinking", "reply_to": "t", "call": 1, "content": "hmm, so" }),
            ),
            (
                ServerMessage::StreamRestart {
                    reply_to: "t".to_string(),
                    call: 1,
                },
                serde_json::json!({ "type": "stream_restart", "reply_to": "t", "call": 1 }),
            ),
        ];
        for (frame, expected) in frames {
            assert_eq!(serde_json::to_value(&frame).unwrap(), expected);
        }
    }

    #[test]
    fn server_message_serialize_tool_frames_name_their_turn_and_call() {
        let call = ServerMessage::ToolCall {
            reply_to: "t".to_string(),
            call: 3,
            id: "tc".to_string(),
            name: "exec".to_string(),
            arguments: serde_json::json!({}),
            server: None,
        };
        let result = ServerMessage::ToolResult {
            reply_to: "t".to_string(),
            tool_call_id: "tc".to_string(),
            name: "exec".to_string(),
            output: "ok".to_string(),
            is_error: false,
            auto_mode: None,
        };
        let call = serde_json::to_value(&call).unwrap();
        let result = serde_json::to_value(&result).unwrap();
        assert_eq!(call.get("reply_to"), Some(&serde_json::json!("t")));
        assert_eq!(call.get("call"), Some(&serde_json::json!(3)));
        assert_eq!(result.get("reply_to"), Some(&serde_json::json!("t")));
    }

    #[test]
    fn server_message_serialize_pong() {
        let msg = ServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#, "pong should serialize cleanly");
    }

    #[test]
    fn server_message_serialize_error() {
        let msg = ServerMessage::Error {
            reply_to: Some("id-1".to_string()),
            message: "something failed".to_string(),
            details: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"error\""), "should have type tag");
        assert!(
            !json.contains("\"details\""),
            "a None details must not appear in the JSON at all"
        );
        assert!(
            json.contains("\"reply_to\":\"id-1\""),
            "should have reply_to field"
        );
        assert!(
            json.contains("\"message\":\"something failed\""),
            "should have message field"
        );
    }

    #[test]
    fn server_message_serialize_message_refused() {
        let msg = ServerMessage::MessageRefused {
            id: "id-1".to_string(),
            message: "An attached file wasn't available anymore, so it's uploading again. Send once it's ready.".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "message_refused",
                "id": "id-1",
                "message": "An attached file wasn't available anymore, so it's uploading again. Send once it's ready.",
            }),
            "message_refused should carry the send's id and the reason"
        );
    }

    #[test]
    fn server_message_serialize_retry_refused() {
        let msg = ServerMessage::RetryRefused {
            id: "id-1".to_string(),
            message: "A reply is still being written. Try again once it finishes.".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "retry_refused",
                "id": "id-1",
                "message": "A reply is still being written. Try again once it finishes.",
            }),
            "retry_refused should carry the retry's id and the reason"
        );
    }

    #[test]
    fn client_message_deserialize_retry_turn() {
        let json = r#"{"type":"retry_turn","id":"msg-7"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::RetryTurn { ref id } if id == "msg-7"),
            "should deserialize to RetryTurn carrying the failed turn's id"
        );
    }

    #[test]
    fn client_message_deserialize_reload() {
        let json = r#"{"type":"reload"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(msg, ClientMessage::Reload),
            "should deserialize to Reload"
        );
    }

    #[test]
    fn server_message_serialize_turn_ended() {
        let msg = ServerMessage::TurnEnded {
            reply_to: "id-1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"turn_ended\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"reply_to\":\"id-1\""),
            "should have reply_to field"
        );
    }

    #[test]
    fn server_message_serialize_reloading() {
        let msg = ServerMessage::Reloading;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json, r#"{"type":"reloading"}"#,
            "reloading should serialize cleanly"
        );
    }

    #[test]
    fn client_message_deserialize_server_command() {
        let json = r#"{"type":"server_command","name":"observe","args":null}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                &msg,
                ClientMessage::ServerCommand { name, args }
                    if name == "observe" && args.is_none()
            ),
            "should deserialize to ServerCommand with name=observe"
        );
    }

    #[test]
    fn client_message_deserialize_server_command_with_args() {
        let json = r#"{"type":"server_command","name":"custom","args":"some arg"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                &msg,
                ClientMessage::ServerCommand { name, args }
                    if name == "custom" && args.as_deref() == Some("some arg")
            ),
            "should deserialize ServerCommand with args"
        );
    }

    #[test]
    fn server_message_serialize_notice() {
        let msg = ServerMessage::Notice {
            message: "observed successfully".to_string(),
            reload: None,
            reload_of: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"notice\""), "should have type tag");
        assert!(
            json.contains("\"message\":\"observed successfully\""),
            "should have message field"
        );
        assert!(
            !json.contains("reload"),
            "a plain notice carries no reload field"
        );
    }

    #[test]
    fn server_message_serialize_reload_notice() {
        for (outcome, word) in [
            (ReloadOutcome::Reloaded, "reloaded"),
            (ReloadOutcome::Unchanged, "unchanged"),
            (ReloadOutcome::Failed, "failed"),
        ] {
            let msg = ServerMessage::Notice {
                message: "configuration reloaded: models".to_string(),
                reload: Some(outcome),
                reload_of: Some(ReloadedFiles::AgentConfig),
            };
            assert_eq!(
                serde_json::to_value(&msg).unwrap(),
                serde_json::json!({
                    "type": "notice",
                    "message": "configuration reloaded: models",
                    "reload": word,
                    "reload_of": "agent_config",
                })
            );
        }
    }

    #[test]
    fn server_message_serialize_broadcast_response() {
        let msg = ServerMessage::BroadcastResponse {
            reply_to: "id-1".to_string(),
            call: 0,
            content: "checking that for you".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"broadcast_response\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"content\":\"checking that for you\""),
            "should have content field"
        );
    }

    #[test]
    fn client_message_serialize_server_command() {
        let msg = ClientMessage::ServerCommand {
            name: "reflect".to_string(),
            args: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"server_command\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"name\":\"reflect\""),
            "should have name field"
        );
    }

    #[test]
    fn client_message_deserialize_inbox_add() {
        let json = r#"{"type":"inbox_add","body":"remember to deploy tomorrow"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&msg, ClientMessage::InboxAdd { body } if body == "remember to deploy tomorrow"),
            "should deserialize to InboxAdd with correct body"
        );
    }

    #[test]
    fn client_message_serialize_server_command_with_args() {
        let msg = ClientMessage::ServerCommand {
            name: "context".to_string(),
            args: Some("verbose".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"server_command\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"args\":\"verbose\""),
            "should have args field"
        );
    }

    #[test]
    fn client_message_serialize_inbox_add() {
        let msg = ClientMessage::InboxAdd {
            body: "hello world".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"inbox_add\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"body\":\"hello world\""),
            "should have body field"
        );
    }

    #[test]
    fn client_message_deserialize_cancel() {
        let json = r#"{"type":"cancel","reply_to":"abc-123"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&msg, ClientMessage::Cancel { reply_to } if reply_to == "abc-123"),
            "should deserialize to Cancel with correct reply_to"
        );
    }

    #[test]
    fn client_message_serialize_cancel_roundtrip() {
        let msg = ClientMessage::Cancel {
            reply_to: "turn-42".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"cancel\""), "should have type tag");
        assert!(
            json.contains("\"reply_to\":\"turn-42\""),
            "should have reply_to field"
        );
        let deserialized: ClientMessage = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(&deserialized, ClientMessage::Cancel { reply_to } if reply_to == "turn-42"),
            "should survive a serialization round-trip"
        );
    }

    #[test]
    fn client_message_invalid_type_fails() {
        let json = r#"{"type":"unknown_type"}"#;
        let result = serde_json::from_str::<ClientMessage>(json);
        assert!(result.is_err(), "unknown type should fail to deserialize");
    }

    #[test]
    fn client_message_send_message_with_attachments_roundtrip() {
        let msg = ClientMessage::SendMessage {
            id: "att-1".to_string(),
            content: "look at this".to_string(),
            attachments: vec!["0123456789abcdef0123456789abcdef".to_string()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"attachments\":[\"0123456789abcdef0123456789abcdef\"]"),
            "attachments field should be serialized when non-empty, got {json}"
        );
        let deserialized: ClientMessage = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(
                &deserialized,
                ClientMessage::SendMessage { id, content, attachments }
                    if id == "att-1" && content == "look at this" && attachments.len() == 1
            ),
            "should survive serialization round-trip with attachments"
        );
    }

    #[test]
    fn client_message_send_message_without_attachments_omits_the_field() {
        let msg = ClientMessage::SendMessage {
            id: "plain-1".to_string(),
            content: "hi".to_string(),
            attachments: Vec::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("attachments"),
            "an empty attachments list should not be serialized, got {json}"
        );
    }

    #[test]
    fn server_message_serialize_file_attachment() {
        let msg = ServerMessage::FileAttachment {
            reply_to: "id-1".to_string(),
            filename: "report.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 1024,
            url: "/api/files/abc-123".to_string(),
            caption: Some("Here's the report".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"file_attachment\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"filename\":\"report.pdf\""),
            "should have filename"
        );
        assert!(
            json.contains("\"url\":\"/api/files/abc-123\""),
            "should have url"
        );
    }

    #[test]
    fn server_message_serialize_file_attachment_no_caption() {
        let msg = ServerMessage::FileAttachment {
            reply_to: "id-2".to_string(),
            filename: "voice.mp3".to_string(),
            mime_type: "audio/mpeg".to_string(),
            size: 2048,
            url: "/api/files/def-456".to_string(),
            caption: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("\"caption\""),
            "caption should be skipped when None"
        );
    }

    #[test]
    fn client_message_deserialize_session_commands() {
        let send: ClientMessage = serde_json::from_str(
            r#"{"type":"session_send_message","id":"c1","address":"spawned-a-0001","content":"hi"}"#,
        )
        .unwrap();
        assert!(matches!(
            &send,
            ClientMessage::SessionSendMessage { id, address, content, message_id: None }
                if id == "c1" && address == "spawned-a-0001" && content == "hi"
        ));
        let with_message_id: ClientMessage = serde_json::from_str(
            r#"{"type":"session_send_message","id":"c3","address":"spawned-a-0001","content":"hi","message_id":"m-1"}"#,
        )
        .unwrap();
        assert!(matches!(
            &with_message_id,
            ClientMessage::SessionSendMessage { id, message_id: Some(message_id), .. }
                if id == "c3" && message_id == "m-1"
        ));

        let stop: ClientMessage =
            serde_json::from_str(r#"{"type":"session_stop","id":"c2","address":"spawned-a-0001"}"#)
                .unwrap();
        assert!(matches!(
            &stop,
            ClientMessage::SessionStop { id, address } if id == "c2" && address == "spawned-a-0001"
        ));
    }

    #[test]
    fn server_message_serialize_session_command_replies() {
        let delivered = ServerMessage::SessionMessageDelivered {
            id: "c1".into(),
            address: "spawned-a-0001".into(),
            message_id: "web-1".into(),
            outcome: SessionDeliveryOutcome::Resumed,
        };
        assert_eq!(
            serde_json::to_value(&delivered).unwrap(),
            serde_json::json!({
                "type": "session_message_delivered",
                "id": "c1",
                "address": "spawned-a-0001",
                "message_id": "web-1",
                "outcome": "resumed",
            })
        );

        let failed = ServerMessage::SessionCommandFailed {
            id: "c2".into(),
            address: "spawned-a-0001".into(),
            code: SessionCommandErrorCode::UnknownAddress,
            message: "no such session".into(),
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            serde_json::json!({
                "type": "session_command_failed",
                "id": "c2",
                "address": "spawned-a-0001",
                "code": "unknown_address",
                "message": "no such session",
            })
        );
    }

    #[test]
    fn server_message_serialize_session_start_failed() {
        let msg = ServerMessage::SessionStartFailed {
            address: "spawned-session-1a2b".into(),
            error: "The session spawned-session-1a2b couldn't start.".into(),
            error_details: Some("failed to build provider chain: no key".into()),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "session_start_failed",
                "address": "spawned-session-1a2b",
                "error": "The session spawned-session-1a2b couldn't start.",
                "error_details": "failed to build provider chain: no key",
            })
        );

        let without_details = ServerMessage::SessionStartFailed {
            address: "spawned-session-1a2b".into(),
            error: "The session spawned-session-1a2b couldn't start.".into(),
            error_details: None,
        };
        assert_eq!(
            serde_json::to_value(&without_details).unwrap(),
            serde_json::json!({
                "type": "session_start_failed",
                "address": "spawned-session-1a2b",
                "error": "The session spawned-session-1a2b couldn't start.",
            }),
            "a None error_details must not appear in the JSON at all"
        );
    }

    #[test]
    fn server_message_serialize_session_state_changed() {
        let msg = ServerMessage::SessionStateChanged {
            address: "scheduled-pulse-0001".into(),
            run_id: "run-1".into(),
            state: SessionState::Idle,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "session_state_changed",
                "address": "scheduled-pulse-0001",
                "run_id": "run-1",
                "state": "idle",
            })
        );
    }

    #[test]
    fn workspace_watch_frames_round_trip_the_documented_shapes() {
        let watch: ClientMessage =
            serde_json::from_str(r#"{"type":"watch_workspace","prefixes":["wiki",""]}"#).unwrap();
        assert!(
            matches!(&watch, ClientMessage::WatchWorkspace { prefixes } if prefixes == &["wiki", ""])
        );

        let changed = ServerMessage::WorkspaceChanged {
            changes: vec![WorkspaceChange {
                path: "wiki/a.md".into(),
                kind: residuum_bus::workspace_change::WorkspaceChangeKind::Created,
            }],
        };
        assert_eq!(
            serde_json::to_value(&changed).unwrap(),
            serde_json::json!({
                "type": "workspace_changed",
                "changes": [{ "path": "wiki/a.md", "kind": "created" }],
            })
        );
        let resync = ServerMessage::WorkspaceResync {
            reason: WorkspaceResyncReason::WatcherRestarted,
        };
        assert_eq!(
            serde_json::to_value(&resync).unwrap(),
            serde_json::json!({ "type": "workspace_resync", "reason": "watcher_restarted" })
        );
    }

    #[test]
    fn server_message_serialize_turn_usage_without_session_totals() {
        let msg = ServerMessage::TurnUsage {
            reply_to: "id-1".to_string(),
            output_tokens: 42,
            has_usage: true,
            tool_calls: 3,
            session_totals: None,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "turn_usage",
                "reply_to": "id-1",
                "output_tokens": 42,
                "has_usage": true,
                "tool_calls": 3,
            }),
            "session_totals should be omitted, not serialized as null"
        );
    }

    #[test]
    fn server_message_serialize_turn_usage_with_session_totals() {
        let msg = ServerMessage::TurnUsage {
            reply_to: "id-1".to_string(),
            output_tokens: 42,
            has_usage: true,
            tool_calls: 3,
            session_totals: Some(SessionUsageTotals {
                input_tokens: 100,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 42,
                context_tokens: Some(100),
                tool_calls: 3,
            }),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "turn_usage",
                "reply_to": "id-1",
                "output_tokens": 42,
                "has_usage": true,
                "tool_calls": 3,
                "session_totals": {
                    "input_tokens": 100,
                    "cache_read_tokens": 0,
                    "cache_write_tokens": 0,
                    "output_tokens": 42,
                    "context_tokens": 100,
                    "tool_calls": 3,
                },
            })
        );
    }

    #[test]
    fn server_message_deserialize_turn_usage_without_tool_calls_field() {
        let old_payload = serde_json::json!({
            "type": "turn_usage",
            "reply_to": "id-1",
            "output_tokens": 42,
            "has_usage": true,
        });
        let msg: ServerMessage = serde_json::from_value(old_payload).unwrap();
        assert!(matches!(
            msg,
            ServerMessage::TurnUsage {
                tool_calls: 0,
                output_tokens: 42,
                has_usage: true,
                ..
            }
        ));
    }

    #[test]
    fn server_message_serialize_session_turn_usage() {
        let msg = ServerMessage::SessionTurnUsage {
            address: "spawned-x-0001".into(),
            run_id: "run-1".into(),
            output_tokens: 7,
            has_usage: true,
            tool_calls: 2,
            session_totals: None,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "session_turn_usage",
                "address": "spawned-x-0001",
                "run_id": "run-1",
                "output_tokens": 7,
                "has_usage": true,
                "tool_calls": 2,
            })
        );
    }
}

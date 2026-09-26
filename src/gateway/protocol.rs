//! WebSocket protocol types: client and server message frames.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::agent::usage::SessionUsageTotals;
use crate::background::registry::{SessionCategory, SessionState};
use crate::inference::ImageData;
use crate::workspace::watch::{WorkspaceChange, WorkspaceResyncReason};

/// Messages sent from a WebSocket client to the server.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMessage {
    /// Send a user message to the agent.
    SendMessage {
        /// Client-generated correlation ID.
        id: String,
        /// The user message content.
        content: String,
        /// Optional image attachments (base64-encoded).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageData>,
    },
    /// Toggle verbose mode (tool call/result events).
    SetVerbose {
        /// Whether to receive tool events.
        enabled: bool,
    },
    /// Keepalive ping.
    Ping,
    /// Request the gateway to reload its configuration.
    Reload,
    /// A named server command (observe, reflect, context, etc.).
    ServerCommand {
        /// Command name.
        name: String,
        /// Optional argument text.
        args: Option<String>,
    },
    /// Add a message to the inbox without triggering an agent turn.
    InboxAdd {
        /// The message body to add.
        body: String,
    },
    /// Stop the currently running agent turn.
    Cancel {
        /// Correlation ID of the turn to stop (matches the `id` sent with
        /// the original `SendMessage`, and the `reply_to` on its
        /// `TurnStarted`/`TurnEnded`). A stop for a turn that has already
        /// ended is silently ignored.
        reply_to: String,
    },
    /// Send the owner's message to an agent session from the sessions
    /// sidebar. Delivered like any agent message (hop count 0): an interrupt
    /// if the session's turn is running, a new turn if it is idle, or a new
    /// run if it has completed. Answered with `SessionMessageDelivered` or
    /// `SessionCommandFailed`, both carrying `id`.
    SessionSendMessage {
        /// Client-generated correlation ID for the reply.
        id: String,
        /// Address of the target session (not `main`).
        address: String,
        /// The message content.
        content: String,
    },
    /// Stop a live agent session: cancels any in-flight turn and moves it
    /// straight to `completing`. Answered with `SessionStopRequested` or
    /// `SessionCommandFailed`, both carrying `id`.
    SessionStop {
        /// Client-generated correlation ID for the reply.
        id: String,
        /// Address of the session to stop.
        address: String,
    },
    /// Replace this connection's watched workspace prefixes. `[]` stops
    /// watching; `""` watches the whole workspace. An invalid prefix is
    /// answered with an `Error` frame and leaves the set unchanged.
    WatchWorkspace {
        /// Workspace-relative paths, matched by whole segments.
        prefixes: Vec<String>,
    },
}

/// One run of an agent session, as listed in the web UI's sessions sidebar
/// and carried by `SessionStarted`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    /// The session's stable address.
    pub address: String,
    /// This run's id (unique across all runs).
    pub run_id: String,
    /// How the session was started.
    pub category: SessionCategory,
    /// Precise source (e.g. `"pulse:email_check"`, `"agent:researcher"`).
    pub source_label: String,
    /// Current lifecycle state (`completed` for a finished run).
    pub state: SessionState,
    /// The agent that spawned this session, if any.
    pub spawner: Option<String>,
    /// Depth from the main agent (main = 0).
    pub depth: u32,
    /// One-line description of what the run is doing.
    pub purpose: String,
    /// When the run started (RFC 3339, UTC).
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    /// When the run completed (RFC 3339, UTC), once it has.
    #[ts(type = "string | null")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Episode the run was merged into, if it produced one.
    pub episode_id: Option<String>,
    /// `true` when the run was completed at startup because the process
    /// exited before it finished on its own.
    pub interrupted: bool,
    /// This run's cumulative token usage, for the `SessionView` footer —
    /// live for a run still going, final for a completed one. See
    /// `docs/systems-usage/turn-control.md`.
    pub usage: SessionUsageTotals,
    /// How the run ended — completed, cancelled, or failed. `None` while the
    /// run is still live, and `None` for a completed run recorded before
    /// this field existed (it shows as plain "finished" rather than a
    /// guessed outcome).
    pub outcome: Option<SessionRunStatus>,
    /// The failure reason, when `outcome` is `Failed`. `None` otherwise.
    pub error: Option<String>,
    /// Full technical cause chain behind `error`, shown behind the same
    /// expandable "details" toggle the web UI uses for a live `SessionError`.
    /// `None` for a failure with nothing richer to show, and `None` for a
    /// record written before this field existed.
    #[serde(default)]
    pub error_details: Option<String>,
    /// Set when this run is a pulse fire that started while its previous run
    /// was still live. `None` for every other trigger.
    pub overlap: Option<crate::bus::PulseOverlap>,
}

/// `GET /api/sessions` response: live sessions plus one page of completed
/// runs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionListResponse {
    /// Every live session (forking, running, idle, or completing) matching
    /// the filter, newest first. Not paginated.
    pub live: Vec<SessionSummary>,
    /// One page of completed runs from the session store, newest first.
    pub completed: Vec<SessionSummary>,
    /// Opaque cursor for the next page of completed runs (pass back as
    /// `?before=`), or `null` when there are no more.
    pub next_cursor: Option<String>,
}

/// How a session's run ended, as reported by `SessionCompleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionRunStatus {
    /// The last turn finished normally.
    Completed,
    /// The session was stopped.
    Cancelled,
    /// The last turn failed.
    Failed,
}

impl SessionRunStatus {
    /// Lowercase label used in the session store (see `RunRecord::outcome`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    /// Parse the label [`Self::as_str`] produces, as recorded in the session
    /// store. `None` for anything else.
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

/// One run's outcome, for a pulse's or action's "last outcome" in the
/// Scheduled view.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduledRunOutcome {
    pub status: SessionRunStatus,
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    pub error: Option<String>,
}

/// A pulse's or action's currently live run, if it has one, in the
/// Scheduled view.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduledCurrentRun {
    pub address: String,
    pub run_id: String,
    /// Set when this run started while its previous run was still going.
    pub overlap: Option<crate::bus::PulseOverlap>,
}

/// One pulse, as listed by `GET /api/scheduled/pulses`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PulseInfo {
    pub name: String,
    /// `false` for a pulse that failed to load at all (see `problems`) —
    /// there is no real `enabled` value to report for it.
    pub enabled: bool,
    pub schedule: Option<String>,
    pub active_hours: Option<String>,
    pub agent: Option<String>,
    /// Estimated from the schedule and `pulse_state.json`'s last run time;
    /// does not account for an `active_hours` window that would delay the
    /// actual fire.
    #[ts(type = "string | null")]
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<ScheduledRunOutcome>,
    pub current_run: Option<ScheduledCurrentRun>,
    /// Loading problems naming this pulse (a removed option, a duplicate
    /// name, a bad `schedule`/`active_hours` string, or a per-entry
    /// deserialize failure). Plain-language messages, ready to show as-is.
    pub problems: Vec<String>,
}

/// One pending scheduled action, as listed by `GET /api/scheduled/actions`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActionInfo {
    pub id: String,
    pub name: String,
    #[ts(type = "string")]
    pub run_at: DateTime<Utc>,
    pub agent: Option<String>,
    pub model_tier: Option<String>,
    pub current_run: Option<ScheduledCurrentRun>,
}

/// Where a `SessionSendMessage` landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionDeliveryOutcome {
    /// Delivered to the live session: mid-turn if it was running, as a new
    /// turn if it was idle.
    Live,
    /// The session had completed; a new run was started at the same
    /// address with the message as its input.
    Resumed,
    /// The session's run was finishing up; the message will start a new run
    /// once it has.
    Queued,
}

/// Why a session command (`SessionSendMessage`, `SessionStop`) failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionCommandErrorCode {
    /// The request itself is unusable (empty message, `main` as target).
    InvalidRequest,
    /// No session has ever run at this address.
    UnknownAddress,
    /// The session is live but can't take another message right now.
    Busy,
    /// The session is not live (already completing or completed), so there
    /// is nothing to stop.
    NotLive,
    /// The message could not be handed to the session.
    DeliveryFailed,
}

/// One artifact in `GET /api/workbench/artifacts`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactSummary {
    /// Artifact name, as used in `/workbench/{name}`.
    pub name: String,
    /// The page's `<title>`, or the name when it has none.
    pub title: String,
    /// When the page was last modified (RFC 3339, UTC).
    #[ts(type = "string")]
    pub modified_at: DateTime<Utc>,
    /// Page size in bytes.
    #[ts(type = "number")]
    pub size: u64,
}

/// `GET /api/workbench/info`: where workbench artifacts are served.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkbenchInfo {
    /// Port of the local artifacts listener, or `null` when it isn't running.
    pub port: Option<u16>,
    /// Plain-language reason the artifacts listener isn't running, when it
    /// isn't.
    pub unavailable_reason: Option<String>,
    /// Public origins through the cloud relay, when connected to a relay that
    /// announces them.
    pub relay: Option<WorkbenchRelayOrigins>,
}

/// The web UI's and the artifacts' public origins through the cloud relay.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkbenchRelayOrigins {
    /// Origin the web UI is served from through the relay.
    pub ui_origin: String,
    /// Origin the artifacts are served from through the relay.
    pub artifacts_origin: String,
}

/// Messages sent from the server to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMessage {
    /// The agent began processing a queued message.
    TurnStarted {
        /// Correlation ID of the message being processed.
        reply_to: String,
    },
    /// The agent turn has finished (success or error), whether or not any
    /// response text was produced. Marks the end of a turn that a
    /// `TurnStarted` opened, so clients can stop waiting even when the turn
    /// yielded zero `Response` frames (e.g. an interrupted or tool-only turn).
    TurnEnded {
        /// Correlation ID of the message whose turn just completed.
        reply_to: String,
    },
    /// Token usage progress for the main agent's turn still running: this
    /// turn's own output tokens so far (for the running-turn indicator)
    /// and, once at least one call has reported usage, the updated
    /// cumulative session totals (for the chat footer). Never delivered
    /// to the agent itself. See `docs/systems-usage/turn-control.md`.
    TurnUsage {
        /// Correlation ID of the message being processed.
        reply_to: String,
        /// Output tokens generated by every model call so far this turn.
        output_tokens: u32,
        /// Whether any model call so far this turn reported usage.
        has_usage: bool,
        /// Updated cumulative session totals, once known.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_totals: Option<SessionUsageTotals>,
    },
    /// A tool was invoked during the agent turn (verbose only).
    ToolCall {
        /// Unique tool call ID for correlating with results.
        id: String,
        /// Name of the tool.
        name: String,
        /// Tool arguments as JSON.
        arguments: serde_json::Value,
    },
    /// A tool completed execution (verbose only).
    ToolResult {
        /// Correlation ID matching the original tool call.
        tool_call_id: String,
        /// Name of the tool.
        name: String,
        /// Tool output text.
        output: String,
        /// Whether the tool returned an error.
        is_error: bool,
    },
    /// The agent's final text response.
    Response {
        /// Correlation ID of the original message.
        reply_to: String,
        /// The response content.
        content: String,
    },
    /// A file attachment from the agent.
    FileAttachment {
        /// Correlation ID of the original message.
        reply_to: String,
        /// Original filename.
        filename: String,
        /// MIME type of the file.
        mime_type: String,
        /// File size in bytes.
        #[ts(type = "number")]
        size: u64,
        /// URL to fetch the file: a durable workspace-relative link (e.g.
        /// "/api/files/workspace?path=...") for a file inside the
        /// workspace, else an expiring token link (e.g. "/api/files/{id}").
        url: String,
        /// Optional caption text.
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    /// Intermediate text the agent emitted alongside tool calls.
    BroadcastResponse {
        /// The intermediate content.
        content: String,
    },
    /// An error related to a specific request.
    Error {
        /// Correlation ID of the original message, if applicable.
        reply_to: Option<String>,
        /// Plain-language error description.
        message: String,
        /// Full technical cause chain, shown behind a details toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// Keepalive pong.
    Pong,
    /// The gateway is reloading its configuration.
    Reloading,
    /// Result of a manual memory operation (observe or reflect).
    Notice {
        /// Human-readable result message.
        message: String,
    },
    /// Multi-line command output that should render inline in the chat
    /// stream rather than as a transient toast.
    InlineOutput {
        /// Body to render inline.
        message: String,
    },
    /// A new agent session run was registered (state `forking`).
    SessionStarted {
        /// The new run.
        session: SessionSummary,
    },
    /// A live session run moved to a new lifecycle state (`running`,
    /// `idle`, or `completing`). Reaching `completed` is reported by
    /// `SessionCompleted` instead.
    SessionStateChanged {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// The new state.
        state: SessionState,
    },
    /// A session run finished, was recorded in the session store, and is
    /// no longer live. Its transcript is available from the transcript
    /// endpoint.
    SessionCompleted {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// How the run's last turn ended.
        status: SessionRunStatus,
        /// The failure, when `status` is `failed`.
        error: Option<String>,
        /// Full technical cause chain behind `error`, shown behind the same
        /// expandable "details" toggle as a live `SessionError`. `None` when
        /// there's nothing richer to show.
        #[serde(skip_serializing_if = "Option::is_none")]
        error_details: Option<String>,
        /// Episode the run was merged into, if it produced one.
        episode_id: Option<String>,
    },
    /// A session turn began.
    SessionTurnStarted {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Identifies the turn within the run.
        turn_id: String,
    },
    /// A session turn finished, whatever its outcome.
    SessionTurnEnded {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Identifies the turn within the run.
        turn_id: String,
    },
    /// A session invoked a tool (verbose only).
    SessionToolCall {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Unique tool call ID for correlating with results.
        id: String,
        /// Name of the tool.
        name: String,
        /// Tool arguments as JSON.
        arguments: serde_json::Value,
    },
    /// A tool a session invoked returned (verbose only).
    SessionToolResult {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Correlation ID matching the original tool call.
        tool_call_id: String,
        /// Name of the tool.
        name: String,
        /// Tool output text.
        output: String,
        /// Whether the tool returned an error.
        is_error: bool,
    },
    /// Token usage progress for a session's turn still running — the
    /// session counterpart of `TurnUsage`.
    SessionTurnUsage {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Output tokens generated by every model call so far this turn.
        output_tokens: u32,
        /// Whether any model call so far this turn reported usage.
        has_usage: bool,
        /// Updated cumulative session totals, once known.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_totals: Option<SessionUsageTotals>,
    },
    /// Intermediate text a session emitted alongside tool calls.
    SessionBroadcastResponse {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// The intermediate content.
        content: String,
    },
    /// A session turn's final text response.
    SessionResponse {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// The turn that produced it.
        turn_id: String,
        /// The response content.
        content: String,
    },
    /// Something went wrong that affects a session: a failed turn, a
    /// message refused at the hop limit, or a result relay that could not
    /// be delivered.
    SessionError {
        /// Session address.
        address: String,
        /// Run id.
        run_id: String,
        /// Plain-language error description.
        message: String,
        /// Full technical cause chain, shown behind a details toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// A session's message reached the main agent (a turn-result relay or a
    /// `message_agent` call to `main`). The main chat shows it as a compact
    /// item linking to the session.
    SessionMessageToMain {
        /// Sending session's address.
        address: String,
        /// Sending run's id.
        run_id: String,
        /// The message body as main received it.
        content: String,
    },
    /// Reply to `SessionSendMessage`: the message was handed to the session.
    SessionMessageDelivered {
        /// The `id` from the `SessionSendMessage`.
        id: String,
        /// The target session's address.
        address: String,
        /// Where the message landed.
        outcome: SessionDeliveryOutcome,
    },
    /// Reply to `SessionStop`: the session was signalled to stop. Its
    /// `SessionStateChanged` (`completing`) and `SessionCompleted` follow.
    SessionStopRequested {
        /// The `id` from the `SessionStop`.
        id: String,
        /// The stopped session's address.
        address: String,
    },
    /// Reply to `SessionSendMessage` or `SessionStop`: the command failed.
    SessionCommandFailed {
        /// The `id` from the failed command.
        id: String,
        /// The session address the command named.
        address: String,
        /// Machine-readable reason.
        code: SessionCommandErrorCode,
        /// Human-readable explanation, suitable to show the user.
        message: String,
    },
    /// A workbench artifact's page was created or modified.
    ArtifactUpdated {
        /// Artifact name, as used in `/workbench/{name}`.
        name: String,
    },
    /// A workbench artifact's page was deleted.
    ArtifactRemoved {
        /// Artifact name, as used in `/workbench/{name}`.
        name: String,
    },
    /// Workspace files under this connection's watched prefixes changed.
    WorkspaceChanged {
        /// The matching changes of one debounced batch, sorted by path.
        changes: Vec<WorkspaceChange>,
    },
    /// This connection's view of its watched prefixes may be stale; reload
    /// what they show. Sent instead of `WorkspaceChanged`.
    WorkspaceResync {
        /// Why changes may have been missed.
        reason: WorkspaceResyncReason,
    },
    /// The workspace watcher isn't running, so no workspace frames will
    /// arrive. Sent to watching connections.
    WorkspaceWatchUnavailable {
        /// Plain-language explanation for the user.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_deserialize_send_message() {
        let json = r#"{"type":"send_message","id":"abc-123","content":"hello"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(
            matches!(
                &msg,
                ClientMessage::SendMessage { id, content, images }
                    if id == "abc-123" && content == "hello" && images.is_empty()
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
            content: "hello back".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"type\":\"response\""),
            "should have type tag"
        );
        assert!(
            json.contains("\"reply_to\":\"id-1\""),
            "should have reply_to"
        );
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"notice\""), "should have type tag");
        assert!(
            json.contains("\"message\":\"observed successfully\""),
            "should have message field"
        );
    }

    #[test]
    fn server_message_serialize_broadcast_response() {
        let msg = ServerMessage::BroadcastResponse {
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
    fn client_message_send_message_with_images_roundtrip() {
        let msg = ClientMessage::SendMessage {
            id: "img-1".to_string(),
            content: "look at this".to_string(),
            images: vec![ImageData {
                media_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"images\""),
            "images field should be serialized when non-empty"
        );
        let deserialized: ClientMessage = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(
                &deserialized,
                ClientMessage::SendMessage { id, content, images }
                    if id == "img-1" && content == "look at this" && images.len() == 1
            ),
            "should survive serialization round-trip with images"
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
            ClientMessage::SessionSendMessage { id, address, content }
                if id == "c1" && address == "spawned-a-0001" && content == "hi"
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
            outcome: SessionDeliveryOutcome::Resumed,
        };
        assert_eq!(
            serde_json::to_value(&delivered).unwrap(),
            serde_json::json!({
                "type": "session_message_delivered",
                "id": "c1",
                "address": "spawned-a-0001",
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
                kind: crate::workspace::watch::WorkspaceChangeKind::Created,
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
            session_totals: None,
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "turn_usage",
                "reply_to": "id-1",
                "output_tokens": 42,
                "has_usage": true,
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
            session_totals: Some(SessionUsageTotals {
                input_tokens: 100,
                output_tokens: 42,
                context_tokens: Some(100),
            }),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::json!({
                "type": "turn_usage",
                "reply_to": "id-1",
                "output_tokens": 42,
                "has_usage": true,
                "session_totals": {
                    "input_tokens": 100,
                    "output_tokens": 42,
                    "context_tokens": 100,
                },
            })
        );
    }

    #[test]
    fn server_message_serialize_session_turn_usage() {
        let msg = ServerMessage::SessionTurnUsage {
            address: "spawned-x-0001".into(),
            run_id: "run-1".into(),
            output_tokens: 7,
            has_usage: true,
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
            })
        );
    }
}

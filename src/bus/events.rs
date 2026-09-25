//! Event types carried on the bus.

use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::bus::types::{SessionAddress, SkillName};
use crate::config::BackgroundModelTier;
use crate::inference::{ImageData, MessageSender};
use crate::interfaces::attachment::FileAttachment;
use crate::interfaces::types::{InboundMessage, MessageOrigin};

// ---------------------------------------------------------------------------
// EventTrigger
// ---------------------------------------------------------------------------

/// What triggered a background event or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTrigger {
    /// A recurring pulse (cron-style schedule).
    Pulse,
    /// A one-shot action.
    Action,
    /// A subagent spawned the work.
    Agent,
    /// An inbound webhook with the given name.
    Webhook(String),
    /// A message in a conversation the interfaces admitted but that doesn't
    /// belong to the main agent's own conversation (a group chat, a channel,
    /// or a non-owner DM) — routed to that conversation's `external` session.
    Conversation,
    /// A workbench artifact with the given name started the session through
    /// the sessions HTTP API.
    Artifact(String),
}

impl EventTrigger {
    /// Lowercase label for display and serialization.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pulse => "pulse",
            Self::Action => "action",
            Self::Agent => "agent",
            Self::Webhook(_) => "webhook",
            Self::Conversation => "conversation",
            Self::Artifact(_) => "artifact",
        }
    }
}

impl fmt::Display for EventTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Webhook(name) => write!(f, "webhook:{name}"),
            Self::Artifact(name) => write!(f, "artifact:{name}"),
            other @ (Self::Pulse | Self::Action | Self::Agent | Self::Conversation) => {
                f.write_str(other.as_str())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ResultDisposition
// ---------------------------------------------------------------------------

/// Sentinel an agent puts in its summary to signal nothing needs surfacing.
pub const HEARTBEAT_OK: &str = "HEARTBEAT_OK";

/// Sentinel an agent puts in its summary to signal the result cannot wait.
///
/// Deliberately distinct from the word "urgent" so that a summary *about*
/// something urgent does not escalate itself by accident.
pub const HEARTBEAT_URGENT: &str = "HEARTBEAT_URGENT";

/// True when `summary` ends with `sentinel` (ignoring trailing whitespace),
/// rather than merely mentioning it somewhere in the middle.
///
/// A plain substring check misfires whenever a longer summary happens to
/// discuss the sentinel itself — e.g. an assistant explaining that it
/// deliberately did *not* end its reply with `HEARTBEAT_OK` still contains
/// that literal text. Both sentinels are meant to be read off the end of a
/// summary (a pulse instruction says "respond with exactly `HEARTBEAT_OK`";
/// a `HEARTBEAT.yml` template says "end a summary with `HEARTBEAT_URGENT`"),
/// so anchoring the check there is what those instructions actually mean.
#[must_use]
pub fn ends_with_sentinel(summary: &str, sentinel: &str) -> bool {
    summary.trim_end().ends_with(sentinel)
}

/// What the producing agent signalled should happen with its result.
///
/// The agent that ran the task is the only participant with the full
/// transcript and the task's intent, so it makes this call itself via
/// sentinel strings in its summary rather than deferring to a downstream
/// classifier working from a summary alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDisposition {
    /// Nothing worth surfacing; discard without delivering anywhere.
    Silent,
    /// Ordinary result; file it for review.
    Normal,
    /// Needs attention now; push it as well as filing it.
    Urgent,
}

// ---------------------------------------------------------------------------
// AgentResultStatus
// ---------------------------------------------------------------------------

/// Terminal status of a background/subagent task.
#[derive(Debug, Clone)]
pub enum AgentResultStatus {
    /// Task finished successfully.
    Completed,
    /// Task was cancelled before completion.
    Cancelled,
    /// Task failed with an error.
    Failed {
        /// Plain-language description of what went wrong.
        error: String,
        /// Full technical cause chain, when the failure was classified from
        /// a model-call error. `None` for failures with nothing richer to
        /// show (a panic, a shutdown mid-run).
        details: Option<String>,
    },
}

impl fmt::Display for AgentResultStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed { error, .. } => write!(f, "failed: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Event structs
// ---------------------------------------------------------------------------

/// Inbound message from a user or external source.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    /// Correlation ID for reply routing.
    pub id: String,
    /// Message content.
    pub content: String,
    /// Where this message originated.
    pub origin: MessageOrigin,
    /// Local timestamp (see `crate::time::now_local`).
    pub timestamp: NaiveDateTime,
    /// Inline images attached to the message.
    pub images: Vec<ImageData>,
    /// Earlier conversation the sender's interface supplies as background
    /// (e.g. group chat messages leading up to an @mention).
    pub context: Option<String>,
}

impl MessageEvent {
    /// Build a `MessageEvent` addressed to the main agent from an internal,
    /// non-interface source (an agent-to-main relay, a delivery-failure
    /// notice). Always carries background origin — `endpoint: "background"`,
    /// no sender, no conversation — so `MessageOrigin::belongs_to_main`
    /// is true for it and it always reaches main, never a conversation
    /// session.
    #[must_use]
    pub fn from_background(content: String) -> Self {
        Self {
            id: format!("bg-{}", uuid::Uuid::new_v4()),
            content,
            origin: MessageOrigin {
                endpoint: "background".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: Vec::new(),
            context: None,
        }
    }

    /// Build a background `MessageEvent` delivering another agent's message
    /// to main: [`Self::from_background`] with the message's formatted text,
    /// plus its structured sender so main's history attributes it.
    #[must_use]
    pub fn from_agent(msg: &AgentMessageEvent) -> Self {
        let mut event = Self::from_background(msg.format_for_agent());
        event.origin.agent_sender = msg.agent_sender().map(Box::new);
        event
    }
}

/// Agent response destined for an endpoint.
#[derive(Debug, Clone)]
pub struct ResponseEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Response body.
    pub content: String,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
    /// Optional file attachment.
    pub attachment: Option<FileAttachment>,
    /// Conversation on the endpoint to deliver to (an ID from
    /// `list_conversations`). When `None` the interface routes by
    /// correlation ID: the conversation that started the turn, else the owner.
    pub conversation: Option<String>,
}

/// A conversation session's turn output, destined for its own conversation
/// on a chat interface.
///
/// Distinct from [`ResponseEvent`] (the main agent's replies and
/// `send_message` posts) so an interface's delivery code can enforce the
/// "never falls back to the owner's DM" rule for a session's own output
/// without touching the main agent's delivery path, which keeps its existing
/// owner-DM fallback on an unresolvable target.
#[derive(Debug, Clone)]
pub struct SessionResponseEvent {
    /// Address of the session that produced this output, for the error log
    /// and failure notice to main when delivery fails.
    pub session_address: SessionAddress,
    /// The conversation this output replies to, on the interface subscribed
    /// to this endpoint.
    pub conversation_id: String,
    /// Response body.
    pub content: String,
    /// Optional file attachment.
    pub attachment: Option<FileAttachment>,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
    /// Whether this is the run's final output (`true`, from
    /// `maybe_output_to_conversation`) or intermediate pre-tool-call text
    /// emitted mid-turn (`false`, from `EventContext::publish_intermediate`).
    pub is_final: bool,
}

/// A conversation session's turn started or finished running, for a typing
/// indicator on the interface hosting that conversation — the same
/// signal [`TurnLifecycleEvent`] gives the main agent's own turns, but keyed
/// by conversation id rather than a message correlation id, since a
/// session's turn isn't a reply to any one message.
#[derive(Debug, Clone)]
pub struct ConversationTypingEvent {
    /// Which conversation, on whichever interface subscribes to it.
    pub conversation_id: String,
    /// Whether the session's turn just started (`true`) or ended (`false`).
    pub active: bool,
}

/// Push notification for notify channels.
#[derive(Debug, Clone)]
pub struct NotificationEvent {
    /// Short label for the notification.
    pub title: String,
    /// Body/details.
    pub content: String,
    /// What produced this notification.
    pub source: EventTrigger,
    /// Whether this needs attention now. Drives platform interruption level.
    pub urgent: bool,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

/// Tool invocation sent by the agent during a turn.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Unique identifier for this tool invocation.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    pub arguments: serde_json::Value,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Matches the originating tool call.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Tool output text.
    pub output: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
}

/// Intermediate model text emitted during a turn.
#[derive(Debug, Clone)]
pub struct IntermediateEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Partial/intermediate content.
    pub content: String,
}

/// Result from a completed agent session run.
#[derive(Debug, Clone)]
pub struct AgentResultEvent {
    /// Address of the session that produced this result.
    pub session_address: SessionAddress,
    /// Unique identifier of the run within the session.
    pub run_id: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"action:deploy"`).
    pub source_label: String,
    /// Skill the session ran with, if any.
    pub agent_skill: Option<SkillName>,
    /// What triggered this session.
    pub source: EventTrigger,
    /// What the producing agent signalled should happen with this result.
    pub disposition: ResultDisposition,
    /// Terminal status.
    pub status: AgentResultStatus,
    /// Human-readable summary of the result.
    pub summary: String,
    /// Path to the full conversation transcript, if saved.
    pub transcript_path: Option<PathBuf>,
    /// Local timestamp.
    pub timestamp: NaiveDateTime,
}

impl AgentResultEvent {
    /// Format this result for injection into the agent's conversation context.
    #[must_use]
    pub fn format_for_agent(&self) -> String {
        let mut out = format!(
            "[Session Result]\nSession: {} ({})\nTask: {}\nSource: {}\nStatus: {}",
            self.session_address,
            self.run_id,
            self.source_label,
            self.source.as_str(),
            self.status,
        );

        if !self.summary.is_empty() {
            out.push('\n');
            out.push_str("Output:\n");
            out.push_str(&self.summary);
        }

        match &self.transcript_path {
            Some(path) => {
                out.push_str("\nTranscript: ");
                out.push_str(&path.display().to_string());
            }
            None => out.push_str("\nTranscript: unavailable (failed to save)"),
        }

        out
    }
}

/// A message one agent sends to another by address (`message_agent` tool).
///
/// Delivered as an [`crate::agent::interrupt::Interrupt::AgentMessage`] to a
/// live session (an interrupt when its turn is running, a new turn's input
/// when it's idle). The main agent's own delivery reuses the existing
/// `MessageEvent`/`UserMessage` bus path instead, which already implements
/// the same interrupt-if-running/new-turn-if-idle behavior.
#[derive(Debug, Clone)]
pub struct AgentMessageEvent {
    /// Address of the sender: `"main"` or a session address for an agent,
    /// [`crate::background::registry::OWNER_ADDRESS`] for the owner, or
    /// `artifact:<name>` for a workbench artifact.
    pub from: SessionAddress,
    /// The sender's category label (`"main"`, `"scheduled"`, `"external"`,
    /// `"spawned"`, `"artifact"`, or `"owner"`).
    pub from_category: String,
    /// The message body.
    pub content: String,
    /// Hop count carried by this message: one more than the highest hop
    /// count among the inputs that drove the sending turn. External-origin
    /// input (a user message, a pulse/action firing, a webhook, a web
    /// sidebar message) is hop `0`.
    pub hop_count: u32,
}

impl AgentMessageEvent {
    /// Format this message for injection into the recipient's conversation,
    /// naming the sender's address and category so the recipient can reply.
    ///
    /// A message the owner typed into the web sessions sidebar (sender
    /// [`crate::background::registry::OWNER_ADDRESS`]) is labelled as coming
    /// from the owner instead: the owner is not an agent and has no address
    /// to message back, but sees this session's responses directly. A message
    /// a workbench artifact sent (sender `artifact:<name>`) is labelled as
    /// coming from that artifact, for the same reason.
    ///
    /// History entries carry the sender as a structured field (see
    /// [`Self::to_history_message`]), which is what the web UI trusts. It
    /// still recognizes the `[Agent Message from <address> (<category>)]`
    /// header for history written before that field existed, so a change to
    /// it needs a matching change in `web/src/lib/relay.ts`.
    #[must_use]
    pub fn format_for_agent(&self) -> String {
        if self.from.as_ref() == crate::background::registry::OWNER_ADDRESS {
            return format!(
                "[Message from the owner via the web UI — your response in this turn is shown \
                 to them directly]\n{}",
                self.content
            );
        }
        if let Some(artifact) = self.artifact_sender() {
            return format!(
                "[Message from the workbench artifact \"{artifact}\" — your response in this \
                 turn is shown to it directly]\n{}",
                self.content
            );
        }
        format!(
            "[Agent Message from {} ({})]\n{}",
            self.from, self.from_category, self.content
        )
    }

    /// The name of the workbench artifact that sent this message, or `None`
    /// when an agent or the owner sent it.
    #[must_use]
    pub fn artifact_sender(&self) -> Option<&str> {
        if self.from_category != crate::background::registry::ARTIFACT_SENDER_CATEGORY {
            return None;
        }
        self.from
            .as_ref()
            .strip_prefix(crate::background::registry::ARTIFACT_SENDER_PREFIX)
    }

    /// The structured sender for this message's history entry: the sending
    /// agent, or `None` for a message the owner typed into the web sessions
    /// sidebar or a workbench artifact sent (neither is an agent).
    #[must_use]
    pub fn agent_sender(&self) -> Option<crate::inference::AgentSender> {
        let from_an_agent = self.from.as_ref() != crate::background::registry::OWNER_ADDRESS
            && self.artifact_sender().is_none();
        from_an_agent.then(|| crate::inference::AgentSender {
            address: self.from.to_string(),
            category: self.from_category.clone(),
        })
    }

    /// This message as the recipient's history records it: the formatted
    /// text, tagged with its structured sender.
    #[must_use]
    pub fn to_history_message(&self) -> crate::inference::Message {
        crate::inference::Message::user(self.format_for_agent())
            .with_agent_sender(self.agent_sender())
    }
}

/// Outcome a session declared through its `a2a_task_update` tool call,
/// carried to the A2A [`crate::a2a::executor::SessionExecutor`] waiting on
/// the session's address so it can end the task's execution stream with the
/// right terminal (or input-required) status.
#[derive(Debug, Clone)]
pub struct A2aTaskSignalEvent {
    /// Address of the session that signaled its task's outcome.
    pub address: SessionAddress,
    /// The outcome the session declared.
    pub state: A2aTaskSignalState,
    /// The session's final message for this outcome.
    pub message: String,
    /// Files the session attached as artifacts, already read into `a2a`
    /// parts by the `a2a_task_update` tool.
    pub artifacts: Vec<a2a::Artifact>,
}

/// The outcome states a session can declare through `a2a_task_update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2aTaskSignalState {
    /// The delegated task is done.
    Completed,
    /// The task needs more input from the caller before it can continue.
    InputRequired,
    /// The task could not be completed.
    Failed,
}

/// Request to spawn an agent session from any source.
#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
    /// Address the resulting session will run under, generated by the caller
    /// up front so a synchronous caller (e.g. the `subagent_spawn` tool) can
    /// hand it back immediately.
    pub address: SessionAddress,
    /// Skill to activate for this session, if any.
    pub skill: Option<SkillName>,
    /// Human-readable source label (e.g. `"pulse:email_check"`, `"agent:researcher"`).
    pub source_label: String,
    /// The prompt/instructions for the session.
    pub prompt: String,
    /// Additional context to prepend to the session's prompt.
    pub context: Option<String>,
    /// What triggered this spawn request.
    pub source: EventTrigger,
    /// Model tier to run the session at.
    pub model_tier: BackgroundModelTier,
    /// The agent that requested this spawn: `Some(address)` for a `spawned`
    /// session (main or another session), `None` for `scheduled` and
    /// `external` sessions, which have no spawner.
    pub spawner: Option<SessionAddress>,
    /// Depth from the main agent this session will run at (main = 0). A
    /// `scheduled`/`external` session is always depth 1; a spawned session is
    /// its spawner's depth plus 1.
    pub depth: u32,
    /// Hop count for the new run's first turn: `0` for a `scheduled`/
    /// `external` trigger (external-origin input), one more than the
    /// spawning turn's highest input hop count for an agent-initiated spawn,
    /// or the hop count of the message that triggered a resume.
    pub hop_count: u32,
    /// Who sent this session's kickoff message, for a `Conversation`-triggered
    /// spawn — carried onto the fork's opening message so the session sees
    /// the same `[From: name via interface (location)]` attribution the main
    /// agent shows. `None` for every other trigger.
    pub sender: Option<MessageSender>,
    /// The conversation this session replies to, for a `Conversation`-triggered
    /// spawn. `None` for every other trigger, which have no conversation of
    /// their own to reply into.
    pub conversation: Option<ConversationTarget>,
    /// The original inbound message that triggered this spawn or resume, for
    /// a `Conversation`-triggered request. Carried alongside the already-
    /// extracted `prompt`/`context`/`sender` fields so that if this request
    /// loses the race for its target address (see
    /// `crate::background::listener::race_guard_interrupt`), its content can
    /// still be delivered as `Interrupt::UserMessage` — with the same sender
    /// attribution ordinary conversation delivery gets — instead of being
    /// misattributed as a plain agent message from `main`. `None` for every
    /// other trigger.
    pub inbound: Option<InboundMessage>,
    /// Images attached to this run's kickoff message, carried into its first
    /// turn alongside `prompt`/`context`. Populated for a `Conversation`-
    /// triggered spawn or resume from the triggering inbound message's
    /// images (or, for a resume combining several buffered messages, every
    /// buffered message's images); empty for every other trigger, which have
    /// no images of their own.
    pub images: Vec<ImageData>,
    /// Set when this is a pulse fire that started while its previous run was
    /// still live in the registry. `None` for every other trigger, and for a
    /// pulse fire that found no live previous run. See [`PulseOverlap`].
    pub overlap: Option<PulseOverlap>,
}

/// Marks a pulse run that started while its previous run was still going
/// (still `forking`/`queued`/`running`/`idle`/`completing` in the registry).
/// The new run is never skipped, blocked, or cancelled for this — it starts
/// normally — but this flag makes the overlap visible in the Scheduled view
/// and the run's own session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PulseOverlap {
    /// Run id of the still-live previous run this one overlapped with.
    pub previous_run_id: String,
    /// When that previous run started, so the overlap can be described as
    /// "still going after N minutes" without a second lookup.
    #[ts(type = "string")]
    pub previous_started_at: chrono::DateTime<chrono::Utc>,
}

/// The conversation an `external` conversation session replies to: which
/// interface endpoint, and which conversation on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTarget {
    /// Endpoint name the conversation lives on (e.g. `"discord"`).
    pub endpoint: String,
    /// Stable conversation id on that endpoint (an id `list_conversations`
    /// and `send_message` use to reach it).
    pub conversation_id: String,
}

impl SpawnRequestEvent {
    /// Combine this request's context and prompt into a turn's opening
    /// message, the same way a fresh fork's `TurnKickoff::Initial` does.
    /// Used when a spawn or resume attempt discovers the target address is
    /// already live and falls back to delivering this request's content into
    /// that run as a message instead of forking a second one.
    #[must_use]
    pub fn kickoff_text(&self) -> String {
        match &self.context {
            Some(ctx) => format!("{ctx}\n\n{}", self.prompt),
            None => self.prompt.clone(),
        }
    }
}

/// Operational notice broadcast to connected endpoints.
#[derive(Debug, Clone)]
pub struct NoticeEvent {
    /// Human-readable notice message.
    pub message: String,
}

/// Which background post-turn cycle (see `crate::gateway::post_turn`) an
/// activity signal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostTurnActivityKind {
    /// The automatic observer/reflector cycle.
    Memory,
    /// The end-of-turn subconscious evaluation.
    Subconscious,
}

/// A background post-turn cycle started or finished running, for a quiet
/// "updating memory…" / "reviewing turn…" indicator in the web UI — see
/// `crate::gateway::post_turn`'s module docs for why this work no longer
/// blocks the event loop, and so needs a signal of its own instead of
/// being implied by the turn indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostTurnActivityEvent {
    pub kind: PostTurnActivityKind,
    pub active: bool,
}

/// Multi-line command output meant for inline rendering in a chat surface.
///
/// Distinct from [`NoticeEvent`]: notices are transient toasts in the web UI,
/// while inline output lands in the chat stream so the user can scan it.
#[derive(Debug, Clone)]
pub struct InlineOutputEvent {
    /// Body to render inline.
    pub message: String,
}

/// An error tied to a specific agent turn, broadcast to connected endpoints.
#[derive(Debug, Clone)]
pub struct ErrorEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Plain-language error description, safe to show as-is.
    pub message: String,
    /// Full technical cause chain, for a web UI details toggle or a
    /// developer's own logs. Chat interfaces (Discord, Telegram, Teams)
    /// never show this — they destructure only `message`.
    pub details: Option<String>,
}

/// Something observable happened in a live agent session: a lifecycle
/// transition, or one of the same turn events the main agent publishes.
///
/// Carried on [`super::topics::Sessions`] so the web UI can follow every
/// session without polling. Every event names the session's address and the
/// run it belongs to, since one address can have several runs over time.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    /// Address of the session the event belongs to.
    pub address: SessionAddress,
    /// Run within the session the event belongs to.
    pub run_id: String,
    /// What happened.
    pub kind: SessionEventKind,
}

/// The payload of a [`SessionEvent`].
#[derive(Debug, Clone)]
pub enum SessionEventKind {
    /// A new run was registered (state `forking`). Carries the run's full
    /// registry entry so a listener can render it without a lookup.
    Started(Box<crate::background::registry::SessionInfo>),
    /// The run moved to a new lifecycle state (`running`, `idle`, or
    /// `completing`). Reaching `completed` is reported by [`Self::Completed`]
    /// instead.
    StateChanged(crate::background::registry::SessionState),
    /// The run finished, was recorded in the session store, and left the
    /// registry.
    Completed {
        /// Outcome of the run's last turn.
        status: AgentResultStatus,
        /// Episode the run was merged into, if it produced one.
        episode_id: Option<String>,
    },
    /// A turn began executing.
    TurnStarted {
        /// Identifies this turn within the run.
        turn_id: String,
    },
    /// A turn finished, whatever its outcome.
    TurnEnded {
        /// Identifies this turn within the run.
        turn_id: String,
    },
    /// The session invoked a tool.
    ToolCall(ToolCallEvent),
    /// A tool the session invoked returned.
    ToolResult(ToolResultEvent),
    /// Intermediate text the session emitted alongside tool calls.
    Intermediate {
        /// The intermediate content.
        content: String,
    },
    /// Token usage progress for a turn still running — see
    /// [`TurnUsageEvent`], which the main agent's own turns publish
    /// instead of this variant.
    TurnUsage {
        /// Output tokens generated by every model call so far this turn.
        output_tokens: u32,
        /// Whether any model call so far this turn reported usage.
        has_usage: bool,
        /// Updated cumulative session totals, when the caller tracks them.
        session_totals: Option<crate::agent::usage::SessionUsageTotals>,
    },
    /// A turn's final text response.
    Response {
        /// Identifies the turn that produced it.
        turn_id: String,
        /// The response content.
        content: String,
    },
    /// Something went wrong that affects this session: a failed turn, a
    /// refused message (hop limit), or a result relay that could not be
    /// delivered.
    Error {
        /// Plain-language description.
        message: String,
        /// Full technical cause chain, when there is one richer than
        /// `message` (e.g. classified from a failed model call).
        details: Option<String>,
    },
    /// The session's message reached the main agent: a turn-result relay to
    /// its spawner or a `message_agent` call addressed to `main`. Lets the
    /// web UI show the message in the main chat as it arrives, attributed to
    /// this run.
    MessageToMain {
        /// The message body as main received it (without the sender header).
        content: String,
    },
}

// ---------------------------------------------------------------------------
// Typed topic event enums
// ---------------------------------------------------------------------------

/// Tool activity during a turn (call or result).
#[derive(Debug, Clone)]
pub enum ToolActivityEvent {
    /// A tool was invoked by the agent.
    Call(ToolCallEvent),
    /// A tool execution completed.
    Result(ToolResultEvent),
}

/// Turn lifecycle transitions.
#[derive(Debug, Clone)]
pub enum TurnLifecycleEvent {
    /// Agent turn has started processing.
    Started {
        /// Links back to the originating message.
        correlation_id: String,
    },
    /// Agent turn has finished processing.
    Ended {
        /// Links back to the originating message.
        correlation_id: String,
    },
}

/// Token usage progress for a turn still running: this turn's own output
/// tokens so far (for the running-turn indicator) and, when the caller
/// tracks cumulative session totals, the updated totals (for the chat
/// footer). Published after every model call; never delivered to the
/// agent itself. See `docs/systems-usage/turn-control.md`.
#[derive(Debug, Clone)]
pub struct TurnUsageEvent {
    /// Links back to the originating message.
    pub correlation_id: String,
    /// Output tokens generated by every model call so far this turn.
    pub output_tokens: u32,
    /// Whether any model call so far this turn reported usage.
    pub has_usage: bool,
    /// Updated cumulative session totals, when the caller tracks them.
    pub session_totals: Option<crate::agent::usage::SessionUsageTotals>,
}

/// A workbench artifact appeared, changed, or was deleted: its page, or any
/// file in its folder.
///
/// Carried on [`super::topics::Workbench`] so open workbench views can reload
/// the artifact live while the agent edits it. Derived from the workspace
/// change feed ([`WorkspaceEvent`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkbenchEvent {
    /// The artifact was created or modified.
    Updated {
        /// Artifact name.
        name: String,
    },
    /// The artifact was deleted.
    Removed {
        /// Artifact name.
        name: String,
    },
}

/// One debounced batch from the workspace change feed.
///
/// Carried on [`super::topics::Workspace`]. Each WebSocket connection filters
/// batches by the prefixes it watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// The changes of one batch, sorted by path. Shared so every subscriber
    /// gets the batch without copying it.
    Changed(std::sync::Arc<[crate::workspace::watch::WorkspaceChange]>),
    /// Changes may have been missed; watchers must reload what they show.
    Resync(crate::workspace::watch::WorkspaceResyncReason),
    /// The watcher stopped: live updates are off.
    Unavailable,
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::types::SkillName;

    fn make_agent_result(summary: &str, transcript: Option<&str>) -> AgentResultEvent {
        AgentResultEvent {
            session_address: SessionAddress::from("scheduled-check-0001"),
            run_id: "t1".into(),
            source_label: "pulse:check".into(),
            agent_skill: Some(SkillName::from("default")),
            source: EventTrigger::Pulse,
            disposition: ResultDisposition::Silent,
            status: AgentResultStatus::Completed,
            summary: summary.into(),
            transcript_path: transcript.map(std::path::PathBuf::from),
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 13)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        }
    }

    #[test]
    fn ends_with_sentinel_matches_exact_and_trailing() {
        assert!(ends_with_sentinel("HEARTBEAT_OK", HEARTBEAT_OK));
        assert!(ends_with_sentinel("HEARTBEAT_OK\n", HEARTBEAT_OK));
        assert!(ends_with_sentinel(
            "Nothing to report today. HEARTBEAT_OK",
            HEARTBEAT_OK
        ));
    }

    #[test]
    fn ends_with_sentinel_rejects_mid_text_mention() {
        // The sentinel appearing anywhere other than the end is a summary
        // *about* the sentinel, not a summary ending with it.
        assert!(!ends_with_sentinel(
            "The instruction to omit HEARTBEAT_OK was honored — this note \
             ends without it, since a sentinel was not appropriate here.",
            HEARTBEAT_OK
        ));
        assert!(!ends_with_sentinel(
            "HEARTBEAT_URGENT wasn't warranted this time, just a routine update.",
            HEARTBEAT_URGENT
        ));
    }

    #[test]
    fn event_trigger_webhook_debug() {
        let trigger = EventTrigger::Webhook("github".into());
        let debug = format!("{trigger:?}");
        assert!(
            debug.contains("github"),
            "Debug should contain webhook name"
        );
    }

    #[test]
    fn event_trigger_as_str() {
        assert_eq!(EventTrigger::Pulse.as_str(), "pulse");
        assert_eq!(EventTrigger::Action.as_str(), "action");
        assert_eq!(EventTrigger::Agent.as_str(), "agent");
        assert_eq!(EventTrigger::Webhook("github".into()).as_str(), "webhook");
        // Webhook name does not affect the label.
        assert_eq!(EventTrigger::Webhook("custom".into()).as_str(), "webhook");
        assert_eq!(EventTrigger::Artifact("wiki".into()).as_str(), "artifact");
    }

    #[test]
    fn event_trigger_display() {
        assert_eq!(
            EventTrigger::Webhook("github".into()).to_string(),
            "webhook:github"
        );
        assert_eq!(EventTrigger::Pulse.to_string(), "pulse");
        assert_eq!(EventTrigger::Action.to_string(), "action");
        assert_eq!(EventTrigger::Agent.to_string(), "agent");
        assert_eq!(
            EventTrigger::Artifact("wiki".into()).to_string(),
            "artifact:wiki"
        );
    }

    #[test]
    fn agent_result_status_display() {
        assert_eq!(AgentResultStatus::Completed.to_string(), "completed");
        assert_eq!(AgentResultStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(
            AgentResultStatus::Failed {
                error: "timeout".into(),
                details: None,
            }
            .to_string(),
            "failed: timeout"
        );
    }

    #[test]
    fn format_for_agent_empty_summary_no_transcript() {
        let event = make_agent_result("", None);
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nTranscript: unavailable (failed to save)"
        );
    }

    #[test]
    fn format_for_agent_with_summary_no_transcript() {
        let event = make_agent_result("some output", None);
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nOutput:\nsome output\nTranscript: unavailable (failed to save)"
        );
    }

    #[test]
    fn format_for_agent_empty_summary_with_transcript() {
        let event = make_agent_result("", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nTranscript: /tmp/transcript.txt"
        );
    }

    #[test]
    fn agent_message_event_format_for_agent_names_sender_and_category() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from("spawned-researcher-3f9a"),
            from_category: "spawned".to_string(),
            content: "found the answer".to_string(),
            hop_count: 0,
        };
        assert_eq!(
            msg.format_for_agent(),
            "[Agent Message from spawned-researcher-3f9a (spawned)]\nfound the answer"
        );
    }

    #[test]
    fn agent_message_event_from_owner_is_labelled_as_the_owner() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from(crate::background::registry::OWNER_ADDRESS),
            from_category: "owner".to_string(),
            content: "how's it going?".to_string(),
            hop_count: 0,
        };
        let text = msg.format_for_agent();
        assert!(
            text.starts_with("[Message from the owner via the web UI"),
            "owner messages should not be framed as agent messages, got {text}"
        );
        assert!(text.ends_with("\nhow's it going?"));
    }

    #[test]
    fn agent_message_history_entry_carries_its_structured_sender() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from("spawned-researcher-3f9a"),
            from_category: "spawned".to_string(),
            content: "found the answer".to_string(),
            hop_count: 0,
        };
        let entry = msg.to_history_message();
        assert_eq!(entry.content, msg.format_for_agent());
        assert_eq!(
            entry.agent_sender,
            Some(crate::inference::AgentSender {
                address: "spawned-researcher-3f9a".to_string(),
                category: "spawned".to_string(),
            })
        );
    }

    #[test]
    fn owner_message_history_entry_has_no_agent_sender() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from(crate::background::registry::OWNER_ADDRESS),
            from_category: "owner".to_string(),
            content: "how's it going?".to_string(),
            hop_count: 0,
        };
        assert_eq!(msg.to_history_message().agent_sender, None);
    }

    #[test]
    fn artifact_message_is_labelled_as_the_artifact_not_the_owner_or_an_agent() {
        let msg = AgentMessageEvent {
            from: crate::background::registry::artifact_sender_address("wiki-graph"),
            from_category: crate::background::registry::ARTIFACT_SENDER_CATEGORY.to_string(),
            content: "refresh the index".to_string(),
            hop_count: 0,
        };
        assert_eq!(msg.artifact_sender(), Some("wiki-graph"));
        let text = msg.format_for_agent();
        assert!(
            text.starts_with("[Message from the workbench artifact \"wiki-graph\""),
            "got {text}"
        );
        assert!(!text.contains("owner"), "got {text}");
        assert!(text.ends_with("\nrefresh the index"));
        assert_eq!(msg.to_history_message().agent_sender, None);
    }

    #[test]
    fn artifact_prefix_without_the_artifact_category_is_not_an_artifact_sender() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from("artifact:wiki"),
            from_category: "spawned".to_string(),
            content: "hi".to_string(),
            hop_count: 0,
        };
        assert_eq!(msg.artifact_sender(), None);
    }

    #[test]
    fn message_event_from_agent_attributes_main_history_entry() {
        let msg = AgentMessageEvent {
            from: SessionAddress::from("spawned-researcher-3f9a"),
            from_category: "spawned".to_string(),
            content: "found the answer".to_string(),
            hop_count: 1,
        };
        let event = MessageEvent::from_agent(&msg);
        assert_eq!(event.content, msg.format_for_agent());
        assert!(event.origin.belongs_to_main());
        assert_eq!(event.origin.agent_sender.map(|a| *a), msg.agent_sender());
    }

    #[test]
    fn format_for_agent_with_summary_and_transcript() {
        let event = make_agent_result("some output", Some("/tmp/transcript.txt"));
        assert_eq!(
            event.format_for_agent(),
            "[Session Result]\nSession: scheduled-check-0001 (t1)\nTask: pulse:check\nSource: pulse\nStatus: completed\nOutput:\nsome output\nTranscript: /tmp/transcript.txt"
        );
    }
}

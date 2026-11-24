use std::fmt;
use std::path::PathBuf;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::attachment::FileAttachment;
use crate::origin::MessageOrigin;
use crate::types::{SessionAddress, SkillName};
use residuum_config::ModelSelector;
use residuum_inference::{ImageData, Message, MessageAttachment, MessageSender};
use residuum_memory::types::Visibility;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTrigger {
    Pulse,
    Action,
    Agent,
    Webhook(String),
    Conversation,
    Artifact(String),
}

impl EventTrigger {
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

pub const HEARTBEAT_OK: &str = "HEARTBEAT_OK";

pub const HEARTBEAT_URGENT: &str = "HEARTBEAT_URGENT";

#[must_use]
pub fn ends_with_sentinel(summary: &str, sentinel: &str) -> bool {
    summary.trim_end().ends_with(sentinel)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDisposition {
    Silent,
    Normal,
    Urgent,
}

#[derive(Debug, Clone)]
pub enum AgentResultStatus {
    Completed,
    Cancelled,
    Failed {
        error: String,
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

#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub id: String,
    pub content: String,
    pub origin: MessageOrigin,
    pub timestamp: NaiveDateTime,
    pub images: Vec<ImageData>,
    pub attachments: Vec<MessageAttachment>,
    pub context: Option<String>,
}

impl MessageEvent {
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
            attachments: Vec::new(),
            context: None,
        }
    }

    #[must_use]
    pub fn from_agent(msg: &AgentMessageEvent) -> Self {
        let mut event = Self::from_background(msg.format_for_agent());
        event.origin.agent_sender = msg.agent_sender().map(Box::new);
        event
    }

    #[must_use]
    pub fn into_history_messages(self) -> Vec<Message> {
        let mut user = if self.images.is_empty() {
            Message::user(self.content)
        } else {
            Message::user_with_images(self.content, self.images)
        };
        user.attachments = self.attachments;
        self.context
            .map(|context| Message::user(context).as_context())
            .into_iter()
            .chain(std::iter::once(
                user.with_sender(self.origin.sender)
                    .with_agent_sender(self.origin.agent_sender.map(|a| *a)),
            ))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ResponseEvent {
    pub correlation_id: String,
    pub content: String,
    pub timestamp: NaiveDateTime,
    pub attachment: Option<FileAttachment>,
    pub conversation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionResponseEvent {
    pub session_address: SessionAddress,
    pub conversation_id: String,
    pub content: String,
    pub attachment: Option<FileAttachment>,
    pub timestamp: NaiveDateTime,
    pub is_final: bool,
}

#[derive(Debug, Clone)]
pub struct ConversationTypingEvent {
    pub conversation_id: String,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct NotificationEvent {
    pub title: String,
    pub content: String,
    pub source: EventTrigger,
    pub urgent: bool,
    pub timestamp: NaiveDateTime,
}

#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub correlation_id: String,
    pub tool_call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub server: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    pub correlation_id: String,
    pub tool_call_id: String,
    pub name: String,
    pub output: String,
    pub is_error: bool,
    pub auto_mode: Option<crate::auto_mode::AutoModeVerdict>,
}

#[derive(Debug, Clone)]
pub struct IntermediateEvent {
    pub correlation_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct AgentResultEvent {
    pub session_address: SessionAddress,
    pub run_id: String,
    pub source_label: String,
    pub agent_skill: Option<SkillName>,
    pub source: EventTrigger,
    pub disposition: ResultDisposition,
    pub status: AgentResultStatus,
    pub summary: String,
    pub transcript_path: Option<PathBuf>,
    pub timestamp: NaiveDateTime,
}

#[derive(Debug, Clone)]
pub struct AgentMessageEvent {
    pub id: String,
    pub from: SessionAddress,
    pub from_category: String,
    pub content: String,
    pub hop_count: u32,
}

impl AgentMessageEvent {
    #[must_use]
    pub fn new(
        from: SessionAddress,
        from_category: impl Into<String>,
        content: impl Into<String>,
        hop_count: u32,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from,
            from_category: from_category.into(),
            content: content.into(),
            hop_count,
        }
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    #[must_use]
    pub fn read_event(&self, turn_id: &str) -> SessionEventKind {
        SessionEventKind::MessageRead {
            id: self.id.clone(),
            turn_id: turn_id.to_string(),
            content: self.content.clone(),
            from: self.from.to_string(),
            from_category: self.from_category.clone(),
        }
    }

    #[must_use]
    pub fn format_for_agent(&self) -> String {
        if self.from.as_ref() == crate::session::OWNER_ADDRESS {
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
        if let Some(teammate) = self.teammate_sender() {
            return format!(
                "[Message from teammate {teammate}, not the user. Your response in this turn is \
                 not shown to them; to reply, call message_agent with to=\"{teammate}\". In a \
                 session for an outside chat it is not posted there either: only send_message \
                 reaches that chat.]\n{}",
                self.content
            );
        }
        format!(
            "[Agent Message from {} ({})]\n{}",
            self.from, self.from_category, self.content
        )
    }

    #[must_use]
    pub fn teammate_sender(&self) -> Option<&str> {
        if self.from_category != crate::session::TEAMMATE_SENDER_CATEGORY {
            return None;
        }
        let address = self.from.as_ref();
        address
            .starts_with(crate::session::TEAMMATE_SENDER_PREFIX)
            .then_some(address)
    }

    #[must_use]
    pub fn artifact_sender(&self) -> Option<&str> {
        if self.from_category != crate::session::ARTIFACT_SENDER_CATEGORY {
            return None;
        }
        self.from
            .as_ref()
            .strip_prefix(crate::session::ARTIFACT_SENDER_PREFIX)
    }

    #[must_use]
    pub fn agent_sender(&self) -> Option<residuum_inference::AgentSender> {
        let from_an_agent =
            self.from.as_ref() != crate::session::OWNER_ADDRESS && self.artifact_sender().is_none();
        from_an_agent.then(|| residuum_inference::AgentSender {
            address: self.from.to_string(),
            category: self.from_category.clone(),
            run_id: None,
        })
    }

    #[must_use]
    pub fn to_history_message(&self) -> residuum_inference::Message {
        residuum_inference::Message::user(self.format_for_agent())
            .with_agent_sender(self.agent_sender())
            .with_id(Some(self.id.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct A2aTaskSignalEvent {
    pub address: SessionAddress,
    pub state: A2aTaskSignalState,
    pub message: String,
    pub artifacts: Vec<a2a::Artifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2aTaskSignalState {
    Completed,
    InputRequired,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SpawnRequestEvent {
    pub address: SessionAddress,
    pub skill: Option<SkillName>,
    pub source_label: String,
    pub prompt: String,
    pub context: Option<String>,
    pub source: EventTrigger,
    pub model: ModelSelector,
    pub spawner: Option<SessionAddress>,
    pub depth: u32,
    pub hop_count: u32,
    pub sender: Option<MessageSender>,
    pub conversation: Option<ConversationTarget>,
    pub inbound: Option<MessageEvent>,
    pub images: Vec<ImageData>,
    pub overlap: Option<PulseOverlap>,
    pub fire_id: Option<String>,
    pub carried_history: Vec<residuum_inference::Message>,
    pub delivered_messages: Vec<AgentMessageEvent>,
    pub delivered_chat_messages: Vec<MessageEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PulseOverlap {
    pub previous_run_id: Option<String>,
    #[serde(default)]
    pub previous_fire_id: Option<String>,
    #[ts(type = "string")]
    pub previous_started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTarget {
    pub endpoint: String,
    pub conversation_id: String,
}

impl SpawnRequestEvent {
    #[must_use]
    pub fn kickoff_text(&self) -> String {
        match &self.context {
            Some(ctx) => format!("{ctx}\n\n{}", self.prompt),
            None => self.prompt.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NoticeEvent {
    pub message: String,
    pub reload: Option<ReloadReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadReport {
    pub outcome: ReloadOutcome,
    pub of: ReloadedFiles,
}

impl NoticeEvent {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reload: None,
        }
    }

    #[must_use]
    pub fn reload(outcome: ReloadOutcome, of: ReloadedFiles, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reload: Some(ReloadReport { outcome, of }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReloadOutcome {
    Reloaded,
    Unchanged,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReloadedFiles {
    AgentConfig,
    /// `hub/config.toml`, which the agent reloads after the hub has; the hub
    /// reports its own reload on the hub socket.
    HubConfig,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostTurnActivityKind {
    Memory,
    Subconscious,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostTurnActivityEvent {
    pub kind: PostTurnActivityKind,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct OutboundA2aTaskEvent {
    pub task: crate::remote_task::TrackedTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInboxAddedEvent {
    pub item_id: String,
}

#[derive(Debug, Clone)]
pub struct InlineOutputEvent {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ErrorEvent {
    pub correlation_id: String,
    pub message: String,
    /// Full technical cause chain, for a web UI Details disclosure or a
    /// developer's own logs. Chat interfaces (Discord, Telegram, Teams)
    /// never show this — they destructure only `message`.
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub address: SessionAddress,
    pub run_id: String,
    pub kind: SessionEventKind,
}

#[derive(Debug, Clone)]
pub enum SessionEventKind {
    Started(Box<crate::session::SessionInfo>),
    StateChanged(crate::session::SessionState),
    Completed {
        status: AgentResultStatus,
        memory_pending: bool,
    },
    MemoryMerged {
        episode_id: Option<String>,
    },
    TurnStarted {
        turn_id: String,
    },
    TurnEnded {
        turn_id: String,
    },
    MessageRead {
        id: String,
        turn_id: String,
        content: String,
        from: String,
        from_category: String,
    },
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    Intermediate {
        content: String,
    },
    TurnUsage {
        output_tokens: u32,
        has_usage: bool,
        tool_calls: u32,
        session_totals: Option<crate::usage::SessionUsageTotals>,
    },
    Response {
        turn_id: String,
        content: String,
    },
    Error {
        message: String,
        details: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStartFailedEvent {
    pub address: SessionAddress,
    pub error: String,
    pub error_details: Option<String>,
    pub owner_start: bool,
}

#[derive(Debug, Clone)]
pub enum TurnLifecycleEvent {
    Started { correlation_id: String },
    Ended { correlation_id: String },
}

#[derive(Debug, Clone)]
pub struct TurnUsageEvent {
    pub correlation_id: String,
    pub output_tokens: u32,
    pub has_usage: bool,
    pub tool_calls: u32,
    pub session_totals: Option<crate::usage::SessionUsageTotals>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TurnOrigin {
    pub endpoint: String,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<MessageSender>,
    pub visibility: Visibility,
}

impl TurnOrigin {
    #[must_use]
    pub fn new(message_origin: &MessageOrigin, visibility: Visibility) -> Self {
        Self {
            endpoint: message_origin.endpoint.clone(),
            sender: message_origin.sender.clone(),
            visibility,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MainConversationEvent {
    TurnStarted {
        turn_id: String,
        origin: TurnOrigin,
    },
    TurnEnded {
        turn_id: String,
    },
    UserMessage {
        id: String,
        turn_id: String,
        content: String,
        images: Vec<ImageData>,
        attachments: Vec<MessageAttachment>,
        sender: Option<MessageSender>,
        agent_sender: Option<residuum_inference::AgentSender>,
        endpoint: String,
    },
    MessageQueued {
        id: String,
        content: String,
        images: Vec<ImageData>,
        attachments: Vec<MessageAttachment>,
        sender: Option<MessageSender>,
        endpoint: String,
    },
    MessageWithdrawn {
        id: String,
    },
    ToolCall {
        call: u32,
        event: ToolCallEvent,
    },
    ToolResult(ToolResultEvent),
    TextDelta {
        turn_id: String,
        call: u32,
        text: String,
    },
    ThinkingDelta {
        turn_id: String,
        call: u32,
        text: String,
    },
    StreamRestart {
        turn_id: String,
        call: u32,
    },
    Thinking {
        turn_id: String,
        call: u32,
        content: String,
    },
    Intermediate {
        turn_id: String,
        call: u32,
        content: String,
    },
    Response {
        turn_id: String,
        call: Option<u32>,
        endpoint: String,
        content: String,
    },
    TurnUsage(TurnUsageEvent),
}

impl MainConversationEvent {
    #[must_use]
    pub fn user_message(message: &MessageEvent, turn_id: &str) -> Option<Self> {
        let agent_sender = message.origin.agent_sender.as_deref();
        (message.origin.is_from_person() || agent_sender.is_some()).then(|| Self::UserMessage {
            id: message.id.clone(),
            turn_id: turn_id.to_owned(),
            content: message.content.clone(),
            images: message.images.clone(),
            attachments: message.attachments.clone(),
            sender: message.origin.sender.clone(),
            agent_sender: agent_sender.cloned(),
            endpoint: message.origin.endpoint.clone(),
        })
    }

    #[must_use]
    pub fn message_queued(message: &MessageEvent) -> Option<Self> {
        message
            .origin
            .is_from_person()
            .then(|| Self::MessageQueued {
                id: message.id.clone(),
                content: message.content.clone(),
                images: message.images.clone(),
                attachments: message.attachments.clone(),
                sender: message.origin.sender.clone(),
                endpoint: message.origin.endpoint.clone(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkbenchEvent {
    Updated { name: String },
    Removed { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    Changed(std::sync::Arc<[crate::workspace_change::WorkspaceChange]>),
    Resync(crate::workspace_change::WorkspaceResyncReason),
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn agent_message_event_format_for_agent_names_sender_and_category() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from("spawned-researcher-3f9a"),
            "spawned",
            "found the answer",
            0,
        );
        assert_eq!(
            msg.format_for_agent(),
            "[Agent Message from spawned-researcher-3f9a (spawned)]\nfound the answer"
        );
    }

    #[test]
    fn agent_message_event_from_owner_is_labelled_as_the_owner() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from(crate::session::OWNER_ADDRESS),
            "owner",
            "how's it going?",
            0,
        );
        let text = msg.format_for_agent();
        assert!(
            text.starts_with("[Message from the owner via the web UI"),
            "owner messages should not be framed as agent messages, got {text}"
        );
        assert!(text.ends_with("\nhow's it going?"));
    }

    #[test]
    fn agent_message_history_entry_carries_its_structured_sender() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from("spawned-researcher-3f9a"),
            "spawned",
            "found the answer",
            0,
        );
        let entry = msg.to_history_message();
        assert_eq!(entry.content, msg.format_for_agent());
        assert_eq!(
            entry.agent_sender,
            Some(residuum_inference::AgentSender {
                address: "spawned-researcher-3f9a".to_string(),
                category: "spawned".to_string(),
                run_id: None,
            })
        );
        assert_eq!(entry.id.as_deref(), Some(msg.id.as_str()));
    }

    #[test]
    fn each_new_message_gets_its_own_id_unless_its_sender_chose_one() {
        let from = || SessionAddress::from("spawned-researcher-3f9a");
        let first = AgentMessageEvent::new(from(), "spawned", "a", 0);
        let second = AgentMessageEvent::new(from(), "spawned", "b", 0);
        assert_ne!(first.id, "");
        assert_ne!(first.id, second.id, "fresh ids are unique");

        let chosen = AgentMessageEvent::new(from(), "owner", "c", 0).with_id("m-client");
        assert_eq!(chosen.id, "m-client");
        assert_eq!(chosen.to_history_message().id.as_deref(), Some("m-client"));
    }

    #[test]
    fn owner_message_history_entry_has_no_agent_sender() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from(crate::session::OWNER_ADDRESS),
            "owner",
            "how's it going?",
            0,
        );
        assert_eq!(msg.to_history_message().agent_sender, None);
    }

    #[test]
    fn artifact_message_is_labelled_as_the_artifact_not_the_owner_or_an_agent() {
        let msg = AgentMessageEvent::new(
            crate::session::artifact_sender_address("wiki-graph"),
            crate::session::ARTIFACT_SENDER_CATEGORY.to_string(),
            "refresh the index",
            0,
        );
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
    fn teammate_message_is_labelled_as_a_teammate_with_its_reply_address() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from("agent:writer/spawned-draft-3f9a"),
            crate::session::TEAMMATE_SENDER_CATEGORY.to_string(),
            "chapter two is ready",
            2,
        );
        assert_eq!(
            msg.teammate_sender(),
            Some("agent:writer/spawned-draft-3f9a")
        );
        let text = msg.format_for_agent();
        assert!(
            text.starts_with(
                "[Message from teammate agent:writer/spawned-draft-3f9a, not the user."
            ),
            "got {text}"
        );
        assert!(
            text.contains("call message_agent with to=\"agent:writer/spawned-draft-3f9a\""),
            "got {text}"
        );
        assert!(!text.contains("Agent Message from"), "got {text}");
        assert!(text.ends_with("\nchapter two is ready"));
        assert_eq!(
            msg.to_history_message().agent_sender,
            Some(residuum_inference::AgentSender {
                address: "agent:writer/spawned-draft-3f9a".to_string(),
                category: "teammate".to_string(),
                run_id: None,
            }),
            "the history entry carries the structured sender"
        );
    }

    #[test]
    fn teammate_prefix_without_the_teammate_category_is_not_a_teammate_sender() {
        let msg = AgentMessageEvent::new(SessionAddress::from("agent:writer"), "spawned", "hi", 0);
        assert_eq!(msg.teammate_sender(), None);
        assert!(msg.format_for_agent().starts_with("[Agent Message from"));
    }

    #[test]
    fn artifact_prefix_without_the_artifact_category_is_not_an_artifact_sender() {
        let msg = AgentMessageEvent::new(SessionAddress::from("artifact:wiki"), "spawned", "hi", 0);
        assert_eq!(msg.artifact_sender(), None);
    }

    #[test]
    fn message_event_from_agent_attributes_main_history_entry() {
        let msg = AgentMessageEvent::new(
            SessionAddress::from("spawned-researcher-3f9a"),
            "spawned",
            "found the answer",
            1,
        );
        let event = MessageEvent::from_agent(&msg);
        assert_eq!(event.content, msg.format_for_agent());
        assert!(event.origin.belongs_to_main());
        assert_eq!(event.origin.agent_sender.map(|a| *a), msg.agent_sender());
    }

    #[test]
    fn user_message_announces_a_person_or_an_agent_but_not_other_background_input() {
        let session = residuum_inference::AgentSender {
            address: "spawned-researcher-3f9a".to_string(),
            category: "spawned".to_string(),
            run_id: Some("run-1".to_string()),
        };
        let mut from_session = MessageEvent::from_background("found the answer".to_string());
        from_session.origin.agent_sender = Some(Box::new(session.clone()));
        let Some(MainConversationEvent::UserMessage {
            id,
            turn_id,
            agent_sender,
            endpoint,
            ..
        }) = MainConversationEvent::user_message(&from_session, "turn-1")
        else {
            panic!("a session's message to main is announced");
        };
        assert_eq!(id, from_session.id);
        assert_eq!(turn_id, "turn-1");
        assert_eq!(
            agent_sender,
            Some(session),
            "it names the session and its run"
        );
        assert_eq!(endpoint, crate::origin::BACKGROUND_ENDPOINT);

        let Some(MainConversationEvent::UserMessage {
            agent_sender: person_agent_sender,
            ..
        }) = MainConversationEvent::user_message(&inbound(None), "turn-1")
        else {
            panic!("a person's message is announced");
        };
        assert_eq!(person_agent_sender, None);

        let notice = MessageEvent::from_background("[Session Result] done".to_string());
        assert!(
            MainConversationEvent::user_message(&notice, "turn-1").is_none(),
            "background input no agent sent is not announced"
        );
    }

    fn inbound(context: Option<&str>) -> MessageEvent {
        MessageEvent {
            id: "m1".to_string(),
            content: "can you check the build?".to_string(),
            origin: crate::origin::MessageOrigin {
                endpoint: "teams".to_string(),
                sender: Some(residuum_inference::MessageSender {
                    name: "Jane".to_string(),
                    id: "aad-jane".to_string(),
                    interface: "teams".to_string(),
                    location: Some("#builds".to_string()),
                }),
                conversation: Some(crate::origin::ConversationContext {
                    id: "19:abc@thread.tacv2".to_string(),
                    kind: crate::origin::ConversationKind::Channel,
                    is_owner: true,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: vec![],
            context: context.map(str::to_string),
        }
    }

    #[test]
    fn context_precedes_the_attributed_user_message() {
        use residuum_inference::Role;

        let messages = inbound(Some("[14:05] Sam: build is red")).into_history_messages();
        let [context, user] = messages.as_slice() else {
            panic!("expected context then user message, got {messages:?}");
        };
        assert_eq!(context.role, Role::User);
        assert_eq!(context.content, "[14:05] Sam: build is red");
        assert!(
            context.context,
            "the background context is marked as context"
        );
        assert_eq!(user.role, Role::User);
        assert!(!user.context, "the message itself is not context");
        assert_eq!(user.content, "can you check the build?");
        assert_eq!(user.sender.as_ref().map(|s| s.name.as_str()), Some("Jane"));
    }

    #[test]
    fn agent_sender_on_the_origin_reaches_the_history_message() {
        let mut message = inbound(None);
        message.origin.agent_sender = Some(Box::new(residuum_inference::AgentSender {
            address: "spawned-a".to_string(),
            category: "spawned".to_string(),
            run_id: Some("run-a".to_string()),
        }));
        let messages = message.into_history_messages();
        let [user] = messages.as_slice() else {
            panic!("expected a single user message, got {messages:?}");
        };
        assert_eq!(
            user.agent_sender.as_ref().map(|a| a.address.as_str()),
            Some("spawned-a")
        );
    }

    #[test]
    fn attachments_are_carried_on_the_user_message() {
        let attachment = residuum_inference::MessageAttachment {
            item_id: "item-1".to_string(),
            filename: "notes.md".to_string(),
            size: 12,
            content_type: "text/markdown".to_string(),
            path: "inbox/agent/attachments/item-1/notes.md".to_string(),
        };
        let mut message = inbound(None);
        message.attachments = vec![attachment.clone()];
        let messages = message.into_history_messages();
        let [user] = messages.as_slice() else {
            panic!("expected a single user message, got {messages:?}");
        };
        assert_eq!(user.attachments, vec![attachment]);
    }

    #[test]
    fn a_waiting_message_carries_its_attachments() {
        let attachment = residuum_inference::MessageAttachment {
            item_id: "item-1".to_string(),
            filename: "notes.md".to_string(),
            size: 12,
            content_type: "text/markdown".to_string(),
            path: "inbox/agent/attachments/item-1/notes.md".to_string(),
        };
        let mut message = inbound(None);
        message.attachments = vec![attachment.clone()];
        let Some(MainConversationEvent::MessageQueued { attachments, .. }) =
            MainConversationEvent::message_queued(&message)
        else {
            panic!("expected a queued event for a person's message");
        };
        assert_eq!(attachments, vec![attachment]);
    }

    #[test]
    fn without_context_only_the_user_message() {
        use residuum_inference::Role;

        let messages = inbound(None).into_history_messages();
        let [user] = messages.as_slice() else {
            panic!("expected a single user message, got {messages:?}");
        };
        assert_eq!(user.role, Role::User);
    }
}

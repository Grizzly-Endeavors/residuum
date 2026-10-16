//! Central pub/sub message broker.
//!
//! The bus provides topic-based event routing between subsystems. Publishers
//! send events to named topics; subscribers receive events from topics they
//! have registered interest in. The broker routes each event by `(TopicId,
//! TypeId)` composite key, delivering only to subscribers that match both
//! the topic and event type.

pub mod attachment;
mod broker;
pub mod control;
pub mod conversations;
mod endpoint;
mod events;
mod handle;
pub mod notice;
pub mod origin;
mod registry;
pub mod session;
pub mod topics;
mod types;

pub use broker::{BusHandle, spawn_broker};
pub use endpoint::EndpointCapabilities;
pub use events::{
    A2aTaskSignalEvent, A2aTaskSignalState, AgentMessageEvent, AgentResultEvent, AgentResultStatus,
    ConversationTarget, ConversationTypingEvent, ErrorEvent, EventTrigger, HEARTBEAT_OK,
    HEARTBEAT_URGENT, InlineOutputEvent, IntermediateEvent, MainConversationEvent, MessageEvent,
    NoticeEvent, NotificationEvent, OutboundA2aTaskEvent, PostTurnActivityEvent,
    PostTurnActivityKind, PulseOverlap, ReloadOutcome, ReloadReport, ReloadedFiles, ResponseEvent,
    ResultDisposition, SessionEvent, SessionEventKind, SessionResponseEvent, SpawnRequestEvent,
    ToolCallEvent, ToolResultEvent, TurnLifecycleEvent, TurnOrigin, TurnUsageEvent,
    UserInboxAddedEvent, WorkbenchEvent, WorkspaceEvent, ends_with_sentinel,
};
pub use handle::{Publisher, Subscriber};
pub use registry::{EndpointEntry, EndpointRegistry};
pub use topics::{Carries, Topic};
pub use types::{
    BusError, EndpointId, EndpointName, NotifyName, SYSTEM_CHANNEL, SessionAddress, SkillName,
    TopicId,
};

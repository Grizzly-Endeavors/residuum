//! Central pub/sub message broker.
//!
//! The bus provides topic-based event routing between subsystems. Publishers
//! send events to named topics; subscribers receive events from topics they
//! have registered interest in. The broker routes each event by `(TopicId,
//! TypeId)` composite key, delivering only to subscribers that match both
//! the topic and event type.

mod broker;
mod endpoint;
mod events;
mod handle;
mod registry;
pub mod topics;
mod types;

pub use broker::{BusHandle, spawn_broker};
pub use endpoint::EndpointCapabilities;
pub use events::{
    AgentResultEvent, AgentResultStatus, ErrorEvent, EventTrigger, HeartbeatStatus,
    InlineOutputEvent, IntermediateEvent, MessageEvent, NoticeEvent, NotificationEvent,
    ResponseEvent, SpawnRequestEvent, ToolActivityEvent, ToolCallEvent, ToolResultEvent,
    TurnLifecycleEvent,
};
pub use handle::{Publisher, Subscriber};
pub use registry::{EndpointEntry, EndpointRegistry};
pub use topics::{Carries, Topic};
pub use types::{
    BusError, EndpointId, EndpointName, NotifyName, PresetName, SYSTEM_CHANNEL, TopicId,
};

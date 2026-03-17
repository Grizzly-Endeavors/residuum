//! Central pub/sub message broker.
//!
//! The bus provides topic-based event routing between subsystems. Publishers
//! send events to named topics; subscribers receive events from topics they
//! have registered interest in. The broker task fans out each event to all
//! active subscribers on the target topic.

mod broker;
mod endpoint;
mod events;
mod handle;
mod registry;
pub(crate) mod supervision;
pub mod topics;
mod types;

pub use broker::{BusHandle, spawn_broker};
pub use endpoint::EndpointCapabilities;
pub use events::{
    AgentResultEvent, AgentResultStatus, EventTrigger, HeartbeatStatus, IntermediateEvent,
    MessageEvent, NotificationEvent, ResponseEvent, SpawnRequestEvent, SystemEventData,
    SystemMessageEvent, ToolActivityEvent, ToolCallEvent, ToolResultEvent, TurnLifecycleEvent,
};
pub use handle::{Publisher, Subscriber};
pub use registry::{EndpointEntry, EndpointRegistry};
pub use topics::Topic;
pub use types::{BusError, EndpointId, EndpointName, NotifyName, PresetName, TopicId};

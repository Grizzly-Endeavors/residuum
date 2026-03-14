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
mod types;

pub use broker::{BusHandle, spawn_broker};
pub use endpoint::{EndpointCapabilities, EndpointId};
pub use events::{
    AgentResultEvent, AgentResultStatus, BusEvent, EventTrigger, HeartbeatStatus,
    IntermediateEvent, MessageEvent, NotificationEvent, ResponseEvent, SystemEventData,
    ToolCallEvent, ToolResultEvent,
};
pub use handle::{Publisher, Subscriber};
pub use registry::{EndpointEntry, EndpointRegistry};
pub use types::{BusError, EndpointName, NotifyName, PresetName, TopicId, WebhookName};

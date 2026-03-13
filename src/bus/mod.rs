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
mod types;

pub(crate) use broker::{spawn_broker, BusHandle};
pub(crate) use endpoint::{EndpointCapabilities, EndpointId};
pub(crate) use events::{
    AgentResultEvent, AgentResultStatus, BusEvent, EventTrigger, HeartbeatStatus, MessageEvent,
    NotificationEvent, ResponseEvent,
};
pub(crate) use handle::{Publisher, Subscriber};
pub(crate) use types::{BusError, EndpointName, NotifyName, PresetName, TopicId, WebhookName};

//! The team event log: what happened across the team since the hub started,
//! kept in memory for Home's "Across the team" and shown to clients as it
//! happens.
//!
//! [`TeamEventLog`] holds the entries (see [`event`] for their shape and
//! [`log`] for capacity and eviction). The recorder turns the hub bus and the
//! feed of agent changes into entries, and the hub's HTTP API and WebSocket
//! serve them. Nothing here is written to disk: a new hub process starts an
//! empty log and a new boot id.

pub mod event;
pub mod log;
mod recorder;

pub use event::{
    NewTeamEvent, TeamEvent, TeamEventKind, TeamEventLevel, TeamEventPage, TeamEventPlace,
    TeamEventTarget,
};
pub use log::{
    DEFAULT_PAGE_SIZE, LOG_CAPACITY, MAX_PAGE_SIZE, PROTECTED_ENTRIES, PageQuery, TeamEventLog,
};
pub(crate) use recorder::{TeamEventRecorder, record_hub_started};

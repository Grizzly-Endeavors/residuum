//! The team overview: what Home shows about each agent beyond its state,
//! activity and summary.
//!
//! [`TeamOverview`] keeps one [`AgentOverview`] per agent. A tracker feeds it
//! what the hub learns (agents appearing and going away, and the feed of
//! changes inside running agents), and it reads each part of an overview from
//! where it is kept: the agent's files (a stopped agent's whole overview, and
//! a running agent's schedule and outbound tasks), the session registry and
//! the turn hook's reports for a running one (see [`service`]).
//! The hub's HTTP API serves the overviews, and the hub WebSocket sends an
//! agent's overview whenever any of it changes, at most once a second per
//! agent. See `docs/systems-usage/hub-http.md`.

mod disk;
mod outbound;
pub mod preview;
mod service;
mod tracker;
pub mod types;
mod upcoming;

pub use preview::{PREVIEW_CHARS, plain_preview};
pub use service::{COALESCE_WINDOW, TeamOverview};
pub(crate) use tracker::OverviewTracker;
pub use types::{
    AgentOverview, LastMessage, LastMessageRole, LiveSession, OutboundProblem, OverviewResponse,
    TimePrecision, UpcomingKind, UpcomingRun,
};

#[cfg(test)]
mod tests;

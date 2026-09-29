//! The hub: one process hosting several durable agents as a team.
//!
//! [`types`] holds the shapes shared by the agent host, the hub HTTP API,
//! and the hub WebSocket; [`directory`] is the interface the HTTP surface
//! and the A2A listener use to reach hosted agents. See
//! `docs/design/multi-agent-hub/design.md` and `http-contract.md`.

pub mod directory;
pub mod types;

pub use directory::AgentDirectory;
pub use types::{
    A2aVisibility, Actor, AgentActivity, AgentLastError, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, HubEvent, LifecycleError,
};

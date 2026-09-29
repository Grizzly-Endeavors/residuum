//! The hub: one process hosting several durable agents as a team.
//!
//! [`types`] holds the shapes shared by the agent host, the hub HTTP API,
//! and the hub WebSocket; [`directory`] is the interface the HTTP surface
//! and the A2A listener use to reach hosted agents; [`provision`] writes and
//! removes agent directories. See
//! `docs/design/multi-agent-hub/design.md` and `http-contract.md`.

pub mod directory;
pub mod provision;
pub mod types;

pub use directory::AgentDirectory;
pub use provision::{
    AgentSpec, copy_providers_from, deprovision_agent, first_message, provision_agent,
    restore_agent,
};
pub use types::{
    A2aVisibility, Actor, AgentActivity, AgentLastError, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, HubEvent, LifecycleError,
};

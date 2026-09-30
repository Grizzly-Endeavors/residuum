//! The hub: one process hosting several durable agents as a team.
//!
//! [`types`] holds the shapes shared by the agent host, the hub HTTP API,
//! and the hub WebSocket; [`directory`] is the interface the HTTP surface
//! and the A2A listener use to reach hosted agents; [`provision`] writes and
//! removes agent directories; [`http`] is the hub's HTTP app. See
//! `docs/systems-usage/hub.md` and `docs/systems-usage/hub-http.md`.

pub mod activity;
pub mod directory;
pub mod host;
pub mod http;
pub mod provision;
mod relay_agents;
pub mod runtime;
pub mod services;
pub mod team;
pub(crate) mod team_embedding;
#[cfg(test)]
pub(crate) mod test_support;
pub mod types;

pub use directory::{AgentDirectory, DirectoryHandle};
pub use host::AgentHost;
pub use provision::{
    AgentSpec, copy_providers_from, deprovision_agent, first_message, provision_agent,
    restore_agent,
};
pub use runtime::run_hub;
pub use team::{
    TeamAddressError, TeamLink, TeamRouter, TeamSendError, TeamTarget, parse_team_address,
};
pub use types::{
    A2aVisibility, Actor, AgentActivity, AgentLastError, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, HubEvent, LifecycleError,
};

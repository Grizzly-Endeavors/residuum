//! The hub: one process hosting several durable agents as a team.
//!
//! [`types`] holds the shapes shared by the agent host, the hub HTTP API,
//! and the hub WebSocket; [`directory`] is the interface the HTTP surface
//! uses to reach hosted agents, on top of the runtime's
//! [`crate::background::team::TeamDirectory`]; [`provision`] writes and
//! removes agent directories; [`push`] sends Web Push notifications;
//! [`team_events`] is the log of what happened across the team;
//! [`overview`] is what Home shows about each agent;
//! [`http`] is the hub's HTTP app; [`launch`] is how the host starts an
//! agent through the [`launch::AgentLauncher`] it is given; [`update`], [`features`],
//! [`last_known_good`] and [`config_watcher`] are the process-level pieces
//! the hub runs with. See `docs/systems-usage/hub.md` and
//! `docs/systems-usage/hub-http.md`.

pub mod activity;
pub mod agent_watch;
pub mod config_watcher;
mod deleted;
pub mod directory;
pub mod features;
pub mod file_server;
pub mod host;
pub mod http;
pub mod inbox;
pub mod last_known_good;
pub mod launch;
pub mod overview;
pub mod provision;
pub mod push;
pub(crate) mod relay_agents;
pub mod runtime;
pub mod services;
#[cfg(test)]
mod static_directory;
pub(crate) mod team_embedding;
pub mod team_events;
#[cfg(test)]
pub(crate) mod test_support;
pub mod types;
pub mod update;
pub mod web;
pub(crate) mod workbench_watcher;

pub use directory::{AgentDirectory, AgentFiles};
pub use host::AgentHost;
pub use provision::{
    AgentSpec, RestoreSource, copy_providers_from, deprovision_agent, first_message,
    provision_agent, restore_agent,
};
pub use runtime::{FilterReload, HubExit, HubInputs, run_hub};
pub use types::{AgentActivity, AgentPatch, DeletedAgent, HubEvent, RestoreAgentRequest};

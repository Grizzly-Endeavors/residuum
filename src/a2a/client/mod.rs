//! The A2A client: reaching other agents (and this user's own other
//! instances) through the native `list_agents`/`message_agent`/`stop_agent`
//! tools. See `docs/systems-usage/a2a.md`.
//!
//! - [`config`] loads `config/a2a.json`.
//! - [`hub`]'s [`A2aClientHub`] holds the registered agents, resolves and
//!   caches their cards, and builds protocol clients on demand.
//! - [`tracker`]'s [`RemoteTaskTracker`] persists and watches tasks this
//!   instance started on other agents, delivering the outcome back through
//!   [`crate::background::messaging::AgentMessenger`].
//! - [`siblings`] watches the tunnel connection and registers this user's
//!   other instances in the hub, discovered through the relay directory.

pub mod config;
#[cfg(test)]
mod e2e_tests;
pub mod hub;
pub mod siblings;
pub mod tracker;

pub use config::{
    A2aAgentEntry, is_valid_agent_name, load_a2a_agents_map, validate_a2a_agents_json,
};
pub use hub::{A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, NegotiatedClient};
pub use tracker::{RemoteTaskTracker, TrackedTask};

pub(crate) use siblings::spawn_sibling_discovery;

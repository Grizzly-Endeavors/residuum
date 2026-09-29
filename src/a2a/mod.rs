//! `Agent2Agent` (A2A) protocol support: the caller-key store, each agent's
//! card, the hub's dedicated listener and its per-request auth, the per-agent
//! server assembly ([`agent_a2a_router`]), the session executor, the
//! persistent task store, `ResiduumA2aHandler`, and the outbound client. See
//! `docs/systems-usage/a2a.md`.
//!
//! [`client`] is the client side: reaching other agents through the native
//! `list_agents`/`message_agent`/`stop_agent` tools. The web settings UI lives
//! elsewhere and is not part of this module.

pub mod agent_router;
pub mod auth;
pub mod card;
pub mod client;
pub mod executor;
pub mod handler;
pub mod keys;
pub mod keys_runtime;
pub mod listener;
#[cfg(test)]
mod listener_tests;
pub mod public_url;
#[cfg(test)]
mod server_e2e_tests;
#[cfg(test)]
pub mod static_directory;
pub mod task_store;

pub use agent_router::{AgentA2a, AgentA2aState, agent_a2a_router};
pub use auth::{
    AUTH_CHECK_PATH, Admission, AuthState, CALLER_HEADER, Caller, NoTunnel, TunnelNonceSource,
    authorize,
};
pub use card::{
    AgentCardFile, AgentCardSkillFile, CardError, CardRuntime, CardState, SharedCardState,
    build_agent_card,
};
pub use client::{
    A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, RemoteTaskTracker,
    SiblingFanout, TrackedTask, validate_a2a_agents_json,
};
pub use executor::SessionExecutor;
pub use handler::{ResiduumA2aHandler, resume_in_progress_tasks};
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler, agent_handler_router, hub_a2a_app};
#[cfg(test)]
pub use static_directory::StaticAgentDirectory;
pub use task_store::{FileTaskStore, SharedTaskStore};

pub(crate) use client::spawn_sibling_discovery;

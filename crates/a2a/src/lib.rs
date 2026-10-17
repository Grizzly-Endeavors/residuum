//! `Agent2Agent` (A2A) protocol support: the caller-key store, each agent's
//! card, the hub's dedicated listener and its per-request auth, the per-agent
//! server assembly ([`agent_a2a_router`]), the session executor, the
//! persistent task store, `ResiduumA2aHandler`, and the outbound client. See
//! `docs/systems-usage/a2a.md`.
//!
//! [`client`] is the client side: reaching other agents through the native
//! `list_agents`/`message_agent`/`stop_agent` tools. The web settings UI lives
//! elsewhere and is not part of this module.
//!
//! [`diagnostics_dispatch`] picks the validator for a strictly-parsed workspace
//! or config file by its path. It lives here because a2a is the lowest module
//! below both of its users, the file tools and the workspace file routes, that
//! can reach every validator, `config/a2a.json`'s included.

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod agent_router;
pub mod auth;
pub mod card;
pub mod client;
pub mod diagnostics_dispatch;
pub mod executor;
pub mod handler;
pub mod keys;
pub mod keys_runtime;
pub mod listener;
#[cfg(test)]
mod listener_tests;
pub mod public_url;
#[cfg(any(test, feature = "test-support"))]
pub mod static_directory;
pub mod task_store;

pub use agent_router::{AgentA2a, AgentA2aState, agent_a2a_router};
pub use auth::{AUTH_CHECK_PATH, Admission, AuthState, CALLER_HEADER, Caller, authorize};
pub use card::{
    AgentCardFile, AgentCardSkillFile, CardError, CardRuntime, CardState, SharedCardState,
    build_agent_card,
};
pub use client::{
    A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, RemoteTaskTracker,
    SiblingFanout,
};
pub use executor::SessionExecutor;
pub use handler::{ResiduumA2aHandler, resume_in_progress_tasks};
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler, agent_handler_router, hub_a2a_app};
#[cfg(any(test, feature = "test-support"))]
pub use static_directory::StaticAgentDirectory;
pub use task_store::{FileTaskStore, SharedTaskStore};

pub use client::spawn_sibling_discovery;

//! `Agent2Agent` (A2A) protocol support: the caller-key store, the workspace
//! agent card, the dedicated listener and its auth layer, the session
//! executor, the persistent task store, `ResiduumA2aHandler`, and the
//! outbound client. See `docs/systems-usage/a2a.md`.
//!
//! [`client`] is the client side: reaching other agents through the native
//! `list_agents`/`message_agent`/`stop_agent` tools. The web settings UI lives
//! elsewhere and is not part of this module.

pub mod auth;
pub mod card;
pub mod client;
pub mod executor;
pub mod handler;
pub mod keys;
pub mod keys_runtime;
pub mod listener;
pub mod public_url;
#[cfg(test)]
mod server_e2e_tests;
pub mod task_store;

pub use auth::{
    AUTH_CHECK_PATH, AuthState, CALLER_HEADER, Caller, NoTunnel, TunnelNonceSource, auth_middleware,
};
pub use card::{
    AgentCardFile, AgentCardSkillFile, CardError, CardRuntime, CardState, SharedCardState,
    build_agent_card,
};
pub use client::{
    A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, RemoteTaskTracker,
    TrackedTask, validate_a2a_agents_json,
};
pub use executor::SessionExecutor;
pub use handler::{ResiduumA2aHandler, resume_in_progress_tasks};
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler};
pub use task_store::{FileTaskStore, SharedTaskStore};

pub(crate) use client::spawn_sibling_discovery;
pub(crate) use public_url::{A2aPublicUrl, SharedA2aPublicUrl};
pub(crate) use task_store::DelegatingTaskStore;

pub(crate) use keys::{KEYS_FILE, LOCK_FILE};

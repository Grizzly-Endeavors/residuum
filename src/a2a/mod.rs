//! `Agent2Agent` (A2A) protocol support: the caller-key store, the workspace
//! agent card, the dedicated listener, its auth layer, and the outbound
//! client. See `docs/systems-usage/a2a.md`.
//!
//! [`client`] is the client side: reaching other agents through the native
//! `list_agents`/`message_agent`/`stop_agent` tools. The task executor and
//! persistent task store (wired into [`listener::A2aListener`] in place of
//! [`listener::StubHandler`]) and the web settings UI live elsewhere and are
//! not part of this module.

pub mod auth;
pub mod card;
pub mod client;
pub mod keys;
pub mod keys_runtime;
pub mod listener;

pub use auth::{
    AUTH_CHECK_PATH, AuthState, CALLER_HEADER, Caller, NoTunnel, TunnelNonceSource, auth_middleware,
};
pub use card::{
    AgentCardFile, AgentCardSkillFile, CardError, CardRuntime, CardState, SharedCardState,
    build_agent_card,
};
pub use client::{
    A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, RemoteTaskTracker, TrackedTask,
};
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler};

pub(crate) use keys::{KEYS_FILE, LOCK_FILE};

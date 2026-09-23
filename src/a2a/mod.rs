//! `Agent2Agent` (A2A) protocol support: the caller-key store, the workspace
//! agent card, the dedicated listener, and its auth layer. See
//! `docs/systems-usage/a2a.md`.
//!
//! This module is the foundation other A2A work builds on: the task
//! executor and persistent task store (wired into [`listener::A2aListener`]
//! in place of [`listener::StubHandler`]), the outbound client, and the web
//! settings UI all live elsewhere and are not part of this module.

pub mod auth;
pub mod card;
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
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler};

pub(crate) use keys::{KEYS_FILE, LOCK_FILE};

//! Agent keys: credentials the agent can use in spawned commands without the
//! values entering its context.
//!
//! Values live in a dedicated encrypted store ([`store`]), are injected as
//! environment variables into `exec` children and MCP servers, and are
//! scrubbed from every tool result and trace export by a [`Redactor`]. See
//! `docs/systems-usage/agent-keys.md`.

mod redact;
mod references;
mod runtime;
mod store;

pub use redact::{Redactor, marker_for};
pub use references::{expand_references, has_references};
pub use runtime::{AgentKeys, AgentKeysSnapshot, SharedAgentKeys};
pub use store::{
    AgentKeyInfo, AgentKeyStore, KeyCreator, env_var_for, validate_name, validate_value,
};
pub(crate) use store::{ENCRYPTED_FILE, KEY_FILE, LOCK_FILE};

use thiserror::Error;

/// Errors from agent-key operations. Messages are written to be shown to the
/// agent or the user as-is.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum AgentKeyError {
    /// A name or value broke the storage rules.
    #[error("{0}")]
    Invalid(String),
    /// No key by that name.
    #[error("no agent key named '{0}'")]
    NotFound(String),
    /// Reading, decrypting, or writing the store failed.
    #[error("agent key store unavailable: {0}")]
    Storage(String),
}

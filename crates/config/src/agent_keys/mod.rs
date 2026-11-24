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
pub(crate) use store::{ENCRYPTED_FILE, LOCK_FILE};

use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum AgentKeyError {
    #[error("{0}")]
    Invalid(String),
    #[error("no agent key named '{0}'")]
    NotFound(String),
    #[error("agent key store unavailable: {0}")]
    Storage(String),
}

//! Fatal error types for `Residuum`.
//!
//! [`FatalError`] is reserved for errors that terminate the process: startup
//! failures, unrecoverable config issues, and CLI command failures. Runtime
//! subsystem errors use `anyhow::Error` and are handled locally with tracing
//! and bus publishing.

use crate::models::ModelError;

/// Fatal errors that terminate the process.
#[derive(Debug, thiserror::Error)]
pub enum FatalError {
    /// Configuration loading or validation failed
    #[error("config error: {0}")]
    Config(String),

    /// Workspace directory operations failed
    #[error("workspace error: {0}")]
    Workspace(String),

    /// Model provider error
    #[error(transparent)]
    Model(#[from] ModelError),

    /// Memory subsystem error
    #[error("memory error: {0}")]
    Memory(String),

    /// CLI interface error
    #[error("interface error: {0}")]
    Interface(String),

    /// Scheduling error (pulse or actions)
    #[error("scheduling error: {0}")]
    Scheduling(String),

    /// WebSocket gateway error
    #[error("gateway error: {0}")]
    Gateway(String),

    /// Projects subsystem error
    #[error("projects error: {0}")]
    Projects(String),

    /// Skills subsystem error
    #[error("skills error: {0}")]
    Skills(String),

    /// Subagent presets subsystem error
    #[error("subagents error: {0}")]
    Subagents(String),

    /// Catch-all for other errors
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

//! Fatal error types for `Residuum`.
//!
//! [`FatalError`] is reserved for errors that terminate the process: startup
//! failures, unrecoverable config issues, and CLI command failures. Runtime
//! subsystem errors use `anyhow::Error` and are handled locally with tracing
//! and bus publishing.

/// Fatal errors that terminate the process.
#[derive(Debug, thiserror::Error)]
pub enum FatalError {
    /// Configuration loading or validation failed
    #[error("config error: {0}")]
    Config(String),

    /// Workspace directory operations failed
    #[error("workspace error: {0}")]
    Workspace(String),

    /// WebSocket gateway error
    #[error("gateway error: {0}")]
    Gateway(String),

    /// Catch-all for other errors
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

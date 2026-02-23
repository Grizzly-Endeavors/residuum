//! Crate-level error types for `IronClaw`.

use crate::models::ModelError;
use crate::tools::ToolError;

/// Top-level error type for the `IronClaw` agent gateway.
#[derive(Debug, thiserror::Error)]
pub enum IronclawError {
    /// Configuration loading or validation failed
    #[error("config error: {0}")]
    Config(String),

    /// Workspace directory operations failed
    #[error("workspace error: {0}")]
    Workspace(String),

    /// Model provider error
    #[error(transparent)]
    Model(#[from] ModelError),

    /// Tool execution error
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// Memory subsystem error
    #[error("memory error: {0}")]
    Memory(String),

    /// CLI channel error
    #[error("channel error: {0}")]
    Channel(String),

    /// Scheduling error (pulse or cron)
    #[error("scheduling error: {0}")]
    Scheduling(String),

    /// WebSocket gateway error
    #[error("gateway error: {0}")]
    Gateway(String),

    /// Projects subsystem error
    #[error("projects error: {0}")]
    Projects(String),

    /// Catch-all for other errors
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

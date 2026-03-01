//! Crate-level error types for `Residuum`.

use crate::models::ModelError;
use crate::tools::ToolError;

/// Top-level error type for the `Residuum` agent gateway.
#[derive(Debug, thiserror::Error)]
pub enum ResiduumError {
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

    /// Inbox subsystem error
    #[error("inbox error: {0}")]
    Inbox(String),

    /// Catch-all for other errors
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

//! MCP (Model Context Protocol) server lifecycle management.

pub mod client;
pub mod registry;
pub mod types;

pub use registry::{
    McpReconcileReport, McpReconcileResult, McpRegistry, McpServerState, McpStatus,
    SharedMcpRegistry,
};
pub use types::{McpServerEntry, McpTransport};

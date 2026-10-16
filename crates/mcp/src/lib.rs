//! MCP (Model Context Protocol) server lifecycle management.

pub mod client;
pub(crate) mod registry;
pub mod types;

pub use registry::{McpReconcileReport, McpRegistry, McpServerState, McpStatus, SharedMcpRegistry};
pub use types::{McpServerEntry, McpTransport};

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

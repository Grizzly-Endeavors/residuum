//! MCP (Model Context Protocol) server lifecycle management.

pub mod registry;

pub use registry::{McpReconcileResult, McpRegistry, McpServerState, McpStatus, SharedMcpRegistry};

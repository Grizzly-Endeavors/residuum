//! MCP server configuration types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Transport type for MCP server connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Stdio transport: spawn a child process and communicate over stdin/stdout.
    #[default]
    Stdio,
    /// HTTP transport: connect to a remote MCP server over Streamable HTTP.
    Http,
}

impl McpTransport {
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde skip_serializing_if requires &T signature"
    )]
    fn is_default(&self) -> bool {
        matches!(self, Self::Stdio)
    }
}

/// MCP server entry loaded from `mcp.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Server name.
    pub name: String,
    /// Command (stdio) or URL (http) for the server.
    pub command: String,
    /// Command-line arguments (only used for stdio transport).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables to pass to the server process (only used for stdio transport).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Transport type (defaults to stdio).
    #[serde(default, skip_serializing_if = "McpTransport::is_default")]
    pub transport: McpTransport,
    /// HTTP headers to send with requests (only used for http transport).
    /// Header values support `${VAR}` and `${VAR:-default}` env var interpolation
    /// which is expanded at connect time.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

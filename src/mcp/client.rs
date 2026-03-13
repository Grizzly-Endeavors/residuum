//! MCP client wrapper around `rmcp`.
//!
//! Manages a single connection to an MCP server process, providing
//! tool listing and invocation backed by the rmcp SDK.

use std::borrow::Cow;
use std::time::Duration;

use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, CallToolResult, Content};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::projects::types::{McpServerEntry, McpTransport};
use crate::tools::{ToolError, ToolResult};

/// Default timeout for MCP tool calls (seconds).
const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// A live connection to a single MCP server process.
pub struct McpClient {
    service: RunningService<RoleClient, ()>,
    server_name: String,
}

impl McpClient {
    /// Connect to an MCP server and complete the protocol handshake.
    ///
    /// Dispatches on the transport type:
    /// - **Stdio**: spawns a child process and communicates over stdin/stdout
    /// - **Http**: connects to a remote server via Streamable HTTP
    ///
    /// # Errors
    /// Returns an error if the connection cannot be established or the MCP
    /// handshake fails.
    pub async fn connect(entry: &McpServerEntry) -> Result<Self, anyhow::Error> {
        match entry.transport {
            McpTransport::Stdio => Self::connect_stdio(entry).await,
            McpTransport::Http => Self::connect_http(entry).await,
        }
    }

    async fn connect_stdio(entry: &McpServerEntry) -> Result<Self, anyhow::Error> {
        let mut cmd = tokio::process::Command::new(&entry.command);
        cmd.args(&entry.args);
        for (key, val) in &entry.env {
            cmd.env(key, val);
        }

        let transport = TokioChildProcess::new(cmd)
            .map_err(|e| anyhow::anyhow!("failed to spawn mcp server '{}': {e}", entry.name))?;

        let service = ().serve(transport).await.map_err(|e| {
            anyhow::anyhow!("mcp handshake failed for server '{}': {e}", entry.name)
        })?;

        Ok(Self {
            service,
            server_name: entry.name.clone(),
        })
    }

    async fn connect_http(entry: &McpServerEntry) -> Result<Self, anyhow::Error> {
        let config = StreamableHttpClientTransportConfig::with_uri(entry.command.as_str());
        let transport = StreamableHttpClientTransport::<reqwest::Client>::from_config(config);

        let service = ().serve(transport).await.map_err(|e| {
            anyhow::anyhow!(
                "mcp http connection failed for server '{}' at {}: {e}",
                entry.name,
                entry.command
            )
        })?;

        Ok(Self {
            service,
            server_name: entry.name.clone(),
        })
    }

    /// List all tools advertised by this MCP server.
    ///
    /// Handles pagination automatically via `list_all_tools()`.
    ///
    /// # Errors
    /// Returns an error if the RPC call fails.
    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>, anyhow::Error> {
        let tools = self.service.peer().list_all_tools().await.map_err(|e| {
            anyhow::anyhow!(
                "failed to list tools from mcp server '{}': {e}",
                self.server_name
            )
        })?;

        let definitions = tools
            .into_iter()
            .map(|t| ToolDefinition {
                name: t.name.into_owned(),
                description: t.description.map(Cow::into_owned).unwrap_or_default(),
                parameters: Value::Object(t.input_schema.as_ref().clone()),
            })
            .collect();

        Ok(definitions)
    }

    /// Call a tool on this MCP server.
    ///
    /// # Errors
    /// Returns `ToolError::Execution` if the RPC call fails.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
                return Err(ToolError::InvalidArguments(
                    "mcp tool arguments must be an object".to_string(),
                ));
            }
        };

        let params = CallToolRequestParams {
            meta: None,
            name: Cow::Owned(name.to_string()),
            arguments,
            task: None,
        };

        let result: CallToolResult =
            tokio::time::timeout(TOOL_CALL_TIMEOUT, self.service.peer().call_tool(params))
                .await
                .map_err(|_elapsed| {
                    ToolError::Execution(format!(
                        "mcp tool call '{name}' on server '{}' timed out after {}s",
                        self.server_name,
                        TOOL_CALL_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(|e| {
                    ToolError::Execution(format!(
                        "mcp tool call '{name}' on server '{}' failed: {e}",
                        self.server_name
                    ))
                })?;

        let is_error = result.is_error.unwrap_or(false);
        let output = extract_text_content(&result.content);

        if is_error {
            tracing::debug!(
                tool = %name,
                server = %self.server_name,
                "mcp tool returned error response"
            );
        }

        Ok(ToolResult {
            output,
            is_error,
            images: vec![],
        })
    }

    /// Gracefully shut down the MCP server connection.
    pub async fn shutdown(self) {
        if let Err(e) = self.service.cancel().await {
            tracing::warn!(
                server = %self.server_name,
                error = %e,
                "mcp server shutdown returned error"
            );
        }
    }
}

/// Extract text from MCP content blocks, joining multiple blocks with newlines.
fn extract_text_content(content: &[Content]) -> String {
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
        .collect();
    texts.join("\n")
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn connect_http_invalid_url_returns_error() {
        let entry = McpServerEntry {
            name: "bad-http".to_string(),
            command: "http://127.0.0.1:1/nonexistent".to_string(),
            args: vec![],
            env: HashMap::new(),
            transport: McpTransport::Http,
        };

        let result = McpClient::connect(&entry).await;
        assert!(result.is_err(), "http connect to invalid URL should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("bad-http"),
            "error should mention server name: {err}"
        );
    }

    #[tokio::test]
    async fn connect_stdio_nonexistent_binary_returns_error() {
        let entry = McpServerEntry {
            name: "bad-stdio".to_string(),
            command: "/nonexistent/binary".to_string(),
            args: vec![],
            env: HashMap::new(),
            transport: McpTransport::Stdio,
        };

        let result = McpClient::connect(&entry).await;
        assert!(
            result.is_err(),
            "stdio connect to nonexistent binary should fail"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("bad-stdio"),
            "error should mention server name: {err}"
        );
    }
}

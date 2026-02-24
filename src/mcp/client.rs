//! MCP client wrapper around `rmcp`.
//!
//! Manages a single connection to an MCP server process, providing
//! tool listing and invocation backed by the rmcp SDK.

use std::borrow::Cow;

use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, CallToolResult, Content};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::projects::types::McpServerEntry;
use crate::tools::{ToolError, ToolResult};

/// A live connection to a single MCP server process.
pub struct McpClient {
    service: RunningService<RoleClient, ()>,
    server_name: String,
}

impl McpClient {
    /// Spawn an MCP server process and complete the protocol handshake.
    ///
    /// # Errors
    /// Returns an error if the process cannot be spawned or the MCP
    /// handshake fails.
    pub async fn connect(entry: &McpServerEntry) -> Result<Self, anyhow::Error> {
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

        let result: CallToolResult = self.service.peer().call_tool(params).await.map_err(|e| {
            ToolError::Execution(format!(
                "mcp tool call '{name}' on server '{}' failed: {e}",
                self.server_name
            ))
        })?;

        let is_error = result.is_error.unwrap_or(false);
        let output = extract_text_content(&result.content);

        Ok(ToolResult { output, is_error })
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

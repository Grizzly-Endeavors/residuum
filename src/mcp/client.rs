//! MCP client wrapper around `rmcp`.
//!
//! Manages a single connection to an MCP server process, providing
//! tool listing and invocation backed by the rmcp SDK.

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;

use http::{HeaderName, HeaderValue};
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
        tracing::debug!(server = %entry.name, command = %entry.command, "connecting to mcp server (stdio)");
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
        tracing::debug!(server = %entry.name, url = %entry.command, "connecting to mcp server (http)");
        let mut config = StreamableHttpClientTransportConfig::with_uri(entry.command.as_str());

        if !entry.headers.is_empty() {
            let expanded = expand_header_env_vars(&entry.headers)?;
            config = config.custom_headers(expanded);
        }

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
        tracing::debug!(tool = %name, server = %self.server_name, "dispatching mcp tool call");
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
            tracing::warn!(
                tool = %name,
                server = %self.server_name,
                output = %output,
                "mcp tool returned error response"
            );
        } else {
            tracing::debug!(tool = %name, server = %self.server_name, "mcp tool call completed");
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
        } else {
            tracing::debug!(server = %self.server_name, "mcp server shutdown complete");
        }
    }
}

/// Expand `${VAR}` and `${VAR:-default}` patterns in a string using environment variables.
///
/// Unresolved variables with no default are replaced with an empty string.
#[must_use]
pub fn expand_env_vars(input: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}:]+?)(?::-(.*?))?\}");
    let Ok(re) = re else {
        tracing::error!("failed to compile env var expansion regex — returning input unexpanded");
        return input.to_string();
    };
    re.replace_all(input, |caps: &regex::Captures<'_>| {
        let var_name = caps.get(1).map_or("", |m| m.as_str());
        match std::env::var(var_name) {
            Ok(val) => val,
            Err(_) => caps.get(2).map_or("", |m| m.as_str()).to_string(),
        }
    })
    .into_owned()
}

/// Expand env vars in header values and convert to HTTP header types.
///
/// # Errors
/// Returns an error if any header name or expanded value is invalid.
fn expand_header_env_vars(
    headers: &HashMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, anyhow::Error> {
    let mut result = HashMap::with_capacity(headers.len());
    for (name, value) in headers {
        let header_name = HeaderName::try_from(name.as_str())
            .map_err(|e| anyhow::anyhow!("invalid header name '{name}': {e}"))?;
        let expanded = expand_env_vars(value);
        let header_value = HeaderValue::try_from(expanded.as_str())
            .map_err(|e| anyhow::anyhow!("invalid header value for '{name}': {e}"))?;
        result.insert(header_name, header_value);
    }
    Ok(result)
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
#[expect(
    unsafe_code,
    reason = "tests use set_var/remove_var which are unsafe in edition 2024"
)]
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
            headers: HashMap::new(),
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
            headers: HashMap::new(),
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

    #[test]
    fn expand_env_vars_simple() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::set_var("TEST_MCP_VAR", "hello") };
        assert_eq!(
            expand_env_vars("Bearer ${TEST_MCP_VAR}"),
            "Bearer hello",
            "should expand simple env var"
        );
        unsafe { std::env::remove_var("TEST_MCP_VAR") };
    }

    #[test]
    fn expand_env_vars_with_default() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::remove_var("TEST_MCP_MISSING") };
        assert_eq!(
            expand_env_vars("${TEST_MCP_MISSING:-fallback}"),
            "fallback",
            "should use default when var is missing"
        );
    }

    #[test]
    fn expand_env_vars_no_pattern() {
        assert_eq!(
            expand_env_vars("plain string"),
            "plain string",
            "should pass through strings without patterns"
        );
    }

    #[test]
    fn expand_env_vars_missing_no_default() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::remove_var("TEST_MCP_GONE") };
        assert_eq!(
            expand_env_vars("prefix-${TEST_MCP_GONE}-suffix"),
            "prefix--suffix",
            "should replace with empty string when no default"
        );
    }

    #[test]
    fn expand_env_vars_multiple_vars() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::set_var("TEST_MCP_A", "aaa") };
        unsafe { std::env::set_var("TEST_MCP_B", "bbb") };
        assert_eq!(
            expand_env_vars("${TEST_MCP_A}:${TEST_MCP_B}"),
            "aaa:bbb",
            "should expand multiple vars"
        );
        unsafe { std::env::remove_var("TEST_MCP_A") };
        unsafe { std::env::remove_var("TEST_MCP_B") };
    }
}

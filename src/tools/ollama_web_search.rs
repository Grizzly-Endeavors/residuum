//! Ollama Cloud web search tool.

use std::fmt::Write as _;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Default number of search results to return.
const DEFAULT_MAX_RESULTS: u64 = 5;

/// Tool for searching the web using Ollama Cloud's web search API.
pub(crate) struct OllamaWebSearchTool {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OllamaWebSearchTool {
    /// Create a new Ollama web search tool.
    pub(crate) fn new(api_key: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "failed to build HTTP client for ollama web search, using default");
                reqwest::Client::default()
            });
        Self {
            api_key,
            base_url,
            http,
        }
    }
}

#[async_trait]
impl Tool for OllamaWebSearchTool {
    fn name(&self) -> &'static str {
        "ollama_web_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description:
                "Search the web using Ollama Cloud. Returns search results with titles, URLs, \
                 and snippets."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'query' parameter".into())
            })?;

        let max_results = arguments
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        debug!(query = %query, max_results = max_results, "ollama cloud web search");

        let url = format!("{}/api/web_search", self.base_url);

        let body = serde_json::json!({
            "query": query,
            "max_results": max_results
        });

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                ToolError::Execution(format!("failed to call ollama web search API: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            tracing::warn!(status = %status, "ollama web search API returned non-2xx response");
            return Ok(ToolResult::error(format!(
                "ollama web search API returned HTTP {status}: {error_body}"
            )));
        }

        let response_json: Value = response.json().await.map_err(|e| {
            ToolError::Execution(format!("failed to parse ollama web search response: {e}"))
        })?;

        let output = format_search_results(&response_json);
        Ok(ToolResult::success(output))
    }
}

/// Format search results from the Ollama Cloud API response into readable text.
fn format_search_results(response: &Value) -> String {
    let mut output = String::new();

    // Try to extract results from common response shapes
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .or_else(|| response.get("data").and_then(Value::as_array))
        .or_else(|| response.as_array());

    let Some(items) = results else {
        return format!("Search response:\n{response}");
    };

    if items.is_empty() {
        return "No search results found.".to_string();
    }

    writeln!(output, "Found {} result(s):", items.len()).ok();

    for (i, item) in items.iter().enumerate() {
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(no title)");
        let url = item
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("(no url)");
        let snippet = item
            .get("snippet")
            .or_else(|| item.get("description"))
            .or_else(|| item.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("(no snippet)");

        write!(
            output,
            "\n{}. {}\n   URL: {}\n   {}\n",
            i + 1,
            title,
            url,
            snippet
        )
        .ok();
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_results_with_items() {
        let json = serde_json::json!({
            "results": [
                {
                    "title": "Rust Programming",
                    "url": "https://rust-lang.org",
                    "snippet": "A systems programming language"
                },
                {
                    "title": "Tokio",
                    "url": "https://tokio.rs",
                    "snippet": "An async runtime for Rust"
                }
            ]
        });

        let output = format_search_results(&json);
        assert!(
            output.contains("Found 2 result(s)"),
            "should show result count"
        );
        assert!(
            output.contains("Rust Programming"),
            "should contain first title"
        );
        assert!(
            output.contains("https://tokio.rs"),
            "should contain second URL"
        );
    }

    #[test]
    fn format_results_empty() {
        let json = serde_json::json!({ "results": [] });
        let output = format_search_results(&json);
        assert_eq!(
            output, "No search results found.",
            "should report no results"
        );
    }

    #[test]
    fn format_results_missing_fields() {
        let json = serde_json::json!({
            "results": [{ "title": "Only Title" }]
        });
        let output = format_search_results(&json);
        assert!(output.contains("Only Title"), "should show available title");
        assert!(output.contains("(no url)"), "should show placeholder url");
        assert!(
            output.contains("(no snippet)"),
            "should show placeholder snippet"
        );
    }

    #[test]
    fn format_results_fallback_to_raw() {
        let json = serde_json::json!({ "status": "ok" });
        let output = format_search_results(&json);
        assert!(
            output.contains("Search response:"),
            "should fall back to raw output"
        );
    }

    #[test]
    fn definition_has_correct_name() {
        let tool = OllamaWebSearchTool::new("key".into(), "http://localhost".into());
        let def = tool.definition();
        assert_eq!(def.name, "ollama_web_search", "tool name should match");
    }

    #[tokio::test]
    async fn missing_query_returns_error() {
        let tool = OllamaWebSearchTool::new("key".into(), "http://localhost".into());
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing query should error");
    }
}

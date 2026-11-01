//! Ollama Cloud web search tool.

use std::fmt::Write as _;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use residuum_inference::ToolDefinition;

use residuum_agent::Tool;
use residuum_inference::{ToolError, ToolResult};

use super::split_chars;

/// Default number of search results to return.
const DEFAULT_MAX_RESULTS: u64 = 5;

/// The most results the Ollama Cloud search API returns per call.
const MAX_RESULTS: u64 = 10;

/// Snippet characters shown per result; the rest is left out with a note.
const SNIPPET_MAX_CHARS: usize = 500;

/// Characters of an unrecognised response shown raw; the rest is left out.
const RAW_RESPONSE_MAX_CHARS: usize = 2_000;

/// The `max_results` the call asks for: the default when it names none, and
/// never more than [`MAX_RESULTS`].
fn requested_max_results(arguments: &Value) -> u64 {
    arguments
        .get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_RESULTS)
}

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
                tracing::warn!(error = %e, "failed to build HTTP client for ollama web search, using default");
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
                        "maximum": MAX_RESULTS,
                        "description": "Maximum number of results to return (default: 5, at most 10)"
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

        let max_results = requested_max_results(&arguments);

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
            tracing::warn!(status = %status, query = %query, "ollama web search API returned non-2xx response");
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
        return raw_response(response);
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
            .map_or_else(|| "(no snippet)".to_string(), snippet_text);

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

/// A result's snippet, cut to [`SNIPPET_MAX_CHARS`] with a note that says how
/// much was left out and where the rest is.
fn snippet_text(snippet: &str) -> String {
    let (shown, omitted) = split_chars(snippet, SNIPPET_MAX_CHARS);
    if omitted == 0 {
        return shown.to_string();
    }
    format!("{shown}… [snippet cut, {omitted} more characters; open the URL for the rest]")
}

/// An unrecognised response, dumped raw up to [`RAW_RESPONSE_MAX_CHARS`]. The
/// note gives the full length so the reader knows what was left out.
fn raw_response(response: &Value) -> String {
    let text = response.to_string();
    let (shown, omitted) = split_chars(&text, RAW_RESPONSE_MAX_CHARS);
    if omitted == 0 {
        return format!("Search response:\n{text}");
    }
    let total = RAW_RESPONSE_MAX_CHARS + omitted;
    format!(
        "Search response (first {RAW_RESPONSE_MAX_CHARS} of {total} characters):\n{shown}… [{omitted} more characters not shown]"
    )
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

    #[test]
    fn definition_states_the_max_results_maximum() {
        let tool = OllamaWebSearchTool::new("key".into(), "http://localhost".into());
        let schema = tool.definition().parameters;
        assert_eq!(
            schema.pointer("/properties/max_results/maximum"),
            Some(&serde_json::json!(MAX_RESULTS)),
            "schema should carry the maximum"
        );
    }

    #[test]
    fn max_results_defaults_and_clamps_to_the_api_maximum() {
        assert_eq!(
            requested_max_results(&serde_json::json!({"query": "q"})),
            DEFAULT_MAX_RESULTS
        );
        assert_eq!(
            requested_max_results(&serde_json::json!({"query": "q", "max_results": 3})),
            3
        );
        assert_eq!(
            requested_max_results(&serde_json::json!({"query": "q", "max_results": 500})),
            MAX_RESULTS
        );
    }

    #[test]
    fn long_snippet_is_cut_and_says_how_much_was_left_out() {
        let snippet = "a".repeat(SNIPPET_MAX_CHARS + 100);
        let json = serde_json::json!({
            "results": [{ "title": "T", "url": "https://example.com", "snippet": snippet }]
        });
        let output = format_search_results(&json);
        assert!(
            output.contains(&format!(
                "{}… [snippet cut, 100 more characters; open the URL for the rest]",
                "a".repeat(SNIPPET_MAX_CHARS)
            )),
            "{output}"
        );
    }

    #[test]
    fn snippet_at_the_limit_is_shown_whole() {
        let snippet = "b".repeat(SNIPPET_MAX_CHARS);
        assert_eq!(snippet_text(&snippet), snippet);
    }

    #[test]
    fn unrecognised_response_is_dumped_up_to_the_cap_with_its_full_length() {
        let json = serde_json::json!({ "status": "x".repeat(RAW_RESPONSE_MAX_CHARS) });
        let total = json.to_string().chars().count();
        let output = format_search_results(&json);
        let omitted = total - RAW_RESPONSE_MAX_CHARS;
        assert!(
            output.starts_with(&format!(
                "Search response (first {RAW_RESPONSE_MAX_CHARS} of {total} characters):\n"
            )),
            "{output}"
        );
        assert!(
            output.ends_with(&format!("… [{omitted} more characters not shown]")),
            "{output}"
        );
    }

    #[test]
    fn short_unrecognised_response_is_dumped_whole() {
        let json = serde_json::json!({ "status": "ok" });
        assert_eq!(
            format_search_results(&json),
            format!("Search response:\n{json}")
        );
    }

    #[tokio::test]
    async fn missing_query_returns_error() {
        let tool = OllamaWebSearchTool::new("key".into(), "http://localhost".into());
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing query should error");
    }
}

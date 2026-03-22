//! Web page content fetcher optimized for LLM consumption.

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use crate::models::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Maximum content length returned to avoid context window blowout.
const MAX_CONTENT_CHARS: usize = 50_000;

/// Tool for fetching web page content and extracting readable text.
pub(crate) struct WebFetchTool {
    http: reqwest::Client,
}

impl WebFetchTool {
    /// Create a new web fetch tool with a dedicated HTTP client.
    #[must_use]
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (compatible; Residuum/1.0; +https://github.com/residuum)")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "failed to build HTTP client for web fetch, using default");
                reqwest::Client::default()
            });
        Self { http }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch a web page and extract its main content as readable text. \
                          Returns the page title and cleaned content, optimized for reading. \
                          Use this to read articles, documentation, or any web page."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let url = arguments
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required 'url' parameter".into())
            })?;

        debug!(url = %url, "fetching web page");

        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to fetch {url}: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolResult::error(format!("HTTP {status} fetching {url}")));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("text/html") && !content_type.contains("text/plain") {
            return Ok(ToolResult::error(format!(
                "unsupported content type: {content_type}"
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read response body: {e}")))?;

        if content_type.contains("text/plain") {
            let content = truncate_content(&html);
            return Ok(ToolResult::success(content));
        }

        // Extract readable content from HTML
        match extract_content(&html, url) {
            Ok(text) => {
                let content = truncate_content(&text);
                Ok(ToolResult::success(content))
            }
            Err(msg) => {
                warn!(url = %url, error = %msg, "content extraction failed, returning raw text");
                // Fall back to basic text extraction
                let fallback = strip_html_tags(&html);
                let content = truncate_content(&fallback);
                Ok(ToolResult::success(content))
            }
        }
    }
}

/// Extract readable content from HTML using readability algorithm.
fn extract_content(html: &str, url: &str) -> Result<String, String> {
    let mut readability =
        dom_smoothie::Readability::new(html, Some(url), None).map_err(|e| e.to_string())?;
    let article = readability.parse().map_err(|e| e.to_string())?;

    let mut output = String::new();
    if !article.title.is_empty() {
        output.push_str("# ");
        output.push_str(&article.title);
        output.push_str("\n\n");
    }
    // article.content is cleaned HTML; strip remaining tags for plain text
    output.push_str(&strip_html_tags(&article.content));
    Ok(output)
}

/// Basic HTML tag stripping for fallback content extraction.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_whitespace = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => {
                let is_ws = ch.is_whitespace();
                if is_ws && last_was_whitespace {
                    continue;
                }
                last_was_whitespace = is_ws;
                result.push(ch);
            }
            _ => {}
        }
    }

    result.trim().to_string()
}

/// Truncate content to a maximum character length with a note.
fn truncate_content(content: &str) -> String {
    if content.len() <= MAX_CONTENT_CHARS {
        return content.to_string();
    }
    let mut truncated = String::with_capacity(MAX_CONTENT_CHARS + 50);
    // Find a safe truncation point (don't split mid-char)
    let boundary = content.floor_char_boundary(MAX_CONTENT_CHARS);
    if let Some(slice) = content.get(..boundary) {
        truncated.push_str(slice);
    }
    truncated.push_str("\n\n[content truncated]");
    truncated
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn strip_html_basic() {
        let html = "<p>Hello <b>world</b></p>";
        assert_eq!(strip_html_tags(html), "Hello world", "should strip tags");
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        let html = "<p>Hello   \n\n   world</p>";
        assert_eq!(
            strip_html_tags(html),
            "Hello world",
            "should collapse whitespace"
        );
    }

    #[test]
    fn truncate_short_content() {
        let short = "hello world";
        assert_eq!(
            truncate_content(short),
            "hello world",
            "short content unchanged"
        );
    }

    #[test]
    fn truncate_long_content() {
        let long = "a".repeat(MAX_CONTENT_CHARS + 100);
        let result = truncate_content(&long);
        assert!(
            result.len() < long.len(),
            "truncated should be shorter than original"
        );
        assert!(
            result.ends_with("[content truncated]"),
            "should end with truncation notice"
        );
    }

    #[tokio::test]
    async fn fetch_html_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(
                        "<html><head><title>Test</title></head>\
                         <body><article><p>Main content here.</p></article></body></html>",
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/article", server.uri())}))
            .await
            .unwrap();

        assert!(!result.is_error, "should succeed");
        assert!(
            result.output.contains("Main content here"),
            "should contain extracted content: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn fetch_404_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/missing", server.uri())}))
            .await
            .unwrap();

        assert!(result.is_error, "404 should be an error");
        assert!(result.output.contains("404"), "should mention status code");
    }

    #[tokio::test]
    async fn fetch_non_html_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/image.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![0_u8; 10]),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/image.png", server.uri())}))
            .await
            .unwrap();

        assert!(result.is_error, "non-HTML should be an error");
        assert!(
            result.output.contains("unsupported content type"),
            "should mention content type"
        );
    }

    #[tokio::test]
    async fn missing_url_returns_error() {
        let tool = WebFetchTool::new();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing url should error");
    }
}

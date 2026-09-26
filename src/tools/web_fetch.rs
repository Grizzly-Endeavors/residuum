//! Web page content fetcher optimized for LLM consumption.

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, warn};

use crate::inference::ToolDefinition;

use super::{Tool, ToolError, ToolResult};

/// Default page size returned per call, to avoid context window blowout.
///
/// Not a hard limit on what can be fetched: `offset` pages through the rest
/// of a longer page across further calls.
const DEFAULT_PAGE_BYTES: usize = 50_000;

/// Tool for fetching web page content and extracting readable text.
pub(crate) struct WebFetchTool {
    http: reqwest::Client,
}

impl WebFetchTool {
    /// Create a new web fetch tool with a dedicated HTTP client.
    #[must_use]
    pub(crate) fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (compatible; Residuum/1.0; +https://github.com/residuum)")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to build HTTP client for web fetch, using default");
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
            name: self.name().to_string(),
            description: "Fetch a web page or other textual URL and extract its readable \
                          content. HTML is cleaned to its main article text; JSON, XML, \
                          plain text, and other textual bodies are returned as-is with their \
                          content type noted. Binary content (images, PDFs, archives, etc.) \
                          is refused. Output is paged: the header reports the total size, and \
                          a page beyond the first is fetched by passing the next `offset` it \
                          reports."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset into the fetched content to start the page from (default: 0). Use the offset from a previous response's \"more content\" note to continue reading."
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
        let offset = arguments
            .get("offset")
            .and_then(Value::as_u64)
            .map_or(0, |o| usize::try_from(o).unwrap_or(usize::MAX));

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

        if !is_textual_content_type(&content_type) {
            return Ok(ToolResult::error(format!(
                "content type '{content_type}' is binary — web_fetch only handles textual \
                 content (HTML, JSON, XML, plain text, and similar)"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("failed to read response body: {e}")))?;

        let is_html = content_type.contains("html");
        let content = if is_html {
            match extract_content(&body, url) {
                Ok(text) => text,
                Err(msg) => {
                    warn!(url = %url, error = %msg, "content extraction failed, returning raw text");
                    strip_html_tags(&body)
                }
            }
        } else {
            body
        };

        // Note the content type on anything other than HTML/plain text, since
        // the caller sees only extracted/raw text with no header otherwise.
        let content_type_note =
            if is_html || content_type.contains("text/plain") || content_type.is_empty() {
                None
            } else {
                Some(format!("content-type: {content_type}"))
            };

        Ok(ToolResult::success(page_content(
            &content,
            offset,
            content_type_note.as_deref(),
        )))
    }
}

/// Textual content types `web_fetch` will return; everything else (images,
/// video, audio, PDFs, archives, etc.) is refused with a clear message.
///
/// Missing/empty content type is treated as textual rather than refused,
/// since some servers omit the header for plain responses.
fn is_textual_content_type(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.is_empty()
        || ct.starts_with("text/")
        || ct.contains("json")
        || ct.contains("xml")
        || ct.contains("yaml")
        || ct.contains("javascript")
        || ct.contains("csv")
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

/// Page `content` starting at byte `offset`, returning at most
/// `DEFAULT_PAGE_BYTES` of it with a header reporting the total size and,
/// when more remains, the `offset` to pass next to continue reading.
///
/// `extra_note`, when present (e.g. a non-HTML/plain content type), is
/// included in the header on every page.
fn page_content(content: &str, offset: usize, extra_note: Option<&str>) -> String {
    let total_len = content.len();
    let start = content.floor_char_boundary(offset.min(total_len));
    let end = content.floor_char_boundary((start + DEFAULT_PAGE_BYTES).min(total_len));
    let page = content.get(start..end).unwrap_or_default();

    let mut header = vec![format!("total {total_len} bytes")];
    if let Some(note) = extra_note {
        header.push(note.to_string());
    }
    if end < total_len {
        header.push(format!(
            "showing bytes {start}-{end}; call again with offset={end} to continue"
        ));
    } else if start > 0 {
        header.push(format!("showing bytes {start}-{end} (end of content)"));
    }

    if header.len() == 1 && start == 0 && end == total_len {
        // Whole content fit in one page with nothing extra to note — no
        // header needed, just the content.
        return page.to_string();
    }

    format!("{}\n\n{page}", header.join("\n"))
}

#[cfg(test)]
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
    fn page_short_content_is_unadorned() {
        let short = "hello world";
        assert_eq!(
            page_content(short, 0, None),
            "hello world",
            "content that fits in one page with no note needs no header"
        );
    }

    #[test]
    fn page_long_content_reports_total_and_continuation() {
        let long = "a".repeat(DEFAULT_PAGE_BYTES + 100);
        let result = page_content(&long, 0, None);
        assert!(
            result.contains(&format!("total {} bytes", long.len())),
            "should report the total size: {result}"
        );
        assert!(
            result.contains(&format!("offset={DEFAULT_PAGE_BYTES}")),
            "should report the offset to continue from: {result}"
        );
    }

    #[test]
    fn page_content_continuation_reaches_the_end() {
        let long = "a".repeat(DEFAULT_PAGE_BYTES + 100);
        let result = page_content(&long, DEFAULT_PAGE_BYTES, None);
        assert!(
            result.contains("end of content"),
            "the final page should say there's nothing more: {result}"
        );
        assert!(
            !result.contains("call again"),
            "the final page should not offer a further offset: {result}"
        );
    }

    #[test]
    fn page_content_includes_extra_note_on_every_page() {
        let short = "{}";
        let result = page_content(short, 0, Some("content-type: application/json"));
        assert!(
            result.contains("content-type: application/json"),
            "should surface the extra note even when everything fits in one page: {result}"
        );
    }

    #[tokio::test]
    async fn fetch_html_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "<html><head><title>Test</title></head>\
                         <body><article><p>Main content here.</p></article></body></html>",
                "text/html",
            ))
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
    async fn fetch_binary_content_returns_error() {
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

        assert!(result.is_error, "binary content should be an error");
        assert!(
            result.output.contains("image/png") && result.output.contains("binary"),
            "should name the content type and say it's binary: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn fetch_json_body_is_returned_with_content_type_note() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/data.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("{\"hello\":\"world\"}", "application/json"),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/data.json", server.uri())}))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "JSON body should be returned, not refused"
        );
        assert!(
            result.output.contains("application/json"),
            "should note the content type: {}",
            result.output
        );
        assert!(
            result.output.contains("{\"hello\":\"world\"}"),
            "should return the raw JSON body: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn missing_url_returns_error() {
        let tool = WebFetchTool::new();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing url should error");
    }
}

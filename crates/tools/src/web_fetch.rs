//! Web page content fetcher optimized for LLM consumption.

use async_trait::async_trait;
use encoding_rs::{Encoding, UTF_8};
use html_escape::decode_html_entities;
use serde_json::Value;
use tracing::{debug, warn};

use residuum_inference::ToolDefinition;

use residuum_agent::Tool;
use residuum_inference::{ToolError, ToolResult};

/// Default page size returned per call, to avoid context window blowout.
///
/// Not a hard limit on what can be fetched: `offset` pages through the rest
/// of a longer page across further calls.
const DEFAULT_PAGE_BYTES: usize = 50_000;

/// Response body bytes read before the download stops.
///
/// Bounds memory for a single fetch. A page past this size is cut short, the
/// header says so, and `offset` pages through what was downloaded.
const MAX_DOWNLOAD_BYTES: usize = 5 * 1024 * 1024;

/// Elements whose contents are code, styling or fallback markup rather than page text.
const NON_TEXT_ELEMENTS: [&str; 4] = ["script", "style", "noscript", "template"];

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
                          reports. Downloads stop at a size limit, which the header reports \
                          when a page is cut short."
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
                        "description": "Byte offset into the fetched content to start the page from (default: 0). To continue reading, pass the offset from a previous response's \"call again with offset=N to continue\" line."
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

        let mut response = self
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

        let (body, truncated) = read_capped_body(&mut response, &content_type).await?;

        let is_html = content_type.contains("html");
        let content = if is_html {
            match extract_content(&body, url) {
                Ok(text) => text,
                Err(msg) => {
                    warn!(url = %url, error = %msg, "content extraction failed, returning raw text");
                    html_to_text(&body)
                }
            }
        } else {
            body
        };

        // Note the content type on anything other than HTML/plain text, since
        // the caller sees only extracted/raw text with no header otherwise.
        let mut notes = Vec::new();
        if !(is_html || content_type.contains("text/plain") || content_type.is_empty()) {
            notes.push(format!("content-type: {content_type}"));
        }
        if truncated {
            notes.push(format!(
                "page is larger than the {MAX_DOWNLOAD_BYTES}-byte download limit; only the \
                 first {MAX_DOWNLOAD_BYTES} bytes were fetched"
            ));
        }

        Ok(ToolResult::success(page_content(&content, offset, &notes)))
    }
}

/// Read a response body up to `MAX_DOWNLOAD_BYTES`, decoded with the charset
/// its Content-Type declares. Returns the text and whether the body was cut short.
async fn read_capped_body(
    response: &mut reqwest::Response,
    content_type: &str,
) -> Result<(String, bool), ToolError> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ToolError::Execution(format!("failed to read response body: {e}")))?
    {
        let room = MAX_DOWNLOAD_BYTES - bytes.len();
        if chunk.len() > room {
            bytes.extend(chunk.iter().take(room));
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }

    let encoding = charset_of(content_type).unwrap_or(UTF_8);
    let (text, _, _) = encoding.decode(&bytes);
    Ok((text.into_owned(), truncated))
}

/// The encoding named by a `charset=` parameter of a Content-Type header.
fn charset_of(content_type: &str) -> Option<&'static Encoding> {
    content_type
        .split(';')
        .filter_map(|param| param.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
        .and_then(|(_, label)| Encoding::for_label(label.trim().trim_matches('"').as_bytes()))
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
    // article.content is cleaned HTML; reduce it to plain text
    output.push_str(&html_to_text(&article.content));
    Ok(output)
}

/// Convert HTML to plain text.
///
/// Drops tags, comments and the contents of `NON_TEXT_ELEMENTS`, collapses
/// whitespace runs, and decodes entities. Entities are decoded last, after the
/// markup is gone, so an escaped `&lt;b&gt;` stays text instead of becoming a tag.
fn html_to_text(html: &str) -> String {
    let mut raw = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(tag_start) = rest.find('<') {
        raw.push_str(rest.get(..tag_start).unwrap_or_default());
        let Some(after) = skip_markup(rest.get(tag_start..).unwrap_or_default()) else {
            // An unterminated tag has no end, so nothing after it is text.
            return collapse_and_decode(&raw);
        };
        rest = after;
    }
    raw.push_str(rest);
    collapse_and_decode(&raw)
}

/// Skip the markup at the start of `markup` (which begins with `<`) and return
/// the input that follows it. An element in `NON_TEXT_ELEMENTS` is skipped with
/// its contents. Returns `None` when the markup is never closed.
fn skip_markup(markup: &str) -> Option<&str> {
    if let Some(comment) = markup.strip_prefix("<!--") {
        // Commented-out markup can contain a `>` or an element opening tag,
        // so comments are skipped whole rather than by their first `>`.
        return comment.find("-->").and_then(|end| comment.get(end + 3..));
    }
    let tag_end = markup.find('>')?;
    let tag = markup.get(1..tag_end).unwrap_or_default();
    let after_tag = markup.get(tag_end + 1..).unwrap_or_default();
    match non_text_element_opened_by(tag) {
        Some(element) => Some(skip_element_contents(after_tag, element)),
        None => Some(after_tag),
    }
}

/// The `NON_TEXT_ELEMENTS` entry an opening tag (the text between `<` and `>`) starts.
fn non_text_element_opened_by(tag: &str) -> Option<&'static str> {
    let name = tag
        .split(|c: char| c.is_ascii_whitespace() || c == '/')
        .next()
        .unwrap_or_default();
    NON_TEXT_ELEMENTS
        .into_iter()
        .find(|element| element.eq_ignore_ascii_case(name))
}

/// Return the input after the closing tag of `element`. An element that is
/// never closed runs to the end of the input, as it does in a browser.
fn skip_element_contents<'a>(contents: &'a str, element: &str) -> &'a str {
    let Some(close) = find_ignore_ascii_case(contents, &format!("</{element}")) else {
        return "";
    };
    let closing_tag = contents.get(close..).unwrap_or_default();
    closing_tag
        .find('>')
        .and_then(|end| closing_tag.get(end + 1..))
        .unwrap_or_default()
}

/// Byte index of the first ASCII-case-insensitive match of `needle` in `haystack`.
///
/// Needles start with `<`, so a match is always on a character boundary.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Collapse whitespace runs to their first character, then decode entities in
/// the result and trim it.
fn collapse_and_decode(raw: &str) -> String {
    let mut collapsed = String::with_capacity(raw.len());
    let mut last_was_whitespace = false;
    for ch in raw.chars() {
        let is_ws = ch.is_whitespace();
        if is_ws && last_was_whitespace {
            continue;
        }
        last_was_whitespace = is_ws;
        collapsed.push(ch);
    }
    decode_html_entities(&collapsed).trim().to_string()
}

/// Page `content` starting at byte `offset`, returning at most
/// `DEFAULT_PAGE_BYTES` of it with a header reporting the total size and,
/// when more remains, the `offset` to pass next to continue reading.
///
/// `notes` (e.g. a non-HTML content type, or a download cut short) are
/// included in the header on every page.
fn page_content(content: &str, offset: usize, notes: &[String]) -> String {
    let total_len = content.len();
    let start = content.floor_char_boundary(offset.min(total_len));
    let end = content.floor_char_boundary((start + DEFAULT_PAGE_BYTES).min(total_len));
    let page = content.get(start..end).unwrap_or_default();

    let mut header = vec![format!("total {total_len} bytes")];
    header.extend(notes.iter().cloned());
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
    fn html_to_text_basic() {
        let html = "<p>Hello <b>world</b></p>";
        assert_eq!(html_to_text(html), "Hello world", "should strip tags");
    }

    #[test]
    fn html_to_text_collapses_whitespace() {
        let html = "<p>Hello   \n\n   world</p>";
        assert_eq!(
            html_to_text(html),
            "Hello world",
            "should collapse whitespace"
        );
    }

    #[test]
    fn html_to_text_drops_non_text_elements_with_their_contents() {
        let html = "<p>Before</p>\
                    <script type=\"text/javascript\">var secret = 1;</script>\
                    <STYLE>.a { color: red; }</STYLE>\
                    <noscript><img src=\"pixel.gif\">Enable JS</noscript>\
                    <template><div>cloned markup</div></template>\
                    <p>After</p>";
        assert_eq!(
            html_to_text(html),
            "BeforeAfter",
            "script, style, noscript and template contents are not page text"
        );
    }

    #[test]
    fn html_to_text_unclosed_non_text_element_runs_to_end() {
        assert_eq!(
            html_to_text("<p>kept</p><script>var x = '</p>"),
            "kept",
            "an unclosed script runs to the end of the input, as in a browser"
        );
    }

    #[test]
    fn html_to_text_skips_commented_out_element_openings() {
        let html = "<!-- <script> old widget > --><p>visible</p><!-- </script> -->";
        assert_eq!(
            html_to_text(html),
            "visible",
            "a comment is skipped whole, so an element opening inside it does not swallow the page"
        );
    }

    #[test]
    fn html_to_text_drops_unclosed_tag_and_everything_after_it() {
        assert_eq!(
            html_to_text("<p>kept</p><div class=\"broken"),
            "kept",
            "an unterminated tag has no end, so nothing after it is text"
        );
    }

    #[test]
    fn html_to_text_decodes_entities() {
        let html = "<p>Tom &amp; Jerry&#39;s &lt;b&gt; &#x41;&hellip;</p>";
        assert_eq!(
            html_to_text(html),
            "Tom & Jerry's <b> A…",
            "named and numeric entities should decode"
        );
    }

    #[test]
    fn html_to_text_decodes_entities_after_removing_markup() {
        assert_eq!(
            html_to_text("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "<script>alert(1)</script>",
            "an escaped tag is text, not markup, so it is not dropped as an element"
        );
    }

    #[test]
    fn page_short_content_is_unadorned() {
        let short = "hello world";
        assert_eq!(
            page_content(short, 0, &[]),
            "hello world",
            "content that fits in one page with no note needs no header"
        );
    }

    #[test]
    fn page_long_content_reports_total_and_continuation() {
        let long = "a".repeat(DEFAULT_PAGE_BYTES + 100);
        let result = page_content(&long, 0, &[]);
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
        let result = page_content(&long, DEFAULT_PAGE_BYTES, &[]);
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
    fn page_content_includes_notes_on_every_page() {
        let short = "{}";
        let notes = ["content-type: application/json".to_string()];
        let result = page_content(short, 0, &notes);
        assert!(
            result.contains("content-type: application/json"),
            "should surface the extra note even when everything fits in one page: {result}"
        );
    }

    #[test]
    fn offset_description_quotes_the_header_continuation_line() {
        let definition = WebFetchTool::new().definition();
        let offset_description = definition
            .parameters
            .pointer("/properties/offset/description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            offset_description.contains("call again with offset=N to continue"),
            "the offset description should quote the header line it refers to: {offset_description}"
        );
        assert!(
            !offset_description.contains("more content"),
            "no response emits a 'more content' note: {offset_description}"
        );
    }

    #[test]
    fn description_mentions_the_download_limit() {
        let definition = WebFetchTool::new().definition();
        assert!(
            definition.description.contains("size limit"),
            "the description should say that downloads stop at a size limit: {}",
            definition.description
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
    async fn fetch_html_drops_script_and_decodes_entities() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "<html><head><title>Test</title></head>\
                 <body><article><p>Tom &amp; Jerry</p>\
                 <script>var hidden = 1;</script></article></body></html>",
                "text/html",
            ))
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/entities", server.uri())}))
            .await
            .unwrap();

        assert!(
            result.output.contains("Tom & Jerry"),
            "entities should be decoded: {}",
            result.output
        );
        assert!(
            !result.output.contains("var hidden") && !result.output.contains("&amp;"),
            "script contents should not be returned and no entity should be left encoded: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn fetch_body_past_download_limit_is_cut_short_with_note() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/huge.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_bytes(vec![b'a'; MAX_DOWNLOAD_BYTES + 1]),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/huge.txt", server.uri())}))
            .await
            .unwrap();

        assert!(
            !result.is_error,
            "an oversized page should still be returned"
        );
        assert!(
            result
                .output
                .contains(&format!("total {MAX_DOWNLOAD_BYTES} bytes")),
            "should report the size that was fetched: {}",
            result.output
        );
        assert!(
            result.output.contains("download limit"),
            "should say the page was cut short at the download limit: {}",
            result.output
        );
        assert!(
            result
                .output
                .contains(&format!("offset={DEFAULT_PAGE_BYTES}")),
            "the cut-short page should still page through what was fetched: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn fetch_body_at_download_limit_is_not_cut_short() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/exact.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_bytes(vec![b'a'; MAX_DOWNLOAD_BYTES]),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/exact.txt", server.uri())}))
            .await
            .unwrap();

        assert!(
            !result.output.contains("download limit"),
            "a body of exactly the limit is complete and needs no note: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn fetch_decodes_body_in_declared_charset() {
        let server = MockServer::start().await;
        // "café" in ISO-8859-1: the é is the single byte 0xE9.
        Mock::given(method("GET"))
            .and(path("/latin1.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain; charset=ISO-8859-1")
                    .set_body_bytes(vec![0x63, 0x61, 0x66, 0xE9]),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::new();
        let result = tool
            .execute(serde_json::json!({"url": format!("{}/latin1.txt", server.uri())}))
            .await
            .unwrap();

        assert_eq!(
            result.output, "café",
            "the body should be decoded with the charset its Content-Type declares"
        );
    }

    #[tokio::test]
    async fn missing_url_returns_error() {
        let tool = WebFetchTool::new();
        let result = tool.execute(serde_json::json!({})).await;
        assert!(result.is_err(), "missing url should error");
    }
}

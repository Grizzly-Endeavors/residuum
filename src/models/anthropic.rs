//! Anthropic Messages API provider implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use super::http::{SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::retry::{RetryConfig, with_retry};
use super::{
    CompletionOptions, ImageData, Message, ModelError, ModelProvider, ModelResponse,
    ResponseFormat, Role, ThinkingConfig, ThinkingLevel, ToolCall, ToolDefinition, Usage,
};

/// Anthropic Messages API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta headers and identity required for OAuth token access to newer models.
/// OAuth tokens (sk-ant-oat01-*) are issued via the Claude Code OAuth flow and
/// require Claude Code identity markers to access models like claude-sonnet-4-6
/// and claude-opus-4-6.
pub(crate) const OAUTH_BETA: &str = "claude-code-20250219,oauth-2025-04-20";
pub(crate) const OAUTH_USER_AGENT: &str = "claude-cli/2.1.75";
const OAUTH_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

// ---------------------------------------------------------------------------
// Public client
// ---------------------------------------------------------------------------

/// Client for the Anthropic Messages API.
///
/// Sends chat completions to Anthropic's `/v1/messages` endpoint, handling
/// the Anthropic-specific message format (system as top-level field, content
/// blocks, tool use/result blocks).
pub(crate) struct AnthropicClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    retry: RetryConfig,
}

impl AnthropicClient {
    /// Create a new Anthropic client.
    ///
    /// # Arguments
    /// * `http` - Shared HTTP client for connection pooling
    /// * `base_url` - API base URL (e.g. `https://api.anthropic.com`)
    /// * `api_key` - Anthropic API key
    /// * `model` - Model identifier (e.g. `claude-sonnet-4-20250514`)
    /// * `max_tokens` - Default maximum tokens for completions
    #[must_use]
    pub fn new(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        max_tokens: u32,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);
        Self {
            http,
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            max_tokens,
            retry,
        }
    }

    /// Build the full endpoint URL.
    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    /// Convert our generic messages into Anthropic-specific format.
    ///
    /// System messages are extracted and returned separately as a concatenated
    /// string (Anthropic uses a top-level `system` field rather than putting
    /// system messages in the messages array).
    fn convert_messages(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut api_messages: Vec<AnthropicMessage> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_parts.push(&msg.content);
                }
                Role::User => {
                    let content = if msg.images.is_empty() {
                        AnthropicContent::Text(msg.content.clone())
                    } else {
                        let mut blocks = Vec::new();
                        if !msg.content.is_empty() {
                            blocks.push(AnthropicContentBlock::Text {
                                text: msg.content.clone(),
                            });
                        }
                        append_image_blocks(&mut blocks, &msg.images);
                        AnthropicContent::Blocks(blocks)
                    };
                    api_messages.push(AnthropicMessage {
                        role: String::from("user"),
                        content,
                    });
                }
                Role::Assistant => {
                    let mut blocks: Vec<AnthropicContentBlock> = Vec::new();

                    if !msg.content.is_empty() {
                        blocks.push(AnthropicContentBlock::Text {
                            text: msg.content.clone(),
                        });
                    }

                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            blocks.push(AnthropicContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input: tc.arguments.clone(),
                            });
                        }
                    }

                    if blocks.is_empty() {
                        // Empty assistant message -- send as plain text to avoid
                        // sending an empty blocks array which the API rejects
                        api_messages.push(AnthropicMessage {
                            role: String::from("assistant"),
                            content: AnthropicContent::Text(msg.content.clone()),
                        });
                    } else {
                        api_messages.push(AnthropicMessage {
                            role: String::from("assistant"),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    }
                }
                Role::Tool => {
                    let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                    let mut blocks = vec![AnthropicContentBlock::ToolResult {
                        tool_use_id,
                        content: msg.content.clone(),
                    }];
                    append_image_blocks(&mut blocks, &msg.images);
                    api_messages.push(AnthropicMessage {
                        role: String::from("user"),
                        content: AnthropicContent::Blocks(blocks),
                    });
                }
            }
        }

        let system = (!system_parts.is_empty()).then(|| system_parts.join("\n"));

        // Merge consecutive same-role messages (required by Anthropic API for tool results)
        let api_messages = merge_consecutive_messages(api_messages);

        (system, api_messages)
    }

    /// Convert tool definitions to Anthropic's format.
    ///
    /// Returns a heterogeneous vec of tool entries. If `web_search` is set,
    /// a server-side `web_search_20250305` entry is appended.
    fn convert_tools(
        tools: &[ToolDefinition],
        web_search: Option<&super::WebSearchNativeConfig>,
    ) -> Vec<AnthropicToolEntry> {
        let mut entries: Vec<AnthropicToolEntry> = tools
            .iter()
            .map(|t| {
                AnthropicToolEntry::Function(AnthropicTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
            })
            .collect();

        if let Some(ws) = web_search {
            entries.push(AnthropicToolEntry::WebSearch(AnthropicWebSearchTool {
                r#type: "web_search_20250305".to_string(),
                name: "web_search".to_string(),
                max_uses: ws.max_uses,
                allowed_domains: ws.allowed_domains.clone(),
                blocked_domains: ws.blocked_domains.clone(),
            }));
        }

        entries
    }

    /// Map thinking config to Anthropic's thinking budget format.
    fn build_thinking_config(
        thinking: &ThinkingConfig,
        max_tokens: u32,
    ) -> Option<AnthropicThinking> {
        let budget = match thinking {
            ThinkingConfig::Level(ThinkingLevel::Low) => max_tokens / 4,
            ThinkingConfig::Level(ThinkingLevel::Medium) | ThinkingConfig::Toggle(true) => {
                max_tokens / 2
            }
            ThinkingConfig::Level(ThinkingLevel::High) => max_tokens * 3 / 4,
            ThinkingConfig::Toggle(false) => return None,
        };
        // budget_tokens must be > 0 and < max_tokens
        let budget = budget.max(1).min(max_tokens - 1);
        Some(AnthropicThinking {
            r#type: "enabled",
            budget_tokens: budget,
        })
    }

    /// Send a pre-built request to the Anthropic API and parse the response.
    async fn send_completion(
        http: &SharedHttpClient,
        endpoint: &str,
        api_key: &str,
        request: &AnthropicRequest,
    ) -> Result<ModelResponse, ModelError> {
        debug!(
            model = %request.model,
            max_tokens = request.max_tokens,
            message_count = request.messages.len(),
            tool_count = request.tools.as_ref().map_or(0, Vec::len),
            "sending anthropic completion request"
        );

        let timeout_secs = http.timeout_secs();
        let mut req_builder = http
            .client()
            .post(endpoint)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        // OAuth tokens (sk-ant-oat01-*) use Bearer auth + Claude Code identity
        // headers; standard API keys use x-api-key.
        if is_oauth_key(api_key) {
            req_builder = req_builder
                .header("Authorization", format!("Bearer {api_key}"))
                .header("anthropic-beta", OAUTH_BETA)
                .header("user-agent", OAUTH_USER_AGENT)
                .header("x-app", "cli");
        } else {
            req_builder = req_builder.header("x-api-key", api_key);
        }

        let request_json = serde_json::to_string(request)
            .map_err(|e| ModelError::Parse(format!("failed to serialize request: {e}")))?;

        let response = req_builder
            .body(request_json.clone())
            .send()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;

        let status = response.status();
        if !status.is_success() {
            let body = match response.text().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read error response body");
                    format!("failed to read response body: {e}")
                }
            };

            tracing::warn!(
                status = %status,
                response_body = %body,
                request_body = %request_json,
                "anthropic API error — full request/response for diagnosis"
            );

            let error_msg = serde_json::from_str::<AnthropicErrorResponse>(&body).map_or_else(
                |_| format!("anthropic api error {status}: {body}"),
                |parsed| {
                    if parsed.error.r#type.is_empty() {
                        format!("anthropic api error {status}: {}", parsed.error.message)
                    } else {
                        format!(
                            "anthropic api error {status} ({}): {}",
                            parsed.error.r#type, parsed.error.message
                        )
                    }
                },
            );

            return Err(ModelError::Api(error_msg));
        }

        let body = response
            .text()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;

        let api_response: AnthropicResponse = serde_json::from_str(&body)
            .map_err(|e| ModelError::Parse(format!("failed to parse anthropic response: {e}")))?;

        let result = Self::parse_response(api_response);

        info!(
            model = %request.model,
            content_len = result.content.len(),
            tool_calls = result.tool_calls.len(),
            "anthropic completion received"
        );

        Ok(result)
    }

    /// Parse the API response into our generic `ModelResponse`.
    fn parse_response(response: AnthropicResponse) -> ModelResponse {
        let mut text_parts: Vec<String> = Vec::new();
        let mut thinking_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in response.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    text_parts.push(text);
                }
                AnthropicContentBlock::Thinking { thinking } => {
                    thinking_parts.push(thinking);
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                    });
                }
                AnthropicContentBlock::Image { .. } | AnthropicContentBlock::ToolResult { .. } => {
                    // request-only blocks — skip in response parsing
                }
                AnthropicContentBlock::ServerToolUse { id, name, .. } => {
                    debug!(id = %id, name = %name, "server tool use block in response");
                }
                AnthropicContentBlock::WebSearchToolResult { tool_use_id, .. } => {
                    debug!(tool_use_id = %tool_use_id, "web search tool result in response");
                }
            }
        }

        let content = text_parts.join("");
        let thinking_text = (!thinking_parts.is_empty()).then(|| thinking_parts.join(""));

        let usage = response.usage.map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_tokens: u.cache_creation_input_tokens,
            cache_read_tokens: u.cache_read_input_tokens,
        });

        let mut resp = ModelResponse::new(content, tool_calls);
        resp.usage = usage;
        resp.thinking = thinking_text;
        resp
    }
}

#[async_trait]
impl ModelProvider for AnthropicClient {
    /// Send a completion request to the Anthropic Messages API.
    ///
    /// # Errors
    /// Returns `ModelError::Timeout` if the request exceeds the configured timeout,
    /// `ModelError::Api` if the API returns an error status, `ModelError::Parse` if
    /// the response body is malformed, or `ModelError::Request` for network failures.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        let (system, api_messages) = Self::convert_messages(messages);
        let max_tokens = options.max_tokens.unwrap_or(self.max_tokens);
        let has_web_search = options.web_search.is_some();
        let api_tools = (!tools.is_empty() || has_web_search)
            .then(|| Self::convert_tools(tools, options.web_search.as_ref()));

        // OAuth tokens require the Claude Code identity as an isolated first block
        // in the system prompt to access newer models (sonnet 4.6, opus 4.6).
        let system: Option<AnthropicSystem> = if is_oauth_key(&self.api_key) {
            let mut blocks = vec![AnthropicSystemBlock {
                r#type: "text",
                text: OAUTH_IDENTITY.to_string(),
            }];
            if let Some(s) = system {
                blocks.push(AnthropicSystemBlock {
                    r#type: "text",
                    text: s,
                });
            }
            Some(AnthropicSystem::Blocks(blocks))
        } else {
            system.map(AnthropicSystem::Text)
        };
        let model = self.model.clone();
        let endpoint = self.endpoint();
        let api_key = self.api_key.clone();
        let http = self.http.clone();

        let output_config = match &options.response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonSchema { schema, .. } => Some(AnthropicOutputConfig {
                format: AnthropicOutputFormat {
                    r#type: "json_schema".to_string(),
                    schema: schema.clone(),
                },
            }),
        };
        let temperature = options.temperature;

        let thinking = options
            .thinking
            .as_ref()
            .and_then(|tc| Self::build_thinking_config(tc, max_tokens));

        with_retry(&self.retry, || {
            let system = system.clone();
            let api_messages = api_messages.clone();
            let api_tools = api_tools.clone();
            let output_config = output_config.clone();
            let thinking = thinking.clone();
            let model = model.clone();
            let endpoint = endpoint.clone();
            let api_key = api_key.clone();
            let http = http.clone();

            async move {
                let request = AnthropicRequest {
                    model: model.clone(),
                    max_tokens,
                    system,
                    messages: api_messages,
                    tools: api_tools,
                    output_config,
                    temperature,
                    thinking,
                    cache_control: AnthropicCacheControl {
                        r#type: "ephemeral",
                    },
                };

                Self::send_completion(&http, &endpoint, &api_key, &request).await
            }
        })
        .await
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

pub(crate) fn is_oauth_key(key: &str) -> bool {
    key.starts_with("sk-ant-oat01-")
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

/// Append `Image` content blocks for each `ImageData` entry.
fn append_image_blocks(blocks: &mut Vec<AnthropicContentBlock>, images: &[ImageData]) {
    for img in images {
        blocks.push(AnthropicContentBlock::Image {
            source: AnthropicImageSource {
                r#type: String::from("base64"),
                media_type: img.media_type.clone(),
                data: img.data.clone(),
            },
        });
    }
}

// ---------------------------------------------------------------------------
// Message merging
// ---------------------------------------------------------------------------

/// Merge consecutive messages that share the same role.
///
/// Anthropic's API requires that all content blocks for a given role appear in
/// a single message when consecutive (e.g. multiple tool results must be in one
/// user message). This collapses runs of same-role messages by combining their
/// content blocks.
fn merge_consecutive_messages(messages: Vec<AnthropicMessage>) -> Vec<AnthropicMessage> {
    let mut merged: Vec<AnthropicMessage> = Vec::with_capacity(messages.len());

    for msg in messages {
        if let Some(last) = merged.last_mut()
            && last.role == msg.role
        {
            let existing =
                std::mem::replace(&mut last.content, AnthropicContent::Blocks(Vec::new()));
            let mut blocks = existing.into_blocks();
            blocks.extend(msg.content.into_blocks());
            last.content = AnthropicContent::Blocks(blocks);
            continue;
        }
        merged.push(msg);
    }

    merged
}

// ---------------------------------------------------------------------------
// Anthropic API serde types (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct AnthropicCacheControl {
    r#type: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct AnthropicThinking {
    r#type: &'static str,
    budget_tokens: u32,
}

/// System prompt content — plain string or array of text blocks.
///
/// OAuth tokens require the Claude Code identity as an isolated first block,
/// so the system prompt must be sent as an array for OAuth requests.
#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum AnthropicSystem {
    Text(String),
    Blocks(Vec<AnthropicSystemBlock>),
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicSystemBlock {
    r#type: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<AnthropicSystem>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    cache_control: AnthropicCacheControl,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Content can be a simple string or an array of content blocks.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

impl AnthropicContent {
    /// Convert into a vec of content blocks regardless of variant.
    fn into_blocks(self) -> Vec<AnthropicContentBlock> {
        match self {
            Self::Text(s) => vec![AnthropicContentBlock::Text { text: s }],
            Self::Blocks(b) => b,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    /// Server-side tool invocation (e.g. web search). Informational only —
    /// results are already incorporated into the model's text response.
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// Result of a server-side tool (e.g. web search results).
    WebSearchToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AnthropicImageSource {
    r#type: String,
    media_type: String,
    data: String,
}

/// Heterogeneous tool entry for the Anthropic `tools` array.
///
/// Anthropic's API supports both function tools and server-side tools
/// (like `web_search_20250305`) in the same array.
#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum AnthropicToolEntry {
    /// Standard function tool (name, description, `input_schema`).
    Function(AnthropicTool),
    /// Server-side web search tool.
    WebSearch(AnthropicWebSearchTool),
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicWebSearchTool {
    r#type: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_uses: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_domains: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicOutputConfig {
    format: AnthropicOutputFormat,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicOutputFormat {
    r#type: String,
    schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
#[expect(clippy::struct_field_names, reason = "field names match Anthropic API")]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    /// Error classification (e.g. `invalid_request_error`, `authentication_error`).
    #[serde(default)]
    r#type: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::get_unwrap,
    reason = "test code uses get().unwrap() for clarity"
)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::models::http::HttpClientConfig;
    use crate::models::retry::RetryConfig;

    /// Create a test client pointing at the given mock server URL.
    fn test_client(base_url: &str) -> AnthropicClient {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        AnthropicClient::new(
            http,
            base_url,
            "test-api-key",
            "claude-sonnet-4-20250514",
            1024,
            RetryConfig::no_retry(),
        )
    }

    fn simple_user_message() -> Vec<Message> {
        vec![Message::user("Hello")]
    }

    fn success_response_body() -> Value {
        json!({
            "content": [
                {"type": "text", "text": "Hello! How can I help?"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 15
            }
        })
    }

    #[tokio::test]
    async fn basic_chat_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let messages = simple_user_message();
        let options = CompletionOptions::default();

        let result = client.complete(&messages, &[], &options).await;
        assert!(result.is_ok(), "basic chat should succeed");

        let resp = result.unwrap();
        assert_eq!(
            resp.content, "Hello! How can I help?",
            "should extract text from content blocks"
        );
        assert!(resp.tool_calls.is_empty(), "should have no tool calls");

        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 10, "input tokens should match");
        assert_eq!(usage.output_tokens, 15, "output tokens should match");
    }

    #[tokio::test]
    async fn system_message_handling() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello"),
        ];
        let options = CompletionOptions::default();

        let result = client.complete(&messages, &[], &options).await;
        assert!(result.is_ok(), "system message request should succeed");

        // Verify system was extracted properly by checking the conversion
        let (system, api_msgs) = AnthropicClient::convert_messages(&messages);
        assert_eq!(
            system.as_deref(),
            Some("You are a helpful assistant."),
            "system should be extracted to top-level field"
        );
        assert_eq!(
            api_msgs.len(),
            1,
            "system message should not appear in messages array"
        );
    }

    #[tokio::test]
    async fn tool_use_response_parsing() {
        let server = MockServer::start().await;
        let tool_response = json!({
            "content": [
                {"type": "text", "text": "I'll search for that."},
                {
                    "type": "tool_use",
                    "id": "toolu_abc123",
                    "name": "web_search",
                    "input": {"query": "rust programming"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 20,
                "output_tokens": 30
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(tool_response))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let messages = simple_user_message();
        let tools = vec![ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let options = CompletionOptions::default();

        let result = client.complete(&messages, &tools, &options).await;
        assert!(result.is_ok(), "tool use response should parse");

        let resp = result.unwrap();
        assert_eq!(
            resp.content, "I'll search for that.",
            "text content should be extracted"
        );
        assert_eq!(resp.tool_calls.len(), 1, "should have one tool call");

        let tc = resp.tool_calls.first().unwrap();
        assert_eq!(tc.id, "toolu_abc123", "tool call id should match");
        assert_eq!(tc.name, "web_search", "tool call name should match");
        assert_eq!(
            tc.arguments,
            json!({"query": "rust programming"}),
            "tool call arguments should be native JSON"
        );
    }

    #[tokio::test]
    async fn tool_result_serialization() {
        let messages = vec![
            Message::user("Search for rust"),
            Message::assistant(
                "I'll search for that.",
                Some(vec![ToolCall {
                    id: "toolu_abc123".to_string(),
                    name: "web_search".to_string(),
                    arguments: json!({"query": "rust"}),
                }]),
            ),
            Message::tool("Rust is a systems programming language.", "toolu_abc123"),
        ];

        let (system, api_msgs) = AnthropicClient::convert_messages(&messages);
        assert!(system.is_none(), "no system message expected");
        assert_eq!(api_msgs.len(), 3, "should have 3 API messages");

        // Check the tool result message
        let tool_result_msg = api_msgs.get(2).unwrap();
        assert_eq!(
            tool_result_msg.role, "user",
            "tool result should be sent as user role"
        );

        // Verify serialization produces correct structure
        let serialized = serde_json::to_value(tool_result_msg).unwrap();
        let content = serialized.get("content").unwrap();
        assert!(
            content.is_array(),
            "tool result content should be blocks array"
        );

        let blocks = content.as_array().unwrap();
        assert_eq!(blocks.len(), 1, "should have one tool_result block");

        let block = blocks.first().unwrap();
        assert_eq!(
            block.get("type").unwrap().as_str().unwrap(),
            "tool_result",
            "block type should be tool_result"
        );
        assert_eq!(
            block.get("tool_use_id").unwrap().as_str().unwrap(),
            "toolu_abc123",
            "tool_use_id should match"
        );
        assert_eq!(
            block.get("content").unwrap().as_str().unwrap(),
            "Rust is a systems programming language.",
            "tool result content should match"
        );
    }

    #[tokio::test]
    async fn x_api_key_header_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(
            result.is_ok(),
            "request should succeed when x-api-key header is matched"
        );
    }

    #[tokio::test]
    async fn anthropic_version_header_present() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(
            result.is_ok(),
            "request should succeed when anthropic-version header is matched"
        );
    }

    #[tokio::test]
    async fn error_401_handling() {
        let server = MockServer::start().await;
        let error_body = json!({
            "error": {
                "type": "authentication_error",
                "message": "invalid x-api-key"
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_json(error_body))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "401 should return error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("401"),
            "error should contain status code, got: {msg}"
        );
        assert!(
            msg.contains("invalid x-api-key"),
            "error should contain API message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn error_429_handling() {
        let server = MockServer::start().await;
        let error_body = json!({
            "error": {
                "type": "rate_limit_error",
                "message": "rate limit exceeded, please retry after 30s"
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(429).set_body_json(error_body))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "429 should return error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("429"),
            "error should contain status code, got: {msg}"
        );
        assert!(
            msg.contains("rate limit"),
            "error should contain rate limit message, got: {msg}"
        );
        assert!(
            err.is_retryable(),
            "429 rate limit error should be retryable"
        );
    }

    #[tokio::test]
    async fn timeout_handling() {
        let server = MockServer::start().await;
        // Respond with a delay longer than the client timeout (5s)
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(success_response_body())
                    .set_delay(std::time::Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "request should time out");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Timeout(_)),
            "error should be Timeout variant, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn complete_with_json_schema_response_format() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "output_config": {
                    "format": {
                        "type": "json_schema",
                        "schema": {
                            "type": "object",
                            "properties": {
                                "answer": {"type": "string"}
                            }
                        }
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    {"type": "text", "text": "{\"answer\": \"hello\"}"}
                ],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 15}
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            response_format: crate::models::ResponseFormat::JsonSchema {
                name: "test_schema".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"}
                    }
                }),
            },
            ..CompletionOptions::default()
        };

        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(result.is_ok(), "structured output request should succeed");
        assert_eq!(
            result.unwrap().content,
            "{\"answer\": \"hello\"}",
            "should return JSON content"
        );
    }

    #[tokio::test]
    async fn temperature_included_when_set() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "temperature": 0.7
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            temperature: Some(0.7),
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(result.is_ok(), "request with temperature should succeed");
    }

    #[tokio::test]
    async fn cache_tokens_parsed_from_response() {
        let server = MockServer::start().await;
        let body = json!({
            "content": [
                {"type": "text", "text": "cached response"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 15,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 50
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "cache token response should succeed");

        let usage = result.unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 10, "input tokens should match");
        assert_eq!(usage.output_tokens, 15, "output tokens should match");
        assert_eq!(
            usage.cache_creation_tokens,
            Some(100),
            "cache creation tokens should match"
        );
        assert_eq!(
            usage.cache_read_tokens,
            Some(50),
            "cache read tokens should match"
        );
    }

    #[tokio::test]
    async fn temperature_absent_when_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions::default();
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(result.is_ok(), "request without temperature should succeed");

        // Verify by checking the request body does not contain "temperature"
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("temperature").is_none(),
            "temperature should be absent from request body when None"
        );
    }

    #[tokio::test]
    async fn thinking_level_sends_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "thinking": {
                    "type": "enabled"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            max_tokens: Some(1024),
            thinking: Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(
            result.is_ok(),
            "request with thinking should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn thinking_response_extracted() {
        let server = MockServer::start().await;
        let body = json!({
            "content": [
                {"type": "thinking", "thinking": "Let me reason about this..."},
                {"type": "text", "text": "Here is my answer."}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "thinking response should succeed");

        let resp = result.unwrap();
        assert_eq!(
            resp.content, "Here is my answer.",
            "content should only contain text blocks"
        );
        assert_eq!(
            resp.thinking.as_deref(),
            Some("Let me reason about this..."),
            "thinking should be extracted separately"
        );
    }

    #[tokio::test]
    async fn web_search_tool_injected_when_enabled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            web_search: Some(crate::models::WebSearchNativeConfig {
                max_uses: Some(3),
                ..Default::default()
            }),
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(result.is_ok(), "request with web search should succeed");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        let tools = body.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools.len(), 1, "should have one tool (web_search)");
        let ws_tool = tools.first().unwrap();
        assert_eq!(
            ws_tool.get("type").unwrap().as_str().unwrap(),
            "web_search_20250305",
            "tool type should be web_search_20250305"
        );
        assert_eq!(
            ws_tool.get("max_uses").unwrap().as_u64().unwrap(),
            3,
            "max_uses should be set"
        );
    }

    #[tokio::test]
    async fn web_search_absent_when_not_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "request without web search should succeed");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("tools").is_none(),
            "tools should be absent when no tools or web search configured"
        );
    }

    #[tokio::test]
    async fn server_tool_use_blocks_dont_create_tool_calls() {
        let server = MockServer::start().await;
        let response_with_server_tool = json!({
            "content": [
                {
                    "type": "server_tool_use",
                    "id": "srvtoolu_123",
                    "name": "web_search",
                    "input": {"query": "rust programming"}
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_123",
                    "content": [{"type": "web_search_result", "url": "https://example.com"}]
                },
                {
                    "type": "text",
                    "text": "Based on my search, Rust is a systems programming language."
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 20,
                "output_tokens": 30
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_with_server_tool))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            web_search: Some(crate::models::WebSearchNativeConfig::default()),
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(
            result.is_ok(),
            "response with server tool blocks should parse"
        );

        let resp = result.unwrap();
        assert!(
            resp.tool_calls.is_empty(),
            "server_tool_use should not create ToolCall entries"
        );
        assert!(
            resp.content
                .contains("Rust is a systems programming language"),
            "text content should be preserved"
        );
    }

    #[test]
    fn convert_messages_merges_consecutive_tool_results() {
        let messages = vec![
            Message::user("Use both tools"),
            Message::assistant(
                "",
                Some(vec![
                    ToolCall {
                        id: "tool_1".to_string(),
                        name: "search".to_string(),
                        arguments: json!({"q": "a"}),
                    },
                    ToolCall {
                        id: "tool_2".to_string(),
                        name: "search".to_string(),
                        arguments: json!({"q": "b"}),
                    },
                ]),
            ),
            Message::tool("Result A", "tool_1"),
            Message::tool("Result B", "tool_2"),
        ];

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages);

        // user, assistant, merged-user (two tool results)
        assert_eq!(
            api_msgs.len(),
            3,
            "two tool results should merge into one user message"
        );

        let tool_msg = api_msgs.get(2).unwrap();
        assert_eq!(tool_msg.role, "user");

        let serialized = serde_json::to_value(tool_msg).unwrap();
        let blocks = serialized.get("content").unwrap().as_array().unwrap();
        assert_eq!(blocks.len(), 2, "merged message should have two blocks");
        assert_eq!(
            blocks
                .first()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool_result"
        );
        assert_eq!(
            blocks
                .get(1)
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool_result"
        );
        assert_eq!(
            blocks
                .first()
                .unwrap()
                .get("tool_use_id")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool_1"
        );
        assert_eq!(
            blocks
                .get(1)
                .unwrap()
                .get("tool_use_id")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool_2"
        );
    }

    #[test]
    fn convert_messages_merges_user_after_tool_result() {
        let messages = vec![
            Message::assistant(
                "calling tool",
                Some(vec![ToolCall {
                    id: "tool_1".to_string(),
                    name: "search".to_string(),
                    arguments: json!({}),
                }]),
            ),
            Message::tool("Tool output", "tool_1"),
            Message::user("Thanks, now do something else"),
        ];

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages);

        // assistant, merged-user (tool_result + user text)
        assert_eq!(
            api_msgs.len(),
            2,
            "tool result and following user message should merge"
        );

        let merged = api_msgs.get(1).unwrap();
        assert_eq!(merged.role, "user");

        let serialized = serde_json::to_value(merged).unwrap();
        let blocks = serialized.get("content").unwrap().as_array().unwrap();
        assert_eq!(blocks.len(), 2, "merged message should have two blocks");
        assert_eq!(
            blocks
                .first()
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap(),
            "tool_result"
        );
        assert_eq!(
            blocks
                .get(1)
                .unwrap()
                .get("type")
                .unwrap()
                .as_str()
                .unwrap(),
            "text"
        );
    }

    #[test]
    fn convert_messages_no_merge_across_roles() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there", None),
            Message::user("Follow up"),
        ];

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages);

        assert_eq!(api_msgs.len(), 3, "alternating roles should not be merged");
        assert_eq!(api_msgs.first().unwrap().role, "user");
        assert_eq!(api_msgs.get(1).unwrap().role, "assistant");
        assert_eq!(api_msgs.get(2).unwrap().role, "user");
    }

    #[tokio::test]
    async fn cache_control_included_in_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "cache_control": {
                    "type": "ephemeral"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(
            result.is_ok(),
            "request with cache_control should succeed: {result:?}"
        );
    }
}

//! Anthropic Messages API provider implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info};

use super::http::{SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, Role, ToolCall,
    ToolDefinition, Usage,
};

/// Anthropic Messages API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Public client
// ---------------------------------------------------------------------------

/// Client for the Anthropic Messages API.
///
/// Sends chat completions to Anthropic's `/v1/messages` endpoint, handling
/// the Anthropic-specific message format (system as top-level field, content
/// blocks, tool use/result blocks).
pub struct AnthropicClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
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
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);
        Self {
            http,
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            max_tokens,
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
                    api_messages.push(AnthropicMessage {
                        role: String::from("user"),
                        content: AnthropicContent::Text(msg.content.clone()),
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
                    let block = AnthropicContentBlock::ToolResult {
                        tool_use_id,
                        content: msg.content.clone(),
                    };
                    api_messages.push(AnthropicMessage {
                        role: String::from("user"),
                        content: AnthropicContent::Blocks(vec![block]),
                    });
                }
            }
        }

        let system = (!system_parts.is_empty()).then(|| system_parts.join("\n"));

        (system, api_messages)
    }

    /// Convert tool definitions to Anthropic's format.
    fn convert_tools(tools: &[ToolDefinition]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
            .collect()
    }

    /// Parse the API response into our generic `ModelResponse`.
    fn parse_response(response: AnthropicResponse) -> ModelResponse {
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in response.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    text_parts.push(text);
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                    });
                }
                AnthropicContentBlock::ToolResult { .. } => {
                    // tool_result blocks only appear in requests, not responses
                }
            }
        }

        let content = text_parts.join("");

        let usage = response.usage.map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        });

        let mut resp = ModelResponse::new(content, tool_calls);
        resp.usage = usage;
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

        let api_tools = (!tools.is_empty()).then(|| Self::convert_tools(tools));

        let request = AnthropicRequest {
            model: &self.model,
            max_tokens,
            system: system.as_deref(),
            messages: api_messages,
            tools: api_tools,
        };

        debug!(
            model = %self.model,
            max_tokens = max_tokens,
            message_count = messages.len(),
            tool_count = tools.len(),
            "sending anthropic completion request"
        );

        let timeout_secs = self.http.timeout_secs();
        let mut req_builder = self
            .http
            .client()
            .post(self.endpoint())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        // OAuth tokens (sk-ant-oat01-*) use Bearer auth + beta header;
        // standard API keys use x-api-key
        if self.api_key.starts_with("sk-ant-oat01-") {
            req_builder = req_builder
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("anthropic-beta", "oauth-2025-04-20");
        } else {
            req_builder = req_builder.header("x-api-key", &self.api_key);
        }

        let response = req_builder
            .json(&request)
            .send()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();

            let error_msg = serde_json::from_str::<AnthropicErrorResponse>(&body).map_or_else(
                |_| format!("anthropic api error {status}: {body}"),
                |parsed| format!("anthropic api error {status}: {}", parsed.error.message),
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
            model = %self.model,
            content_len = result.content.len(),
            tool_calls = result.tool_calls.len(),
            "anthropic completion received"
        );

        Ok(result)
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// ---------------------------------------------------------------------------
// Anthropic API serde types (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Content can be a simple string or an array of content blocks.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
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
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    #[expect(dead_code, reason = "captured from API for diagnostics/future use")]
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
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

    /// Create a test client pointing at the given mock server URL.
    fn test_client(base_url: &str) -> AnthropicClient {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        AnthropicClient::new(
            http,
            base_url,
            "test-api-key",
            "claude-sonnet-4-20250514",
            1024,
        )
    }

    fn simple_user_message() -> Vec<Message> {
        vec![Message {
            role: Role::User,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }]
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
            Message {
                role: Role::System,
                content: "You are a helpful assistant.".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::User,
                content: "Hello".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
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
            Message {
                role: Role::User,
                content: "Search for rust".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::Assistant,
                content: "I'll search for that.".to_string(),
                tool_calls: Some(vec![ToolCall {
                    id: "toolu_abc123".to_string(),
                    name: "web_search".to_string(),
                    arguments: json!({"query": "rust"}),
                }]),
                tool_call_id: None,
            },
            Message {
                role: Role::Tool,
                content: "Rust is a systems programming language.".to_string(),
                tool_calls: None,
                tool_call_id: Some("toolu_abc123".to_string()),
            },
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
}

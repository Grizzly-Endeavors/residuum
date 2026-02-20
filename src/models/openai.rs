//! Client for OpenAI-compatible chat completion APIs.
//!
//! Supports various providers including Azure, vLLM, LM Studio, and other
//! compatible endpoints.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::http::{HttpClientConfig, SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ToolCall, ToolDefinition,
};

/// OpenAI-compatible API client.
#[derive(Clone)]
pub struct OpenAiClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiClient {
    /// Create a new client without authentication (for local servers).
    ///
    /// This creates a new HTTP client internally. For connection reuse across
    /// multiple clients, use [`with_http_client`](Self::with_http_client) instead.
    ///
    /// # Errors
    /// Returns `ModelError::Request` if the HTTP client cannot be built.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<Self, ModelError> {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(timeout_secs))?;

        Ok(Self {
            http,
            base_url,
            api_key: None,
            model: model.into(),
        })
    }

    /// Create a new client with API key authentication.
    ///
    /// This creates a new HTTP client internally. For connection reuse across
    /// multiple clients, use [`with_http_client_and_api_key`](Self::with_http_client_and_api_key)
    /// instead.
    ///
    /// # Errors
    /// Returns `ModelError::Request` if the HTTP client cannot be built.
    pub fn with_api_key(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
    ) -> Result<Self, ModelError> {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(timeout_secs))?;

        Ok(Self {
            http,
            base_url,
            api_key: Some(api_key.into()),
            model: model.into(),
        })
    }

    /// Create a new client with a shared HTTP client (no authentication).
    ///
    /// Use this constructor to share connection pools across multiple model providers.
    #[must_use]
    pub fn with_http_client(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: None,
            model: model.into(),
        }
    }

    /// Create a new client with a shared HTTP client and API key authentication.
    ///
    /// Use this constructor to share connection pools across multiple model providers.
    #[must_use]
    pub fn with_http_client_and_api_key(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: Some(api_key.into()),
            model: model.into(),
        }
    }

    /// Get the configured timeout in seconds.
    fn timeout_secs(&self) -> u64 {
        self.http.timeout_secs()
    }
}

#[async_trait]
impl ModelProvider for OpenAiClient {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.base_url);

        let openai_messages: Vec<OpenAiMessage> = messages.iter().map(Into::into).collect();

        let openai_tools: Vec<OpenAiTool> = tools
            .iter()
            .map(|t| OpenAiTool {
                r#type: "function".to_string(),
                function: OpenAiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        let request = ChatCompletionRequest {
            model: &self.model,
            messages: openai_messages,
            tools: (!openai_tools.is_empty()).then_some(openai_tools),
            tool_choice: (!tools.is_empty()).then_some("auto"),
        };

        let mut req_builder = self.http.client().post(&url).json(&request);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| map_request_error(e, self.timeout_secs()))?;

        if !response.status().is_success() {
            let status = response.status();
            let raw_body = match response.text().await {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read error response body");
                    format!("failed to read response body: {e}")
                }
            };
            let error_body = serde_json::from_str::<OpenAiErrorResponse>(&raw_body)
                .map_or_else(|_| raw_body, |e| e.error.message);
            return Err(ModelError::Api(format!("{status}: {error_body}")));
        }

        let chat_response: ChatCompletionResponse = response.json().await?;

        let choice = chat_response.choices.into_iter().next().ok_or_else(|| {
            ModelError::Parse("OpenAI API response contained no choices in response".to_string())
        })?;

        // OpenAI uses null for content when tool_calls are present
        let content = choice.message.content.unwrap_or_default();

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                // OpenAI returns arguments as a JSON string, need to parse it
                let arguments: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .map_err(|e| {
                        ModelError::Parse(format!(
                            "failed to parse tool arguments for '{}': {e} (raw: {})",
                            tc.function.name, tc.function.arguments
                        ))
                    })?;
                Ok(ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, ModelError>>()?;

        Ok(ModelResponse::new(content, tool_calls))
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// --- OpenAI API request/response types ---

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl From<&Message> for OpenAiMessage {
    fn from(msg: &Message) -> Self {
        Self {
            role: msg.role.as_str().to_string(),
            content: (!msg.content.is_empty()).then(|| msg.content.clone()),
            tool_calls: msg.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|tc| OpenAiToolCall {
                        id: tc.id.clone(),
                        r#type: "function".to_string(),
                        function: OpenAiFunctionCall {
                            name: tc.name.clone(),
                            // OpenAI expects arguments as a JSON string
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect()
            }),
            tool_call_id: msg.tool_call_id.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Serialize, Deserialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    r#type: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String, // OpenAI returns arguments as JSON string
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    message: OpenAiResponseMessage,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

#[derive(Deserialize)]
struct OpenAiError {
    message: String,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::{CompletionOptions, Role};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn message_conversion_user() {
        let msg = Message {
            role: Role::User,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };

        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "user", "role should map to user");
        assert_eq!(
            openai_msg.content,
            Some("Hello".to_string()),
            "content should be preserved"
        );
        assert!(
            openai_msg.tool_calls.is_none(),
            "tool_calls should be absent"
        );
    }

    #[test]
    fn message_conversion_assistant_with_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(vec![ToolCall {
                id: "call_123".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }]),
            tool_call_id: None,
        };

        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "assistant", "role should map to assistant");
        assert!(
            openai_msg.content.is_none(),
            "empty content should become None"
        );
        let tool_calls = openai_msg.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1, "should have one tool call");
        assert_eq!(
            tool_calls.first().map(|t| &t.id),
            Some(&"call_123".to_string()),
            "tool call id should match"
        );
        assert_eq!(
            tool_calls.first().map(|t| &t.function.name),
            Some(&"bash".to_string()),
            "tool call name should match"
        );
        // Arguments should be JSON string
        assert_eq!(
            tool_calls.first().map(|t| &t.function.arguments),
            Some(&r#"{"command":"ls"}"#.to_string()),
            "arguments should be stringified JSON"
        );
    }

    #[test]
    fn message_conversion_tool() {
        let msg = Message {
            role: Role::Tool,
            content: "result output".to_string(),
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
        };

        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "tool", "role should map to tool");
        assert_eq!(
            openai_msg.content,
            Some("result output".to_string()),
            "content should be preserved"
        );
        assert_eq!(
            openai_msg.tool_call_id,
            Some("call_123".to_string()),
            "tool_call_id should be preserved"
        );
    }

    #[tokio::test]
    async fn complete_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Hello! How can I help you today?"
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let messages = vec![Message {
            role: Role::User,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.content, "Hello! How can I help you today?",
            "response content should match"
        );
        assert!(response.tool_calls.is_empty(), "should have no tool calls");
        assert!(response.is_complete(), "text-only response is complete");
    }

    #[tokio::test]
    async fn complete_with_api_key() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer sk-test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Authenticated response"
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client =
            OpenAiClient::with_api_key(mock_server.uri(), "gpt-4", "sk-test-key", 60).unwrap();
        let messages = vec![Message {
            role: Role::User,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.content, "Authenticated response",
            "authenticated response should match"
        );
    }

    #[tokio::test]
    async fn complete_with_tool_calls() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "bash",
                                "arguments": "{\"command\": \"ls -la\"}"
                            }
                        }]
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let messages = vec![Message {
            role: Role::User,
            content: "List files".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        let response = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert!(
            response.content.is_empty(),
            "null content should become empty string"
        );
        assert_eq!(response.tool_calls.len(), 1, "should have one tool call");
        assert_eq!(
            response.tool_calls.first().map(|t| &t.id),
            Some(&"call_abc123".to_string()),
            "tool call id should match"
        );
        assert_eq!(
            response.tool_calls.first().map(|t| &t.name),
            Some(&"bash".to_string()),
            "tool call name should match"
        );
        assert_eq!(
            response.tool_calls.first().map(|t| &t.arguments),
            Some(&serde_json::json!({"command": "ls -la"})),
            "tool call arguments should be parsed JSON"
        );
        assert!(
            !response.is_complete(),
            "response with tool calls is not complete"
        );
    }

    #[tokio::test]
    async fn api_error_401() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": {
                    "message": "Invalid API key",
                    "type": "invalid_request_error"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "401 should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Api(_)),
            "should be an Api error variant"
        );
        assert!(
            err.to_string().contains("401"),
            "error should contain status code"
        );
        assert!(
            err.to_string().contains("Invalid API key"),
            "error should contain message"
        );
    }

    #[tokio::test]
    async fn api_error_429() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": {
                    "message": "Rate limit exceeded",
                    "type": "rate_limit_error"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "429 should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Api(_)),
            "should be an Api error variant"
        );
        assert!(
            err.to_string().contains("429"),
            "error should contain status code"
        );
    }

    #[tokio::test]
    async fn api_error_500() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": {
                    "message": "Internal server error",
                    "type": "server_error"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "500 should return error");
        assert!(
            matches!(result.unwrap_err(), ModelError::Api(_)),
            "should be an Api error variant"
        );
    }

    #[tokio::test]
    async fn empty_choices() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices": []})),
            )
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "empty choices should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Parse(_)),
            "should be a Parse error variant"
        );
        assert!(
            err.to_string().contains("no choices"),
            "error should mention empty choices"
        );
    }

    #[tokio::test]
    async fn malformed_tool_arguments() {
        let mock_server = MockServer::start().await;

        // Return malformed JSON in arguments -- should return a parse error
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_123",
                            "type": "function",
                            "function": {
                                "name": "test",
                                "arguments": "not valid json"
                            }
                        }]
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "malformed tool arguments should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Parse(_)),
            "should be a Parse error variant"
        );
        assert!(
            err.to_string().contains("test"),
            "error should mention the tool name"
        );
    }

    #[tokio::test]
    async fn complete_timeout() {
        let mock_server = MockServer::start().await;

        // Mock server that delays response beyond timeout
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
            .mount(&mock_server)
            .await;

        // Client with 1 second timeout
        let client = OpenAiClient::new(mock_server.uri(), "gpt-4", 1).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "timeout should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Timeout(1)),
            "should be a Timeout error with 1 second"
        );
        assert_eq!(
            err.to_string(),
            "request timed out after 1 seconds",
            "timeout display should include duration"
        );
    }
}

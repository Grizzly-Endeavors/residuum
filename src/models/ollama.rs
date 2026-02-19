//! Ollama model provider implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::http::{HttpClientConfig, SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, Role, ToolCall,
    ToolDefinition,
};

/// Ollama API client implementing the [`ModelProvider`] trait.
#[derive(Clone)]
pub struct OllamaClient {
    http: SharedHttpClient,
    base_url: String,
    model: String,
}

impl OllamaClient {
    /// Create a new Ollama client with the specified timeout.
    ///
    /// This creates a new HTTP client internally. For connection reuse across
    /// multiple clients, use [`with_http_client`](Self::with_http_client) instead.
    ///
    /// # Errors
    /// Returns `ModelError` if the HTTP client cannot be built.
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
            model: model.into(),
        })
    }

    /// Create a new Ollama client with a shared HTTP client.
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
            model: model.into(),
        }
    }

    fn timeout_secs(&self) -> u64 {
        self.http.timeout_secs()
    }
}

#[async_trait]
impl ModelProvider for OllamaClient {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<OllamaMessage> = messages.iter().map(Into::into).collect();

        let ollama_tools: Vec<OllamaTool> = tools
            .iter()
            .map(|t| OllamaTool {
                r#type: "function".to_string(),
                function: OllamaFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        let request = OllamaChatRequest {
            model: &self.model,
            messages: ollama_messages,
            tools: (!ollama_tools.is_empty()).then_some(ollama_tools),
            stream: false,
        };

        let response = self
            .http
            .client()
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| map_request_error(e, self.timeout_secs()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .json::<OllamaErrorResponse>()
                .await
                .map_or_else(|_| format!("{status}: unknown error"), |e| e.error);
            return Err(ModelError::Api(error_body));
        }

        let chat_response: OllamaChatResponse = response.json().await?;

        let content = chat_response.message.content.unwrap_or_default();
        let tool_calls = chat_response
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, tc)| ToolCall {
                id: format!("call_{i}"),
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        Ok(ModelResponse::new(content, tool_calls))
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// Ollama API request/response types

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

impl From<&Message> for OllamaMessage {
    fn from(msg: &Message) -> Self {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };

        Self {
            role: role.to_string(),
            content: (!msg.content.is_empty()).then(|| msg.content.clone()),
            tool_calls: msg.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|tc| OllamaToolCall {
                        function: OllamaFunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    })
                    .collect()
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaFunction,
}

#[derive(Serialize, Deserialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaResponseMessage,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Deserialize)]
struct OllamaErrorResponse {
    error: String,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::CompletionOptions;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_message_conversion() {
        let msg = Message {
            role: Role::User,
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };

        let ollama_msg: OllamaMessage = (&msg).into();
        assert_eq!(ollama_msg.role, "user", "role should be user");
        assert_eq!(
            ollama_msg.content,
            Some("Hello".to_string()),
            "content should match"
        );
    }

    #[tokio::test]
    async fn test_complete_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you today?"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = OllamaClient::new(mock_server.uri(), "test-model", 60).unwrap();
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
            "content should match response body"
        );
        assert!(response.tool_calls.is_empty(), "should have no tool calls");
        assert!(
            response.is_complete(),
            "text-only response should be complete"
        );
    }

    #[tokio::test]
    async fn test_complete_api_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "model 'nonexistent' not found"
            })))
            .mount(&mock_server)
            .await;

        let client = OllamaClient::new(mock_server.uri(), "nonexistent", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "should return an error for 404");
        let err = result.unwrap_err();
        assert!(matches!(err, ModelError::Api(_)), "should be an Api error");
        assert!(
            err.to_string().contains("not found"),
            "error should contain 'not found'"
        );
    }

    #[tokio::test]
    async fn test_complete_with_tool_calls() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "function": {
                            "name": "bash",
                            "arguments": {"command": "ls -la"}
                        }
                    }]
                }
            })))
            .mount(&mock_server)
            .await;

        let client = OllamaClient::new(mock_server.uri(), "test-model", 60).unwrap();
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
        assert_eq!(response.tool_calls.len(), 1, "should have one tool call");
        assert_eq!(
            response.tool_calls.first().map(|t| &t.name),
            Some(&"bash".to_string()),
            "tool name should be bash"
        );
        assert_eq!(
            response.tool_calls.first().map(|t| &t.id),
            Some(&"call_0".to_string()),
            "tool call id should be synthetic call_0"
        );
        assert!(
            !response.is_complete(),
            "response with tool calls should not be complete"
        );
    }

    #[tokio::test]
    async fn test_complete_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": "internal server error"
            })))
            .mount(&mock_server)
            .await;

        let client = OllamaClient::new(mock_server.uri(), "test-model", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "should return an error for 500");
        assert!(
            matches!(result.unwrap_err(), ModelError::Api(_)),
            "should be an Api error"
        );
    }

    #[tokio::test]
    async fn test_complete_malformed_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&mock_server)
            .await;

        let client = OllamaClient::new(mock_server.uri(), "test-model", 60).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "malformed JSON should fail to parse");
    }

    #[tokio::test]
    async fn test_complete_timeout() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
            .mount(&mock_server)
            .await;

        // Client with 1 second timeout
        let client = OllamaClient::new(mock_server.uri(), "test-model", 1).unwrap();
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "should time out");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Timeout(1)),
            "should be a Timeout error with 1 second"
        );
        assert_eq!(
            err.to_string(),
            "request timed out after 1 seconds",
            "timeout message should include duration"
        );
    }
}

//! Ollama model provider implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::embedding::{EmbeddingProvider, EmbeddingResponse};
use super::http::{SharedHttpClient, map_request_error, read_error_body, warn_if_insecure_remote};
use super::retry::{RetryConfig, with_retry};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ResponseFormat,
    ThinkingConfig, ToolCall, ToolDefinition,
};

/// Ollama API client implementing the [`ModelProvider`] trait.
#[derive(Clone)]
pub(crate) struct OllamaClient {
    http: SharedHttpClient,
    base_url: String,
    model: String,
    api_key: Option<String>,
    keep_alive: Option<String>,
    retry: RetryConfig,
}

impl OllamaClient {
    /// Create a new Ollama client with a shared HTTP client.
    ///
    /// Use this constructor to share connection pools across multiple model providers.
    #[must_use]
    pub fn with_http_client(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        keep_alive: Option<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            model: model.into(),
            api_key: None,
            keep_alive,
            retry,
        }
    }

    /// Create a new Ollama client with a shared HTTP client and API key authentication.
    ///
    /// Use this constructor for cloud-hosted Ollama instances that require authentication.
    #[must_use]
    pub fn with_http_client_and_api_key(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
        keep_alive: Option<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            model: model.into(),
            api_key: Some(api_key.into()),
            keep_alive,
            retry,
        }
    }

    #[tracing::instrument(skip_all, fields(
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.as_ref().map_or(0, Vec::len),
    ))]
    async fn send_completion(
        http: &SharedHttpClient,
        url: &str,
        api_key: Option<&str>,
        request: &OllamaChatRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let timeout_secs = http.timeout_secs();

        debug!(
            model = %request.model,
            message_count = request.messages.len(),
            tool_count = request.tools.as_ref().map_or(0, Vec::len),
            "sending ollama completion request"
        );

        let mut req_builder = http.client().post(url).json(request);
        if let Some(key) = api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;

        if !response.status().is_success() {
            let status = response.status();
            let raw_body = read_error_body(response).await;
            tracing::warn!(
                status = %status,
                response_body = %raw_body,
                "ollama API error"
            );
            let error_msg = serde_json::from_str::<OllamaErrorResponse>(&raw_body)
                .map_or_else(|_| format!("{status}: {raw_body}"), |e| e.error);
            return Err(ModelError::Api(error_msg));
        }

        let body = response
            .text()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;
        let chat_response: OllamaChatResponse = serde_json::from_str(&body)
            .map_err(|e| ModelError::Parse(format!("failed to parse ollama response: {e}")))?;

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

        let mut resp = ModelResponse::new(content, tool_calls);
        resp.thinking = chat_response.message.thinking;
        info!(
            model = %request.model,
            content_len = resp.content.len(),
            tool_calls = resp.tool_calls.len(),
            "ollama completion received"
        );
        Ok(resp)
    }
}

#[async_trait]
impl ModelProvider for OllamaClient {
    #[tracing::instrument(skip_all, fields(model = %self.model, message_count = messages.len(), tool_count = tools.len()))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
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
        let has_tools = !ollama_tools.is_empty();
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let keep_alive = self.keep_alive.clone();
        let http = self.http.clone();

        let format = match &options.response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonSchema { schema, .. } => Some(schema.clone()),
        };
        let model_options = options.temperature.map(|t| OllamaModelOptions {
            temperature: Some(t),
        });

        let think = options.thinking.as_ref().map(|tc| match tc {
            ThinkingConfig::Toggle(val) => *val,
            ThinkingConfig::Level(_) => true,
        });

        with_retry(&self.retry, || {
            let url = url.clone();
            let ollama_messages = ollama_messages.clone();
            let ollama_tools = ollama_tools.clone();
            let model = model.clone();
            let api_key = api_key.clone();
            let keep_alive = keep_alive.clone();
            let http = http.clone();
            let format = format.clone();
            let model_options = model_options.clone();

            async move {
                let request = OllamaChatRequest {
                    model: &model,
                    messages: ollama_messages,
                    tools: has_tools.then_some(ollama_tools),
                    stream: false,
                    format,
                    options: model_options,
                    keep_alive,
                    think,
                };
                Self::send_completion(&http, &url, api_key.as_deref(), &request).await
            }
        })
        .await
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// Ollama API request/response types

/// Nested model options for Ollama (e.g. temperature).
#[derive(Debug, Serialize, Clone)]
struct OllamaModelOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaModelOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone)]
struct OllamaMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
}

impl From<&Message> for OllamaMessage {
    fn from(msg: &Message) -> Self {
        Self {
            role: msg.role.as_str().to_string(),
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
            images: if msg.images.is_empty() {
                None
            } else {
                Some(msg.images.iter().map(|img| img.data.clone()).collect())
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct OllamaTool {
    r#type: String,
    function: OllamaFunction,
}

#[derive(Serialize, Deserialize, Clone)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
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
    #[serde(default)]
    thinking: Option<String>,
}

#[derive(Deserialize)]
struct OllamaErrorResponse {
    error: String,
}

/// Ollama embeddings API client.
pub(crate) struct OllamaEmbeddingClient {
    http: SharedHttpClient,
    base_url: String,
    model: String,
    api_key: Option<String>,
    keep_alive: Option<String>,
    retry: RetryConfig,
}

impl OllamaEmbeddingClient {
    /// Create a new Ollama embedding client with a shared HTTP client.
    #[must_use]
    pub fn with_http_client(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        keep_alive: Option<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            model: model.into(),
            api_key: None,
            keep_alive,
            retry,
        }
    }

    /// Create a new Ollama embedding client with a shared HTTP client and API key authentication.
    #[must_use]
    pub fn with_http_client_and_api_key(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
        keep_alive: Option<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            model: model.into(),
            api_key: Some(api_key.into()),
            keep_alive,
            retry,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingClient {
    #[tracing::instrument(skip_all, fields(model = %self.model, count = texts.len()))]
    async fn embed(&self, texts: &[&str]) -> Result<EmbeddingResponse, ModelError> {
        let url = format!("{}/api/embed", self.base_url);
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let keep_alive = self.keep_alive.clone();
        let http = self.http.clone();
        let timeout_secs = self.http.timeout_secs();

        with_retry(&self.retry, || {
            let url = url.clone();
            let model = model.clone();
            let api_key = api_key.clone();
            let keep_alive = keep_alive.clone();
            let http = http.clone();

            async move {
                let request = OllamaEmbedRequest {
                    model: &model,
                    input: texts,
                    keep_alive,
                };

                debug!(model = %model, count = texts.len(), "sending ollama embed request");

                let mut req_builder = http.client().post(&url).json(&request);

                if let Some(ref key) = api_key {
                    req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
                }

                let response = req_builder
                    .send()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let raw_body = read_error_body(response).await;
                    warn!(
                        status = %status,
                        response_body = %raw_body,
                        "ollama embed API error"
                    );
                    let error_msg = serde_json::from_str::<OllamaErrorResponse>(&raw_body)
                        .map_or_else(|_| format!("{status}: {raw_body}"), |e| e.error);
                    return Err(ModelError::Api(error_msg));
                }

                let body = response
                    .text()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;
                let embed_response: OllamaEmbedResponse =
                    serde_json::from_str(&body).map_err(|e| {
                        ModelError::Parse(format!("failed to parse ollama embed response: {e}"))
                    })?;

                let dimensions =
                    embed_response
                        .embeddings
                        .first()
                        .map(Vec::len)
                        .ok_or_else(|| {
                            ModelError::Parse("embeddings response contained no data".to_string())
                        })?;

                info!(model = %model, count = embed_response.embeddings.len(), dimensions, "ollama embeddings received");
                Ok(EmbeddingResponse {
                    embeddings: embed_response.embeddings,
                    dimensions,
                })
            }
        })
        .await
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[derive(Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::CompletionOptions;
    use crate::models::http::{HttpClientConfig, SharedHttpClient};
    use crate::models::retry::RetryConfig;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(url: impl Into<String>, model: &str) -> OllamaClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OllamaClient::with_http_client(http, url, model, None, RetryConfig::no_retry())
    }

    fn make_client_with_timeout(url: impl Into<String>, model: &str, timeout: u64) -> OllamaClient {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(timeout)).unwrap();
        OllamaClient::with_http_client(http, url, model, None, RetryConfig::no_retry())
    }

    #[test]
    fn message_conversion() {
        let msg = Message::user("Hello");

        let ollama_msg: OllamaMessage = (&msg).into();
        assert_eq!(ollama_msg.role, "user", "role should be user");
        assert_eq!(
            ollama_msg.content,
            Some("Hello".to_string()),
            "content should match"
        );
    }

    #[test]
    fn message_conversion_tool_empty_content_is_none() {
        let msg = Message::tool("", "call_1");
        let ollama_msg: OllamaMessage = (&msg).into();
        assert_eq!(ollama_msg.role, "tool", "role should be tool");
        assert!(
            ollama_msg.content.is_none(),
            "empty content should become None"
        );
    }

    #[test]
    fn message_conversion_assistant_with_tool_calls() {
        let msg = Message::assistant(
            "thinking",
            Some(vec![ToolCall {
                id: "call_0".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }]),
        );
        let ollama_msg: OllamaMessage = (&msg).into();
        assert_eq!(ollama_msg.role, "assistant", "role should be assistant");
        assert_eq!(
            ollama_msg.content,
            Some("thinking".to_string()),
            "content should match"
        );
        let tool_calls = ollama_msg.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1, "should have one tool call");
        assert_eq!(
            tool_calls.first().unwrap().function.name,
            "bash",
            "tool call name should match"
        );
        let serialized = serde_json::to_value(tool_calls.first().unwrap()).unwrap();
        assert_eq!(
            serialized
                .get("function")
                .unwrap()
                .get("arguments")
                .unwrap(),
            &serde_json::json!({"command": "ls"}),
            "arguments should be native JSON"
        );
    }

    #[test]
    fn message_conversion_user_with_images() {
        use crate::models::ImageData;
        let images = vec![ImageData {
            media_type: "image/png".to_string(),
            data: "base64data".to_string(),
        }];
        let msg = Message::user_with_images("look at this", images);
        let ollama_msg: OllamaMessage = (&msg).into();
        assert_eq!(ollama_msg.role, "user", "role should be user");
        let imgs = ollama_msg.images.unwrap();
        assert_eq!(imgs.len(), 1, "should have one image");
        assert_eq!(
            imgs.first().unwrap(),
            "base64data",
            "image data should match"
        );
    }

    #[tokio::test]
    async fn complete_success() {
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

        let client = make_client(mock_server.uri(), "test-model");
        let messages = vec![Message::user("Hello")];

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
    async fn complete_api_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "model 'nonexistent' not found"
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "nonexistent");
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
    async fn complete_with_tool_calls() {
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

        let client = make_client(mock_server.uri(), "test-model");
        let messages = vec![Message::user("List files")];

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
    async fn complete_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": "internal server error"
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
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
    async fn complete_malformed_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "malformed JSON should fail to parse");
    }

    #[tokio::test]
    async fn complete_timeout() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
            .mount(&mock_server)
            .await;

        // Client with 1 second timeout
        let client = make_client_with_timeout(mock_server.uri(), "test-model", 1);
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

    #[tokio::test]
    async fn complete_with_json_schema_response_format() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "format": {
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"}
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "{\"answer\": \"hello\"}"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
        let options = CompletionOptions {
            response_format: crate::models::ResponseFormat::JsonSchema {
                name: "test_schema".to_string(),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"}
                    }
                }),
            },
            ..CompletionOptions::default()
        };

        let response = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await
            .unwrap();
        assert_eq!(
            response.content, "{\"answer\": \"hello\"}",
            "should return JSON content"
        );
    }

    #[tokio::test]
    async fn temperature_nested_in_options() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "options": {
                    "temperature": 1.2
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": { "role": "assistant", "content": "ok" }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
        let options = CompletionOptions {
            temperature: Some(1.2),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(result.is_ok(), "request with temperature should succeed");
    }

    #[tokio::test]
    async fn temperature_options_absent_when_none() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": { "role": "assistant", "content": "ok" }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
        let result = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await;
        assert!(result.is_ok(), "request without temperature should succeed");

        let requests = mock_server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("options").is_none(),
            "options should be absent when temperature is None"
        );
    }

    fn make_client_with_api_key(
        url: impl Into<String>,
        model: &str,
        api_key: &str,
    ) -> OllamaClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OllamaClient::with_http_client_and_api_key(
            http,
            url,
            model,
            api_key,
            None,
            RetryConfig::no_retry(),
        )
    }

    #[tokio::test]
    async fn api_key_sends_bearer_header() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(header("Authorization", "Bearer test-ollama-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "authenticated response"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client_with_api_key(mock_server.uri(), "test-model", "test-ollama-key");
        let messages = vec![Message::user("Hello")];

        let result = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await;
        assert!(
            result.is_ok(),
            "request with api key should succeed: {result:?}"
        );
        assert_eq!(
            result.unwrap().content,
            "authenticated response",
            "content should match"
        );
    }

    #[tokio::test]
    async fn no_auth_header_without_api_key() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "no auth response"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "test-model");
        let messages = vec![Message::user("Hello")];

        let result = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await;
        assert!(
            result.is_ok(),
            "request without api key should succeed: {result:?}"
        );

        let requests = mock_server.received_requests().await.unwrap();
        let req = requests.first().unwrap();
        let has_auth = req
            .headers
            .iter()
            .any(|(name, _)| name.as_str().eq_ignore_ascii_case("authorization"));
        assert!(
            !has_auth,
            "request should not contain an Authorization header"
        );
    }

    #[tokio::test]
    async fn think_flag_included_when_thinking_set() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "think": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "ok"
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "deepseek-r1");
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Toggle(true)),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(
            result.is_ok(),
            "request with think flag should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn thinking_response_extracted() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "Final answer",
                    "thinking": "step by step reasoning"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "deepseek-r1");
        let result = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.content, "Final answer", "content should match");
        assert_eq!(
            result.thinking.as_deref(),
            Some("step by step reasoning"),
            "thinking should be extracted from response"
        );
    }

    // --- Embedding client tests ---

    use crate::models::embedding::EmbeddingProvider;

    #[tokio::test]
    async fn keep_alive_included_in_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "keep_alive": "10m"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": "ok"
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let client = OllamaClient::with_http_client(
            http,
            mock_server.uri(),
            "test-model",
            Some("10m".to_string()),
            RetryConfig::no_retry(),
        );
        let result = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await;
        assert!(
            result.is_ok(),
            "request with keep_alive should succeed: {result:?}"
        );
    }

    fn make_embedding_client(url: impl Into<String>, model: &str) -> OllamaEmbeddingClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OllamaEmbeddingClient::with_http_client(http, url, model, None, RetryConfig::no_retry())
    }

    #[tokio::test]
    async fn embed_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [
                    [0.1, 0.2, 0.3],
                    [0.4, 0.5, 0.6]
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "nomic-embed-text");
        let result = client.embed(&["hello", "world"]).await.unwrap();

        assert_eq!(result.embeddings.len(), 2, "should have 2 embeddings");
        assert_eq!(
            result.dimensions, 3,
            "each embedding should have 3 dimensions"
        );
        assert_eq!(
            result.embeddings.first().map(Vec::as_slice),
            Some([0.1_f32, 0.2, 0.3].as_slice()),
            "first embedding should match"
        );
        assert_eq!(
            result.embeddings.get(1).map(Vec::as_slice),
            Some([0.4_f32, 0.5, 0.6].as_slice()),
            "second embedding should match"
        );
    }

    #[tokio::test]
    async fn embed_batch() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [
                    [1.0, 2.0],
                    [3.0, 4.0],
                    [5.0, 6.0]
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "nomic-embed-text");
        let result = client.embed(&["a", "b", "c"]).await.unwrap();

        assert_eq!(result.embeddings.len(), 3, "should have 3 embeddings");
        assert_eq!(
            result.dimensions, 2,
            "each embedding should have 2 dimensions"
        );
    }

    #[tokio::test]
    async fn embed_api_error_404() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "model not found"
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "nonexistent");
        let result = client.embed(&["hello"]).await;

        assert!(result.is_err(), "should return an error for 404");
        let err = result.unwrap_err();
        assert!(matches!(err, ModelError::Api(_)), "should be an Api error");
        assert!(
            err.to_string().contains("model not found"),
            "error should contain 'model not found'"
        );
    }

    #[tokio::test]
    async fn embed_empty_input_sends_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": []
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "nomic-embed-text");
        let result = client.embed(&[]).await;
        assert!(
            result.is_err(),
            "empty embeddings response should return error"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no data"),
            "error should mention no data: {err}"
        );
    }
}

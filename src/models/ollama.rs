//! Ollama model provider implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::embedding::{EmbeddingProvider, EmbeddingResponse};
use super::http::{SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::retry::{RetryConfig, with_retry};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ResponseFormat, ToolCall,
    ToolDefinition,
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
        let timeout_secs = self.timeout_secs();

        let format = match &options.response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonSchema { schema, .. } => Some(schema.clone()),
        };
        let model_options = options.temperature.map(|t| OllamaModelOptions {
            temperature: Some(t),
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
                };

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
}

#[derive(Serialize, Deserialize, Clone)]
struct OllamaMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
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
                    let error_body = response
                        .json::<OllamaErrorResponse>()
                        .await
                        .map_or_else(|_| format!("{status}: unknown error"), |e| e.error);
                    return Err(ModelError::Api(error_body));
                }

                let embed_response: OllamaEmbedResponse = response.json().await?;

                let dimensions =
                    embed_response
                        .embeddings
                        .first()
                        .map(Vec::len)
                        .ok_or_else(|| {
                            ModelError::Parse("embeddings response contained no data".to_string())
                        })?;

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
}

//! Client for OpenAI-compatible chat completion APIs.
//!
//! Supports various providers including Azure, vLLM, LM Studio, Fireworks, and
//! other compatible endpoints. Host-specific behavior on top of the shared wire
//! format is selected with [`OpenAiDialect`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::inference::embedding::{EmbeddingProvider, EmbeddingResponse};
use crate::inference::http::{
    SharedHttpClient, map_request_error, read_error_body, warn_if_insecure_remote,
};
use crate::inference::retry::{RetryConfig, with_retry};
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ResponseFormat, ThinkingConfig, ThinkingLevel, ToolCall, ToolDefinition, Usage,
};

/// Fireworks response header carrying the prompt tokens served from cache.
const FIREWORKS_CACHED_PROMPT_TOKENS_HEADER: &str = "fireworks-cached-prompt-tokens";

/// Fireworks request header that pins requests to a replica holding a warm cache.
const FIREWORKS_SESSION_AFFINITY_HEADER: &str = "x-session-affinity";

/// Host-specific behavior layered on the OpenAI-compatible wire format.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum OpenAiDialect {
    /// `OpenAI` and generic compatible servers.
    #[default]
    OpenAi,
    /// Fireworks AI: reports cache hits in response headers rather than the
    /// `usage` body, has no hosted web search tool, and routes by an affinity key.
    Fireworks {
        /// Value for the `x-session-affinity` header.
        session_affinity: Option<String>,
    },
}

impl OpenAiDialect {
    /// Whether the host understands `OpenAI`'s hosted `web_search_preview` tool.
    fn supports_hosted_web_search(&self) -> bool {
        matches!(self, Self::OpenAi)
    }
}

/// OpenAI-compatible API client.
#[derive(Clone)]
pub(crate) struct OpenAiClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: Option<String>,
    model: String,
    retry: RetryConfig,
    dialect: OpenAiDialect,
}

impl OpenAiClient {
    /// Create a new client with a shared HTTP client (no authentication).
    ///
    /// Use this constructor to share connection pools across multiple model providers.
    #[must_use]
    pub fn with_http_client(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: None,
            model: model.into(),
            retry,
            dialect: OpenAiDialect::default(),
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
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: Some(api_key.into()),
            model: model.into(),
            retry,
            dialect: OpenAiDialect::default(),
        }
    }

    /// Select the host dialect this client speaks.
    #[must_use]
    pub fn with_dialect(mut self, dialect: OpenAiDialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// Map thinking config to the `reasoning_effort` parameter.
    fn build_reasoning_effort(thinking: &ThinkingConfig) -> Option<String> {
        match thinking {
            ThinkingConfig::Level(ThinkingLevel::Low) => Some("low".to_string()),
            ThinkingConfig::Level(ThinkingLevel::Medium) | ThinkingConfig::Toggle(true) => {
                Some("medium".to_string())
            }
            ThinkingConfig::Level(ThinkingLevel::High) => Some("high".to_string()),
            ThinkingConfig::Toggle(false) => None,
        }
    }

    /// Send a pre-built request to the OpenAI-compatible API and parse the response.
    #[tracing::instrument(skip_all, fields(
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.as_ref().map_or(0, Vec::len),
    ))]
    async fn send_completion(
        http: &SharedHttpClient,
        url: &str,
        api_key: Option<&str>,
        dialect: &OpenAiDialect,
        request: &ChatCompletionRequest<'_>,
    ) -> Result<InferenceResponse, InferenceError> {
        let timeout_secs = http.timeout_secs();
        let request_json = serde_json::to_string(request)
            .map_err(|e| InferenceError::Parse(format!("failed to serialize request: {e}")))?;

        debug!(model = %request.model, "sending openai completion request");

        let mut req_builder = http
            .client()
            .post(url)
            .body(request_json.clone())
            .header("content-type", "application/json");

        if let Some(key) = api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        if let OpenAiDialect::Fireworks {
            session_affinity: Some(affinity),
        } = dialect
        {
            req_builder = req_builder.header(FIREWORKS_SESSION_AFFINITY_HEADER, affinity);
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
                request_body = %request_json,
                "openai API error — full request/response for diagnosis"
            );
            let error_body = serde_json::from_str::<OpenAiErrorResponse>(&raw_body)
                .map_or_else(|_| raw_body, |e| e.error.message);
            return Err(InferenceError::Api(format!("{status}: {error_body}")));
        }

        let header_cached_tokens = match dialect {
            OpenAiDialect::Fireworks { .. } => fireworks_cached_prompt_tokens(response.headers()),
            OpenAiDialect::OpenAi => None,
        };

        let body = response
            .text()
            .await
            .map_err(|e| map_request_error(e, timeout_secs))?;
        let chat_response: ChatCompletionResponse = serde_json::from_str(&body)
            .map_err(|e| InferenceError::Parse(format!("failed to parse openai response: {e}")))?;

        let usage = chat_response.usage.map(|u| Usage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
            cache_creation_tokens: None,
            cache_read_tokens: u
                .prompt_tokens_details
                .and_then(|d| d.cached_tokens)
                .or(header_cached_tokens),
        });

        let choice = chat_response.choices.into_iter().next().ok_or_else(|| {
            InferenceError::Parse(
                "OpenAI API response contained no choices in response".to_string(),
            )
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
                        InferenceError::Parse(format!(
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
            .collect::<Result<Vec<_>, InferenceError>>()?;

        let mut resp = InferenceResponse::new(content, tool_calls);
        resp.usage = usage;
        info!(
            model = %request.model,
            content_len = resp.content.len(),
            tool_calls = resp.tool_calls.len(),
            "openai completion received"
        );
        Ok(resp)
    }
}

#[async_trait]
impl InferenceProvider for OpenAiClient {
    #[tracing::instrument(skip_all, fields(model = %self.model, message_count = messages.len(), tool_count = tools.len()))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        let url = format!("{}/chat/completions", self.base_url);
        let openai_messages: Vec<OpenAiMessage> = messages.iter().map(Into::into).collect();
        let mut openai_tools: Vec<OpenAiToolEntry> = tools
            .iter()
            .map(|t| {
                OpenAiToolEntry::Function(OpenAiTool {
                    r#type: "function".to_string(),
                    function: OpenAiFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    },
                })
            })
            .collect();
        if let Some(ws) = &options.web_search
            && self.dialect.supports_hosted_web_search()
        {
            openai_tools.push(OpenAiToolEntry::WebSearch(OpenAiWebSearchTool {
                r#type: "web_search_preview".to_string(),
                search_context_size: ws.search_context_size.clone(),
            }));
        }
        let has_tools = !openai_tools.is_empty();
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let http = self.http.clone();
        let dialect = self.dialect.clone();

        let response_format = match &options.response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonSchema { name, schema } => Some(OpenAiResponseFormat {
                r#type: "json_schema".to_string(),
                json_schema: OpenAiJsonSchema {
                    name: name.clone(),
                    schema: schema.clone(),
                    strict: true,
                },
            }),
        };
        let temperature = options.temperature;

        let reasoning_effort = options
            .thinking
            .as_ref()
            .and_then(Self::build_reasoning_effort);

        with_retry(&self.retry, || {
            let url = url.clone();
            let openai_messages = openai_messages.clone();
            let openai_tools = openai_tools.clone();
            let model = model.clone();
            let api_key = api_key.clone();
            let http = http.clone();
            let response_format = response_format.clone();
            let reasoning_effort = reasoning_effort.clone();
            let dialect = dialect.clone();

            async move {
                let request = ChatCompletionRequest {
                    model: &model,
                    messages: openai_messages,
                    tools: has_tools.then_some(openai_tools),
                    tool_choice: has_tools.then_some("auto"),
                    response_format,
                    temperature,
                    reasoning_effort,
                };

                Self::send_completion(&http, &url, api_key.as_deref(), &dialect, &request).await
            }
        })
        .await
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Parse the Fireworks cached-prompt-token header, logging a malformed value.
fn fireworks_cached_prompt_tokens(headers: &reqwest::header::HeaderMap) -> Option<u32> {
    let raw = headers.get(FIREWORKS_CACHED_PROMPT_TOKENS_HEADER)?;
    let parsed = raw.to_str().ok().and_then(|v| v.trim().parse::<u32>().ok());
    if parsed.is_none() {
        tracing::warn!(
            header = FIREWORKS_CACHED_PROMPT_TOKENS_HEADER,
            value = ?raw,
            "fireworks returned an unparseable cached-token header; cache stats unavailable for this call"
        );
    }
    parsed
}

// --- OpenAI API request/response types ---

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiToolEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<OpenAiResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Serialize, Clone)]
struct OpenAiResponseFormat {
    r#type: String,
    json_schema: OpenAiJsonSchema,
}

#[derive(Serialize, Clone)]
struct OpenAiJsonSchema {
    name: String,
    schema: serde_json::Value,
    strict: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum OpenAiContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum OpenAiContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiImageUrl {
    url: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenAiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl From<&Message> for OpenAiMessage {
    fn from(msg: &Message) -> Self {
        let content = if !msg.images.is_empty() {
            let mut parts: Vec<OpenAiContentPart> = Vec::new();
            if !msg.content.is_empty() {
                parts.push(OpenAiContentPart::Text {
                    text: msg.content.clone(),
                });
            }
            for img in &msg.images {
                parts.push(OpenAiContentPart::ImageUrl {
                    image_url: OpenAiImageUrl {
                        url: format!("data:{};base64,{}", img.media_type, img.data),
                    },
                });
            }
            Some(OpenAiContent::Parts(parts))
        } else if msg.content.is_empty() {
            None
        } else {
            Some(OpenAiContent::Text(msg.content.clone()))
        };

        Self {
            role: msg.role.as_str().to_string(),
            content,
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

/// Heterogeneous tool entry for the `OpenAI` `tools` array.
#[derive(Serialize, Clone)]
#[serde(untagged)]
enum OpenAiToolEntry {
    /// Standard function tool.
    Function(OpenAiTool),
    /// Web search tool.
    WebSearch(OpenAiWebSearchTool),
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiFunction,
}

#[derive(Serialize, Clone)]
struct OpenAiWebSearchTool {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_context_size: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiToolCall {
    id: String,
    r#type: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String, // OpenAI returns arguments as JSON string
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Deserialize)]
struct OpenAiPromptTokensDetails {
    cached_tokens: Option<u32>,
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

// --- OpenAI Embeddings API types ---

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(Deserialize)]
struct EmbeddingApiResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: u32,
}

/// OpenAI-compatible embeddings API client.
pub(crate) struct OpenAiEmbeddingClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: Option<String>,
    model: String,
    retry: RetryConfig,
}

impl OpenAiEmbeddingClient {
    /// Create a new embedding client with a shared HTTP client (no authentication).
    #[must_use]
    pub fn with_http_client(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: None,
            model: model.into(),
            retry,
        }
    }

    /// Create a new embedding client with a shared HTTP client and API key authentication.
    #[must_use]
    pub fn with_http_client_and_api_key(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);

        Self {
            http,
            base_url,
            api_key: Some(api_key.into()),
            model: model.into(),
            retry,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingClient {
    #[tracing::instrument(skip_all, fields(model = %self.model, count = texts.len()))]
    async fn embed(&self, texts: &[&str]) -> Result<EmbeddingResponse, InferenceError> {
        let url = format!("{}/embeddings", self.base_url);
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let http = self.http.clone();
        let timeout_secs = self.http.timeout_secs();

        with_retry(&self.retry, || {
            let url = url.clone();
            let model = model.clone();
            let api_key = api_key.clone();
            let http = http.clone();

            async move {
                let request = EmbeddingRequest {
                    model: &model,
                    input: texts,
                };

                debug!(model = %model, count = texts.len(), "sending openai embed request");

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
                    tracing::warn!(status = %status, response_body = %raw_body, "openai embed API error");
                    let error_body = serde_json::from_str::<OpenAiErrorResponse>(&raw_body)
                        .map_or_else(|_| raw_body, |e| e.error.message);
                    return Err(InferenceError::Api(format!("{status}: {error_body}")));
                }

                let body = response
                    .text()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;
                let mut api_response: EmbeddingApiResponse =
                    serde_json::from_str(&body).map_err(|e| {
                        InferenceError::Parse(format!("failed to parse openai embedding response: {e}"))
                    })?;

                if api_response.data.is_empty() {
                    return Err(InferenceError::Parse(
                        "embeddings response contained no data".to_string(),
                    ));
                }

                api_response.data.sort_by_key(|d| d.index);

                let dimensions = api_response.data.first().map_or(0, |d| d.embedding.len());

                let embeddings: Vec<Vec<f32>> =
                    api_response.data.into_iter().map(|d| d.embedding).collect();
                info!(model = %model, count = embeddings.len(), dimensions, "openai embeddings received");

                Ok(EmbeddingResponse {
                    embeddings,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::CompletionOptions;
    use crate::inference::http::{HttpClientConfig, SharedHttpClient};
    use crate::inference::retry::RetryConfig;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(url: impl Into<String>, model: &str) -> OpenAiClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OpenAiClient::with_http_client(http, url, model, RetryConfig::no_retry())
    }

    fn make_client_with_key(url: impl Into<String>, model: &str, api_key: &str) -> OpenAiClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OpenAiClient::with_http_client_and_api_key(
            http,
            url,
            model,
            api_key,
            RetryConfig::no_retry(),
        )
    }

    fn make_client_with_timeout(url: impl Into<String>, model: &str, timeout: u64) -> OpenAiClient {
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(timeout)).unwrap();
        OpenAiClient::with_http_client(http, url, model, RetryConfig::no_retry())
    }

    #[test]
    fn message_conversion_user() {
        let msg = Message::user("Hello");

        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "user", "role should map to user");
        // Content should serialize as a plain string (Text variant)
        let content_json = serde_json::to_value(&openai_msg.content).unwrap();
        assert_eq!(
            content_json,
            serde_json::json!("Hello"),
            "content should be preserved as plain string"
        );
        assert!(
            openai_msg.tool_calls.is_none(),
            "tool_calls should be absent"
        );
    }

    #[test]
    fn message_conversion_assistant_with_tool_calls() {
        let msg = Message::assistant(
            "",
            Some(vec![ToolCall {
                id: "call_123".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }]),
        );

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
        let msg = Message::tool("result output", "call_123");

        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "tool", "role should map to tool");
        let content_json = serde_json::to_value(&openai_msg.content).unwrap();
        assert_eq!(
            content_json,
            serde_json::json!("result output"),
            "content should be preserved"
        );
        assert_eq!(
            openai_msg.tool_call_id,
            Some("call_123".to_string()),
            "tool_call_id should be preserved"
        );
    }

    #[test]
    fn message_conversion_user_with_images() {
        use crate::inference::ImageData;
        let images = vec![ImageData {
            media_type: "image/jpeg".to_string(),
            data: "base64abc123".to_string(),
        }];
        let msg = Message::user_with_images("look at this", images);
        let openai_msg: OpenAiMessage = (&msg).into();
        assert_eq!(openai_msg.role, "user", "role should be user");
        let content_json = serde_json::to_value(&openai_msg.content).unwrap();
        assert!(
            content_json.is_array(),
            "content should be Parts array when images are present"
        );
        let parts = content_json.as_array().unwrap();
        assert_eq!(parts.len(), 2, "should have text and image parts");
        let first_part = parts.first().unwrap();
        assert_eq!(first_part["type"], "text", "first part should be text type");
        assert_eq!(
            first_part["text"], "look at this",
            "text part content should match"
        );
        let second_part = parts.last().unwrap();
        assert_eq!(
            second_part["type"], "image_url",
            "second part should be image_url type"
        );
        assert_eq!(
            second_part
                .pointer("/image_url/url")
                .and_then(serde_json::Value::as_str),
            Some("data:image/jpeg;base64,base64abc123"),
            "image URL should use data URI format"
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let messages = vec![Message::user("Hello")];

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

        let client = make_client_with_key(mock_server.uri(), "gpt-4", "sk-test-key");
        let messages = vec![Message::user("Hello")];

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

        let client = make_client(mock_server.uri(), "gpt-4");
        let messages = vec![Message::user("List files")];

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
    async fn complete_passes_through_a_malformed_tool_call_name_verbatim() {
        // Fireworks (and vLLM, upstream) have shipped tool-call parsers for
        // GLM models that fail to strip the model's own `<arg_key>`/
        // `<arg_value>` tool-call template out of a structured response,
        // leaking it into `function.name` instead of a clean `write_file`.
        // This client has no name/markup parsing of its own — it's a thin
        // OpenAI-compatible wire decoder — so it must pass the field through
        // exactly as the API returned it. Downstream (`agent::turn`) is
        // responsible for recognizing the shape is bogus and failing
        // legibly instead of chasing it through the tool registries.
        let mock_server = MockServer::start().await;
        let malformed_name = "write_file\tcontent</arg_key><arg_value># Hello";

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_glm_1",
                            "type": "function",
                            "function": {
                                "name": malformed_name,
                                "arguments": "{}"
                            }
                        }]
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let messages = vec![Message::user("write a file")];

        let response = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_eq!(
            response.tool_calls.first().map(|t| t.name.as_str()),
            Some(malformed_name),
            "the client must not attempt to repair or hand-parse markup out of the tool name; \
             that decision belongs to the caller, not the wire-format decoder"
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "401 should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Api(_)),
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "429 should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Api(_)),
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "500 should return error");
        assert!(
            matches!(result.unwrap_err(), InferenceError::Api(_)),
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "empty choices should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Parse(_)),
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

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "malformed tool arguments should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Parse(_)),
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
        let client = make_client_with_timeout(mock_server.uri(), "gpt-4", 1);
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "timeout should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Timeout(1)),
            "should be a Timeout error with 1 second"
        );
        assert_eq!(
            err.to_string(),
            "request timed out after 1 seconds",
            "timeout display should include duration"
        );
    }

    #[tokio::test]
    async fn complete_with_json_schema_response_format() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "test_schema",
                        "strict": true,
                        "schema": {
                            "type": "object",
                            "properties": {
                                "answer": {"type": "string"}
                            }
                        }
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "{\"answer\": \"hello\"}"
                    }
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let options = CompletionOptions {
            response_format: crate::inference::ResponseFormat::JsonSchema {
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
            "should return JSON string content"
        );
    }

    #[tokio::test]
    async fn temperature_included_when_set() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "temperature": 0.5
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "ok"
                    }
                }]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let options = CompletionOptions {
            temperature: Some(0.5),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(result.is_ok(), "request with temperature should succeed");
    }

    #[tokio::test]
    async fn temperature_absent_when_none() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "ok" }
                }]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
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
            body.get("temperature").is_none(),
            "temperature should be absent when None"
        );
    }

    #[tokio::test]
    async fn usage_with_cached_tokens_parsed() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "cached hello"
                    }
                }],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "prompt_tokens_details": {
                        "cached_tokens": 30
                    }
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let result = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await;
        assert!(result.is_ok(), "usage with cache tokens should succeed");

        let resp = result.unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 100, "input tokens should match");
        assert_eq!(usage.output_tokens, 50, "output tokens should match");
        assert_eq!(
            usage.cache_creation_tokens, None,
            "OpenAI does not report cache creation tokens"
        );
        assert_eq!(
            usage.cache_read_tokens,
            Some(30),
            "cache read tokens should match cached_tokens"
        );
    }

    #[tokio::test]
    async fn reasoning_effort_included_when_thinking_set() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "reasoning_effort": "high"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "ok"
                    }
                }]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "o3-mini");
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Level(ThinkingLevel::High)),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(
            result.is_ok(),
            "request with reasoning_effort should succeed: {result:?}"
        );
    }

    // --- Embedding client tests ---

    fn make_embedding_client(url: impl Into<String>, model: &str) -> OpenAiEmbeddingClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OpenAiEmbeddingClient::with_http_client(http, url, model, RetryConfig::no_retry())
    }

    fn make_embedding_client_with_key(
        url: impl Into<String>,
        model: &str,
        api_key: &str,
    ) -> OpenAiEmbeddingClient {
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        OpenAiEmbeddingClient::with_http_client_and_api_key(
            http,
            url,
            model,
            api_key,
            RetryConfig::no_retry(),
        )
    }

    #[tokio::test]
    async fn embed_success() {
        use crate::inference::embedding::EmbeddingProvider;

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    { "embedding": [0.1, 0.2, 0.3], "index": 0 },
                    { "embedding": [0.4, 0.5, 0.6], "index": 1 }
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "text-embedding-3-small");
        let response = client.embed(&["hello", "world"]).await.unwrap();

        assert_eq!(response.embeddings.len(), 2, "should have 2 embeddings");
        assert_eq!(response.dimensions, 3, "dimensions should be 3");
        assert_eq!(
            response.embeddings.first().map(Vec::as_slice),
            Some([0.1_f32, 0.2, 0.3].as_slice()),
            "first embedding should match"
        );
        assert_eq!(
            response.embeddings.get(1).map(Vec::as_slice),
            Some([0.4_f32, 0.5, 0.6].as_slice()),
            "second embedding should match"
        );
    }

    #[tokio::test]
    async fn embed_batch_ordering() {
        use crate::inference::embedding::EmbeddingProvider;

        let mock_server = MockServer::start().await;

        // Return embeddings out of order
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    { "embedding": [0.4, 0.5, 0.6], "index": 1 },
                    { "embedding": [0.1, 0.2, 0.3], "index": 0 }
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "text-embedding-3-small");
        let response = client.embed(&["first", "second"]).await.unwrap();

        assert_eq!(
            response.embeddings.first().map(Vec::as_slice),
            Some([0.1_f32, 0.2, 0.3].as_slice()),
            "index 0 embedding should be first after sorting"
        );
        assert_eq!(
            response.embeddings.get(1).map(Vec::as_slice),
            Some([0.4_f32, 0.5, 0.6].as_slice()),
            "index 1 embedding should be second after sorting"
        );
    }

    #[tokio::test]
    async fn embed_api_error_401() {
        use crate::inference::embedding::EmbeddingProvider;

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": {
                    "message": "Invalid API key",
                    "type": "invalid_request_error"
                }
            })))
            .mount(&mock_server)
            .await;

        let client =
            make_embedding_client_with_key(mock_server.uri(), "text-embedding-3-small", "bad-key");
        let result = client.embed(&["test"]).await;

        assert!(result.is_err(), "401 should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Api(_)),
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
    async fn embed_empty_data() {
        use crate::inference::embedding::EmbeddingProvider;

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [] })),
            )
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(mock_server.uri(), "text-embedding-3-small");
        let result = client.embed(&["test"]).await;

        assert!(result.is_err(), "empty data should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Parse(_)),
            "should be a Parse error variant"
        );
        assert!(
            err.to_string().contains("no data"),
            "error should mention empty data"
        );
    }

    fn make_fireworks_client(url: impl Into<String>, affinity: Option<&str>) -> OpenAiClient {
        make_client_with_key(url, "accounts/fireworks/models/test", "fw-key").with_dialect(
            OpenAiDialect::Fireworks {
                session_affinity: affinity.map(String::from),
            },
        )
    }

    fn fireworks_usage_body(cached_in_body: Option<u32>) -> serde_json::Value {
        let usage = match cached_in_body {
            Some(cached) => serde_json::json!({
                "prompt_tokens": 1000,
                "completion_tokens": 10,
                "prompt_tokens_details": {"cached_tokens": cached}
            }),
            None => serde_json::json!({"prompt_tokens": 1000, "completion_tokens": 10}),
        };
        serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}}],
            "usage": usage
        })
    }

    #[tokio::test]
    async fn fireworks_sends_session_affinity_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("x-session-affinity", "residuum-main-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fireworks_usage_body(None)))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), Some("residuum-main-abc"));
        client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fireworks_reads_cached_tokens_from_header_when_body_lacks_them() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "768")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            Some(768),
            "header cache count should fill the gap"
        );
    }

    #[tokio::test]
    async fn fireworks_prefers_body_cached_tokens_over_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "1")
                    .set_body_json(fireworks_usage_body(Some(512))),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            Some(512),
            "body cache count is authoritative when present"
        );
    }

    #[tokio::test]
    async fn fireworks_malformed_cache_header_leaves_stats_empty() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "lots")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 1000, "usage still parses");
        assert_eq!(
            usage.cache_read_tokens, None,
            "bad header is not guessed at"
        );
    }

    #[tokio::test]
    async fn openai_dialect_ignores_fireworks_cache_header() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("fireworks-cached-prompt-tokens", "768")
                    .set_body_json(fireworks_usage_body(None)),
            )
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let response = client
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(
            response.usage.and_then(|u| u.cache_read_tokens),
            None,
            "only the fireworks dialect trusts fireworks headers"
        );
    }

    #[tokio::test]
    async fn fireworks_never_sends_hosted_web_search_tool() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fireworks_usage_body(None)))
            .mount(&mock_server)
            .await;

        let client = make_fireworks_client(mock_server.uri(), None);
        let options = CompletionOptions {
            web_search: Some(crate::inference::WebSearchNativeConfig::default()),
            ..CompletionOptions::default()
        };
        client
            .complete(&[Message::user("hi")], &[], &options)
            .await
            .unwrap();

        let requests = mock_server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("tools").is_none(),
            "fireworks rejects web_search_preview, so no tools should be sent: {body}"
        );
    }
}

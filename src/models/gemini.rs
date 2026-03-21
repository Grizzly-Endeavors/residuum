//! Google Gemini API provider implementation.
//!
//! Uses the Gemini `generateContent` REST API. Authentication is via an API
//! key passed as a query parameter. System messages are extracted and sent
//! as the top-level `systemInstruction` field. Tool results are sent as
//! `functionResponse` parts in user-role messages.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::embedding::{EmbeddingProvider, EmbeddingResponse};
use super::http::{SharedHttpClient, map_request_error, warn_if_insecure_remote};
use super::retry::{RetryConfig, with_retry};
use super::{
    CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ResponseFormat, Role,
    ThinkingConfig, ThinkingLevel, ToolCall, ToolDefinition, Usage,
};

/// Client for the Google Gemini `generateContent` API.
pub(crate) struct GeminiClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
    retry: RetryConfig,
}

impl GeminiClient {
    /// Create a new Gemini client with a shared HTTP client.
    ///
    /// # Arguments
    /// * `http` - Shared HTTP client for connection pooling
    /// * `base_url` - API base URL (e.g. `https://generativelanguage.googleapis.com/v1beta`)
    /// * `api_key` - Google AI API key
    /// * `model` - Model identifier (e.g. `gemini-2.0-flash`)
    /// * `max_tokens` - Maximum output tokens for completions
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

    /// Parse a successful Gemini response into our generic `ModelResponse`.
    fn parse_response(gemini_response: GeminiResponse) -> Result<ModelResponse, ModelError> {
        let candidate = gemini_response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| {
                ModelError::Parse("Gemini API response contained no candidates".to_string())
            })?;

        let mut content_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for (idx, part) in candidate.content.parts.into_iter().enumerate() {
            match part {
                GeminiPart::Text { text } => {
                    content_text.push_str(&text);
                }
                GeminiPart::FunctionCall { function_call } => {
                    // Gemini does not return IDs for function calls; synthesize them.
                    tool_calls.push(ToolCall {
                        id: format!("call_{idx}"),
                        name: function_call.name,
                        arguments: function_call.args,
                    });
                }
                GeminiPart::FunctionResponse { .. } => {
                    tracing::warn!(
                        part_index = idx,
                        "unexpected functionResponse part in Gemini model output"
                    );
                }
                GeminiPart::InlineData { .. } => {
                    tracing::warn!(
                        part_index = idx,
                        "unexpected inlineData part in Gemini model output"
                    );
                }
            }
        }

        let usage = gemini_response.usage_metadata.map(|u| Usage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            cache_creation_tokens: None,
            cache_read_tokens: u.cached_content_token_count,
        });

        let mut model_response = ModelResponse::new(content_text, tool_calls);
        model_response.usage = usage;
        Ok(model_response)
    }

    /// Build the full endpoint URL with the API key query parameter.
    fn endpoint(&self) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        )
    }

    /// Convert generic messages into Gemini API format.
    ///
    /// System messages are extracted and returned separately; Gemini uses a
    /// top-level `systemInstruction` field rather than including system content
    /// in the `contents` array. Multiple system messages are concatenated.
    ///
    /// Tool result messages (`Role::Tool`) become user-role messages containing
    /// `functionResponse` parts, as required by the Gemini API.
    fn convert_messages(
        messages: &[Message],
    ) -> (Option<GeminiSystemInstruction>, Vec<GeminiContent>) {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_parts.push(&msg.content);
                }
                Role::User => {
                    let has_images = msg.images.as_ref().is_some_and(|imgs| !imgs.is_empty());

                    if has_images {
                        let mut parts: Vec<GeminiPart> = Vec::new();

                        if !msg.content.is_empty() {
                            parts.push(GeminiPart::Text {
                                text: msg.content.clone(),
                            });
                        }

                        for img in msg.images.as_ref().unwrap_or(&Vec::new()) {
                            parts.push(GeminiPart::InlineData {
                                inline_data: GeminiInlineData {
                                    mime_type: img.media_type.clone(),
                                    data: img.data.clone(),
                                },
                            });
                        }

                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts,
                        });
                    } else {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![GeminiPart::Text {
                                text: msg.content.clone(),
                            }],
                        });
                    }
                }
                Role::Assistant => {
                    let mut parts: Vec<GeminiPart> = Vec::new();

                    if !msg.content.is_empty() {
                        parts.push(GeminiPart::Text {
                            text: msg.content.clone(),
                        });
                    }

                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            parts.push(GeminiPart::FunctionCall {
                                function_call: GeminiFunctionCall {
                                    name: tc.name.clone(),
                                    args: tc.arguments.clone(),
                                },
                            });
                        }
                    }

                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts,
                    });
                }
                Role::Tool => {
                    // Gemini expects functionResponse in a user-role message.
                    // The response must be a JSON object; wrap plain strings.
                    let response_value = serde_json::json!({ "result": msg.content });
                    let tool_name = msg.tool_call_id.as_deref().unwrap_or("unknown");

                    contents.push(GeminiContent {
                        role: "user".to_string(),
                        parts: vec![GeminiPart::FunctionResponse {
                            function_response: GeminiFunctionResponse {
                                name: tool_name.to_string(),
                                response: response_value,
                            },
                        }],
                    });
                }
            }
        }

        let system_instruction = (!system_parts.is_empty()).then(|| GeminiSystemInstruction {
            parts: vec![GeminiPart::Text {
                text: system_parts.join("\n\n"),
            }],
        });

        (system_instruction, contents)
    }
}

#[async_trait]
impl ModelProvider for GeminiClient {
    #[expect(
        clippy::too_many_lines,
        reason = "logging audit requires inline request/response context"
    )]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        let url = self.endpoint();
        let (system_instruction, contents) = Self::convert_messages(messages);
        let has_web_search = options.web_search.is_some();
        let gemini_tools = (!tools.is_empty() || has_web_search).then(|| {
            let function_declarations = (!tools.is_empty()).then(|| {
                tools
                    .iter()
                    .map(|t| GeminiFunctionDeclaration {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
                    .collect()
            });
            let google_search = has_web_search.then(|| serde_json::json!({}));
            vec![GeminiTools {
                function_declarations,
                google_search,
            }]
        });
        let max_output_tokens = options.max_tokens.unwrap_or(self.max_tokens);
        let message_count = messages.len();
        let tool_count = tools.len();
        let model = self.model.clone();
        let http = self.http.clone();
        let timeout_secs = self.http.timeout_secs();

        let (response_mime_type, response_schema) = match &options.response_format {
            ResponseFormat::Text => (None, None),
            ResponseFormat::JsonSchema { schema, .. } => (
                Some("application/json".to_string()),
                Some(strip_unsupported_schema_fields(schema.clone())),
            ),
        };
        let generation_config = GeminiGenerationConfig {
            max_output_tokens,
            response_mime_type,
            response_schema,
            temperature: options.temperature,
        };

        let thinking_config = options.thinking.as_ref().and_then(|tc| match tc {
            ThinkingConfig::Level(ThinkingLevel::Low) => Some(GeminiThinkingConfig {
                thinking_budget: 1024,
            }),
            ThinkingConfig::Level(ThinkingLevel::Medium) => Some(GeminiThinkingConfig {
                thinking_budget: 8192,
            }),
            ThinkingConfig::Level(ThinkingLevel::High) => Some(GeminiThinkingConfig {
                thinking_budget: 32768,
            }),
            ThinkingConfig::Toggle(true) => Some(GeminiThinkingConfig {
                thinking_budget: -1,
            }),
            ThinkingConfig::Toggle(false) => None,
        });

        with_retry(&self.retry, || {
            let url = url.clone();
            let system_instruction = system_instruction.clone();
            let contents = contents.clone();
            let gemini_tools = gemini_tools.clone();
            let generation_config = generation_config.clone();
            let thinking_config = thinking_config.clone();
            let model = model.clone();
            let http = http.clone();

            async move {
                let request = GeminiRequest {
                    contents,
                    system_instruction,
                    tools: gemini_tools,
                    generation_config,
                    thinking_config,
                };

                let request_json = serde_json::to_string(&request)
                    .unwrap_or_else(|e| format!("(serialization failed: {e})"));

                debug!(
                    model = %model,
                    max_output_tokens,
                    message_count,
                    tool_count,
                    "sending gemini generateContent request"
                );

                let response = http
                    .client()
                    .post(&url)
                    .body(request_json.clone())
                    .header("content-type", "application/json")
                    .send()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let raw_body = match response.text().await {
                        Ok(body) => body,
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to read error response body");
                            format!("failed to read response body: {e}")
                        }
                    };
                    tracing::warn!(
                        status = %status,
                        response_body = %raw_body,
                        request_body = %request_json,
                        "gemini API error — full request/response for diagnosis"
                    );
                    let error_body = serde_json::from_str::<GeminiErrorResponse>(&raw_body)
                        .map_or_else(|_| raw_body, |e| e.error.message);
                    return Err(ModelError::Api(format!("{status}: {error_body}")));
                }

                let text = response
                    .text()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;
                let result = Self::parse_response(serde_json::from_str(&text).map_err(|e| {
                    ModelError::Parse(format!("failed to parse gemini response: {e}"))
                })?)?;
                info!(
                    model = %model,
                    content_len = result.content.len(),
                    tool_calls = result.tool_calls.len(),
                    "gemini completion received"
                );
                Ok(result)
            }
        })
        .await
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// ---------------------------------------------------------------------------
// Gemini API request types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct GeminiThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    thinking_budget: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTools>>,
    generation_config: GeminiGenerationConfig,
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Serialize, Clone)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Clone)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: GeminiInlineData,
    },
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiInlineData {
    mime_type: String,
    data: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct GeminiTools {
    #[serde(
        rename = "functionDeclarations",
        skip_serializing_if = "Option::is_none"
    )]
    function_declarations: Option<Vec<GeminiFunctionDeclaration>>,
    #[serde(rename = "googleSearch", skip_serializing_if = "Option::is_none")]
    google_search: Option<serde_json::Value>,
}

#[derive(Serialize, Clone)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Recursively remove fields that Gemini's `responseSchema` doesn't support
/// (e.g. `additionalProperties`).
fn strip_unsupported_schema_fields(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("additionalProperties");
        for child in obj.values_mut() {
            *child = strip_unsupported_schema_fields(child.take());
        }
    } else if let Some(arr) = value.as_array_mut() {
        for item in arr.iter_mut() {
            *item = strip_unsupported_schema_fields(item.take());
        }
    }
    value
}

#[derive(Serialize, Clone)]
struct GeminiGenerationConfig {
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    #[serde(rename = "responseMimeType", skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<String>,
    #[serde(rename = "responseSchema", skip_serializing_if = "Option::is_none")]
    response_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

// ---------------------------------------------------------------------------
// Gemini API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiResponseContent,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
#[expect(clippy::struct_field_names, reason = "field names match Gemini API")]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: u32,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: u32,
    #[serde(default, rename = "cachedContentTokenCount")]
    cached_content_token_count: Option<u32>,
}

#[derive(Deserialize)]
struct GeminiErrorResponse {
    error: GeminiError,
}

#[derive(Deserialize)]
struct GeminiError {
    message: String,
}

// ---------------------------------------------------------------------------
// Gemini embedding request types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct GeminiEmbedContentRequest {
    model: String,
    content: GeminiEmbedContent,
}

#[derive(Serialize, Clone)]
struct GeminiEmbedContent {
    parts: Vec<GeminiEmbedPart>,
}

#[derive(Serialize, Clone)]
struct GeminiEmbedPart {
    text: String,
}

#[derive(Serialize, Clone)]
struct GeminiBatchEmbedRequest {
    requests: Vec<GeminiEmbedContentRequest>,
}

// ---------------------------------------------------------------------------
// Gemini embedding response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GeminiEmbedContentResponse {
    embedding: GeminiEmbeddingValues,
}

#[derive(Deserialize)]
struct GeminiEmbeddingValues {
    values: Vec<f32>,
}

#[derive(Deserialize)]
struct GeminiBatchEmbedResponse {
    embeddings: Vec<GeminiEmbeddingValues>,
}

// ---------------------------------------------------------------------------
// Gemini embedding client
// ---------------------------------------------------------------------------

/// Google Gemini embeddings API client.
pub(crate) struct GeminiEmbeddingClient {
    http: SharedHttpClient,
    base_url: String,
    api_key: String,
    model: String,
    retry: RetryConfig,
}

impl GeminiEmbeddingClient {
    #[must_use]
    pub fn new(
        http: SharedHttpClient,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        retry: RetryConfig,
    ) -> Self {
        let base_url = base_url.into();
        warn_if_insecure_remote(&base_url);
        Self {
            http,
            base_url,
            api_key: api_key.into(),
            model: model.into(),
            retry,
        }
    }

    async fn embed_single(&self, text: String) -> Result<EmbeddingResponse, ModelError> {
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let http = self.http.clone();
        let timeout_secs = self.http.timeout_secs();
        with_retry(&self.retry, || {
            let url = format!("{base_url}/models/{model}:embedContent?key={api_key}");
            let request_body = GeminiEmbedContentRequest {
                model: format!("models/{model}"),
                content: GeminiEmbedContent {
                    parts: vec![GeminiEmbedPart { text: text.clone() }],
                },
            };
            let http = http.clone();
            let model = model.clone();

            async move {
                debug!(model = %model, "sending gemini embed request");

                let response = http
                    .client()
                    .post(&url)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;

                if !response.status().is_success() {
                    return Err(parse_gemini_embed_error(response).await);
                }

                let resp_body = response
                    .text()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;
                let parsed: GeminiEmbedContentResponse =
                    serde_json::from_str(&resp_body).map_err(|e| {
                        ModelError::Parse(format!("failed to parse gemini embed response: {e}"))
                    })?;
                let dimensions = parsed.embedding.values.len();
                info!(model = %model, dimensions, "gemini embedding received");
                Ok(EmbeddingResponse {
                    embeddings: vec![parsed.embedding.values],
                    dimensions,
                })
            }
        })
        .await
    }

    async fn embed_batch(&self, owned_texts: Vec<String>) -> Result<EmbeddingResponse, ModelError> {
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let http = self.http.clone();
        let timeout_secs = self.http.timeout_secs();
        with_retry(&self.retry, || {
            let url = format!("{base_url}/models/{model}:batchEmbedContents?key={api_key}");
            let requests: Vec<GeminiEmbedContentRequest> = owned_texts
                .iter()
                .map(|t| GeminiEmbedContentRequest {
                    model: format!("models/{model}"),
                    content: GeminiEmbedContent {
                        parts: vec![GeminiEmbedPart { text: t.clone() }],
                    },
                })
                .collect();
            let request_body = GeminiBatchEmbedRequest { requests };
            let http = http.clone();
            let model = model.clone();
            let batch_count = owned_texts.len();

            async move {
                debug!(model = %model, count = batch_count, "sending gemini batch embed request");

                let response = http
                    .client()
                    .post(&url)
                    .json(&request_body)
                    .send()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;

                if !response.status().is_success() {
                    return Err(parse_gemini_embed_error(response).await);
                }

                let resp_body = response
                    .text()
                    .await
                    .map_err(|e| map_request_error(e, timeout_secs))?;
                let parsed: GeminiBatchEmbedResponse =
                    serde_json::from_str(&resp_body).map_err(|e| {
                        ModelError::Parse(format!(
                            "failed to parse gemini batch embed response: {e}"
                        ))
                    })?;
                let dimensions = parsed.embeddings.first().map_or(0, |e| e.values.len());
                let embeddings: Vec<Vec<f32>> =
                    parsed.embeddings.into_iter().map(|e| e.values).collect();
                info!(model = %model, count = embeddings.len(), dimensions, "gemini batch embeddings received");
                Ok(EmbeddingResponse {
                    embeddings,
                    dimensions,
                })
            }
        })
        .await
    }
}

#[async_trait]
impl EmbeddingProvider for GeminiEmbeddingClient {
    async fn embed(&self, texts: &[&str]) -> Result<EmbeddingResponse, ModelError> {
        if texts.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: Vec::new(),
                dimensions: 0,
            });
        }

        if texts.len() == 1 {
            // Safe: we checked `texts.is_empty()` above
            let text = texts.first().map(|t| (*t).to_string()).unwrap_or_default();
            self.embed_single(text).await
        } else {
            let owned_texts: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();
            self.embed_batch(owned_texts).await
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Parse a Gemini error response into a `ModelError::Api`.
async fn parse_gemini_embed_error(response: reqwest::Response) -> ModelError {
    let status = response.status();
    let raw_body = match response.text().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read error response body");
            format!("failed to read response body: {e}")
        }
    };
    let error_body = serde_json::from_str::<GeminiErrorResponse>(&raw_body)
        .map_or_else(|_| raw_body, |e| e.error.message);
    ModelError::Api(format!("{status}: {error_body}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::CompletionOptions;
    use crate::models::retry::RetryConfig;
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(base_url: &str) -> GeminiClient {
        let http =
            SharedHttpClient::new(&super::super::http::HttpClientConfig::with_timeout(60)).unwrap();
        GeminiClient::new(
            http,
            base_url,
            "test-api-key",
            "gemini-2.0-flash",
            8192,
            RetryConfig::no_retry(),
        )
    }

    #[test]
    fn endpoint_includes_model_and_key() {
        let http =
            SharedHttpClient::new(&super::super::http::HttpClientConfig::with_timeout(60)).unwrap();
        let client = GeminiClient::new(
            http,
            "https://generativelanguage.googleapis.com/v1beta",
            "my-key",
            "gemini-2.0-flash",
            8192,
            RetryConfig::no_retry(),
        );
        let ep = client.endpoint();
        assert!(
            ep.contains("/models/gemini-2.0-flash:generateContent"),
            "endpoint should include model path"
        );
        assert!(ep.contains("key=my-key"), "endpoint should include API key");
    }

    #[test]
    fn convert_messages_extracts_system() {
        let messages = vec![Message::system("You are helpful."), Message::user("Hello")];
        let (system, contents) = GeminiClient::convert_messages(&messages);

        assert!(system.is_some(), "system instruction should be extracted");
        let sys = system.unwrap();
        assert_eq!(sys.parts.len(), 1, "should have one system part");
        let first_part = sys.parts.first().unwrap();
        assert!(
            matches!(first_part, GeminiPart::Text { .. }),
            "system part should be text"
        );
        if let GeminiPart::Text { text } = first_part {
            assert_eq!(text, "You are helpful.", "system text should match");
        }

        assert_eq!(contents.len(), 1, "only user message in contents");
        assert_eq!(
            contents.first().map(|c| c.role.as_str()),
            Some("user"),
            "content role should be user"
        );
    }

    #[test]
    fn convert_messages_tool_result_becomes_function_response() {
        let messages = vec![Message::tool("command output", "call_0")];
        let (_, contents) = GeminiClient::convert_messages(&messages);

        assert_eq!(contents.len(), 1, "tool message becomes one content entry");
        let entry = contents.first().unwrap();
        assert_eq!(entry.role, "user", "tool result role should be user");
        assert_eq!(entry.parts.len(), 1, "should have one part");
        let part = entry.parts.first().unwrap();
        assert!(
            matches!(part, GeminiPart::FunctionResponse { .. }),
            "part should be functionResponse"
        );
        if let GeminiPart::FunctionResponse { function_response } = part {
            assert_eq!(
                function_response.response,
                serde_json::json!({"result": "command output"}),
                "response should wrap content"
            );
        }
    }

    #[test]
    fn convert_messages_assistant_with_tool_calls() {
        let messages = vec![Message::assistant(
            "thinking",
            Some(vec![ToolCall {
                id: "call_0".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }]),
        )];
        let (_, contents) = GeminiClient::convert_messages(&messages);

        let entry = contents.first().unwrap();
        assert_eq!(entry.role, "model", "assistant maps to model role");
        assert_eq!(entry.parts.len(), 2, "text + function call parts");
        assert!(
            matches!(entry.parts.first(), Some(GeminiPart::Text { .. })),
            "first part should be text"
        );
        assert!(
            matches!(entry.parts.get(1), Some(GeminiPart::FunctionCall { .. })),
            "second part should be function call"
        );
    }

    #[test]
    fn model_name_returns_model() {
        let client = make_client("http://localhost");
        assert_eq!(client.model_name(), "gemini-2.0-flash");
    }

    #[tokio::test]
    async fn complete_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .and(query_param("key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "Hello there!"}]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 5,
                    "totalTokenCount": 15
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let response = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(response.content, "Hello there!", "content should match");
        assert!(response.tool_calls.is_empty(), "should have no tool calls");
        assert!(response.usage.is_some(), "should report usage");
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 10, "input tokens should match");
        assert_eq!(usage.output_tokens, 5, "output tokens should match");
        assert!(response.is_complete(), "text-only response is complete");
    }

    #[tokio::test]
    async fn complete_with_tool_calls() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "name": "bash",
                                "args": {"command": "ls -la"}
                            }
                        }]
                    },
                    "finishReason": "TOOL_CODE"
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let response = client
            .complete(
                &[Message::user("List files")],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        assert!(response.content.is_empty(), "no text in tool-only response");
        assert_eq!(response.tool_calls.len(), 1, "should have one tool call");
        let tc = response.tool_calls.first().unwrap();
        assert_eq!(tc.name, "bash", "tool name should match");
        assert_eq!(
            tc.arguments,
            serde_json::json!({"command": "ls -la"}),
            "arguments should be native JSON"
        );
        assert_eq!(tc.id, "call_0", "should have synthetic ID");
        assert!(
            !response.is_complete(),
            "response with tool calls is not complete"
        );
    }

    #[tokio::test]
    async fn api_error_returned_as_model_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {
                    "code": 400,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "API error should return Err");
        let err = result.unwrap_err();
        assert!(matches!(err, ModelError::Api(_)), "should be an Api error");
        assert!(
            err.to_string().contains("400"),
            "error should contain status code"
        );
        assert!(
            err.to_string().contains("API key not valid"),
            "error should contain Gemini message"
        );
    }

    #[tokio::test]
    async fn empty_candidates_returns_parse_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": []
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "empty candidates should return error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ModelError::Parse(_)),
            "should be a Parse error"
        );
        assert!(
            err.to_string().contains("no candidates"),
            "error should mention missing candidates"
        );
    }

    #[tokio::test]
    async fn complete_timeout() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(3)))
            .mount(&mock_server)
            .await;

        let http =
            SharedHttpClient::new(&super::super::http::HttpClientConfig::with_timeout(1)).unwrap();
        let client = GeminiClient::new(
            http,
            mock_server.uri(),
            "test-api-key",
            "gemini-2.0-flash",
            8192,
            RetryConfig::no_retry(),
        );
        let result = client
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "timeout should return error");
        assert!(
            matches!(result.unwrap_err(), ModelError::Timeout(1)),
            "should be Timeout(1)"
        );
    }

    #[tokio::test]
    async fn complete_with_json_schema_response_format() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .and(query_param("key", "test-api-key"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "generationConfig": {
                    "responseMimeType": "application/json",
                    "responseSchema": {
                        "type": "object",
                        "properties": {
                            "answer": {"type": "string"}
                        }
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "{\"answer\": \"hello\"}"}]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 5,
                    "totalTokenCount": 15
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
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
    async fn temperature_included_in_generation_config() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "generationConfig": {
                    "temperature": 0.9
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "ok"}]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 5,
                    "candidatesTokenCount": 1,
                    "totalTokenCount": 6
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let options = CompletionOptions {
            temperature: Some(0.9),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(result.is_ok(), "request with temperature should succeed");
    }

    #[tokio::test]
    async fn cache_tokens_parsed_from_usage_metadata() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "cached response"}]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 5,
                    "totalTokenCount": 15,
                    "cachedContentTokenCount": 8
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let result = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await;
        assert!(result.is_ok(), "cache token response should succeed");

        let usage = result.unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 10, "input tokens should match");
        assert_eq!(usage.output_tokens, 5, "output tokens should match");
        assert_eq!(
            usage.cache_creation_tokens, None,
            "Gemini does not report cache creation tokens"
        );
        assert_eq!(
            usage.cache_read_tokens,
            Some(8),
            "cache read tokens should match cachedContentTokenCount"
        );
    }

    #[tokio::test]
    async fn thinking_config_included_when_set() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex("/models/.+:generateContent"))
            .and(query_param("key", "test-api-key"))
            .and(wiremock::matchers::body_partial_json(serde_json::json!({
                "thinkingConfig": {
                    "thinkingBudget": 8192
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "ok"}],
                        "role": "model"
                    }
                }]
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = make_client(&mock_server.uri());
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
            ..CompletionOptions::default()
        };
        let result = client
            .complete(&[Message::user("Hello")], &[], &options)
            .await;
        assert!(
            result.is_ok(),
            "request with thinking config should succeed: {result:?}"
        );
    }

    // --- Embedding tests ---

    use crate::models::embedding::EmbeddingProvider;

    fn make_embedding_client(base_url: &str) -> GeminiEmbeddingClient {
        let http =
            SharedHttpClient::new(&super::super::http::HttpClientConfig::with_timeout(60)).unwrap();
        GeminiEmbeddingClient::new(
            http,
            base_url,
            "test-api-key",
            "text-embedding-004",
            RetryConfig::no_retry(),
        )
    }

    #[tokio::test]
    async fn embed_single_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex("/models/text-embedding-004:embedContent"))
            .and(query_param("key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embedding": {
                    "values": [0.1, 0.2, 0.3]
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(&mock_server.uri());
        let response = client.embed(&["hello world"]).await.unwrap();

        assert_eq!(response.embeddings.len(), 1, "should have one embedding");
        assert_eq!(response.dimensions, 3, "should have 3 dimensions");
        assert_eq!(
            response.embeddings.first().map(Vec::as_slice),
            Some([0.1_f32, 0.2, 0.3].as_slice()),
            "embedding values should match"
        );
    }

    #[tokio::test]
    async fn embed_batch_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex("/models/text-embedding-004:batchEmbedContents"))
            .and(query_param("key", "test-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [
                    { "values": [0.1, 0.2, 0.3] },
                    { "values": [0.4, 0.5, 0.6] }
                ]
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(&mock_server.uri());
        let response = client.embed(&["hello", "world"]).await.unwrap();

        assert_eq!(response.embeddings.len(), 2, "should have two embeddings");
        assert_eq!(response.dimensions, 3, "should have 3 dimensions");
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
    async fn embed_api_key_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex("/models/text-embedding-004:embedContent"))
            .and(query_param("key", "test-api-key"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {
                    "code": 400,
                    "message": "API key not valid. Please pass a valid API key.",
                    "status": "INVALID_ARGUMENT"
                }
            })))
            .mount(&mock_server)
            .await;

        let client = make_embedding_client(&mock_server.uri());
        let result = client.embed(&["hello"]).await;

        assert!(result.is_err(), "API error should return Err");
        let err = result.unwrap_err();
        assert!(matches!(err, ModelError::Api(_)), "should be an Api error");
        assert!(
            err.to_string().contains("400"),
            "error should contain status code"
        );
        assert!(
            err.to_string().contains("API key not valid"),
            "error should contain Gemini message"
        );
    }
}

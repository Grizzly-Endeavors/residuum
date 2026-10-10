//! Client for OpenAI-compatible chat completion APIs.
//!
//! Supports various providers including Azure, vLLM, LM Studio, Fireworks, and
//! other compatible endpoints. Host-specific behavior on top of the shared wire
//! format is selected with [`OpenAiDialect`].

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::inference::embedding::{EmbeddingProvider, EmbeddingResponse};
use crate::inference::http::{
    SharedHttpClient, map_request_error, map_stream_request_error, read_error_body,
    warn_if_insecure_remote,
};
use crate::inference::reply::{ReplyAssembler, plain_reasoning};
use crate::inference::retry::{RetryConfig, with_retry};
use crate::inference::stream::{Flow, SseEvent, TrackedSink, answered_whole, read_sse};
use crate::inference::types::current_exchange_start;
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message, ProviderApi,
    ResponseFormat, StopReason, StreamSink, ThinkingConfig, ThinkingLevel, ThinkingOrigin,
    ToolCall, ToolDefinition, Usage,
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

    /// Whether this host wants the reasoning it returned sent back with the
    /// tool calls it led to, and understands the `reasoning_content` field
    /// that carries it. DeepSeek-style hosts reject a tool-use exchange
    /// whose reasoning is missing; `OpenAI` itself returns no reasoning text
    /// and rejects fields it does not know, so it gets none.
    fn replays_reasoning(&self) -> bool {
        match &self.dialect {
            OpenAiDialect::Fireworks { .. } => true,
            OpenAiDialect::OpenAi => url::Url::parse(&self.base_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_lowercase))
                .is_some_and(|host| !is_official_openai_host(&host)),
        }
    }

    /// Who the reasoning this client returns is recorded as coming from, and
    /// the only provider API whose reasoning it sends back.
    fn origin(&self) -> ThinkingOrigin {
        ThinkingOrigin::new(ProviderApi::OpenAiCompatible, &self.model)
    }

    /// Convert a conversation to the wire format, replaying the reasoning an
    /// OpenAI-compatible API returned for the assistant messages of the
    /// tool-use exchange in progress, on a host that wants it.
    fn convert_messages(&self, messages: &[Message]) -> Vec<OpenAiMessage> {
        let replay_from = self
            .replays_reasoning()
            .then(|| current_exchange_start(messages));
        let origin = self.origin();
        messages
            .iter()
            .enumerate()
            .map(|(index, msg)| {
                let mut converted = OpenAiMessage::from(msg);
                if replay_from.is_some_and(|start| index >= start) {
                    converted.reasoning_content = plain_reasoning(&msg.thinking, &origin);
                }
                converted
            })
            .collect()
    }

    /// Build everything about a request that stays the same across retries.
    fn prepare(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> PreparedRequest {
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

        PreparedRequest {
            messages: self.convert_messages(messages),
            tools: (!openai_tools.is_empty()).then_some(openai_tools),
            response_format,
            temperature: options.temperature,
            reasoning_effort: options
                .thinking
                .as_ref()
                .and_then(Self::build_reasoning_effort),
        }
    }

    /// Run a prepared request to completion: retrying transient failures,
    /// and streaming the response into `sink` when there is one.
    async fn run(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&dyn StreamSink>,
    ) -> Result<InferenceResponse, InferenceError> {
        let prepared = self.prepare(messages, tools, options);
        let tracked = sink.map(TrackedSink::new);

        let response = with_retry(&self.retry, || async {
            let request = prepared.request(&self.model, tracked.is_some());
            let result = self.send(&request, tracked.as_ref()).await;
            if result.is_err()
                && let Some(tracked) = &tracked
            {
                tracked.restart_if_needed();
            }
            result
        })
        .await?;
        Ok(response.produced_at(&self.origin()))
    }

    /// Send a pre-built request to the OpenAI-compatible API and parse the response.
    #[tracing::instrument(skip_all, fields(
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.map_or(0, <[OpenAiToolEntry]>::len),
        streaming = sink.is_some(),
    ))]
    async fn send(
        &self,
        request: &ChatCompletionRequest<'_>,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        let timeout_secs = self.http.timeout_secs();
        let request_json = serde_json::to_string(request)
            .map_err(|e| InferenceError::Parse(format!("failed to serialize request: {e}")))?;

        debug!(model = %request.model, "sending openai completion request");

        let client = if sink.is_some() {
            self.http.streaming_client()
        } else {
            self.http.client()
        };
        let mut req_builder = client
            .post(format!("{}/chat/completions", self.base_url))
            .body(request_json.clone())
            .header("content-type", "application/json");

        if let Some(key) = &self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        if let OpenAiDialect::Fireworks {
            session_affinity: Some(affinity),
        } = &self.dialect
        {
            req_builder = req_builder.header(FIREWORKS_SESSION_AFFINITY_HEADER, affinity);
        }

        let response = req_builder.send().await.map_err(|e| {
            if sink.is_some() {
                map_stream_request_error(e, timeout_secs)
            } else {
                map_request_error(e, timeout_secs)
            }
        })?;

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

        let header_cached_tokens = match &self.dialect {
            OpenAiDialect::Fireworks { .. } => fireworks_cached_prompt_tokens(response.headers()),
            OpenAiDialect::OpenAi => None,
        };

        let resp = if let Some(sink) = sink.filter(|_| !answered_whole(&response)) {
            read_stream(response, timeout_secs, sink, header_cached_tokens).await?
        } else {
            let body = response
                .text()
                .await
                .map_err(|e| map_request_error(e, timeout_secs))?;
            let chat_response: ChatCompletionResponse =
                serde_json::from_str(&body).map_err(|e| {
                    InferenceError::Parse(format!("failed to parse openai response: {e}"))
                })?;
            parse_response(chat_response, header_cached_tokens)?
        };
        info!(
            model = %request.model,
            content_len = resp.content.len(),
            tool_calls = resp.tool_calls.len(),
            "openai completion received"
        );
        Ok(resp)
    }
}

/// Turn a completed (or fully streamed) response into an `InferenceResponse`.
fn parse_response(
    chat_response: ChatCompletionResponse,
    header_cached_tokens: Option<u32>,
) -> Result<InferenceResponse, InferenceError> {
    let usage = chat_response
        .usage
        .map(|u| usage_from(u, header_cached_tokens));

    let choice = chat_response.choices.into_iter().next().ok_or_else(|| {
        InferenceError::Parse("OpenAI API response contained no choices in response".to_string())
    })?;

    let mut reply = ReplyAssembler::new(None);
    reply.reasoning(&choice.message.reasoning.text());
    // OpenAI uses null for content when tool_calls are present
    reply.content(choice.message.content.as_deref().unwrap_or_default());
    let (content, thinking) = reply.finish();

    let raw_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| RawToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments: tc.function.arguments,
        })
        .collect();
    let truncated = choice.finish_reason.as_deref() == Some("length");

    let mut resp = InferenceResponse::new(content, build_tool_calls(raw_calls, truncated)?);
    resp.usage = usage;
    resp.thinking = thinking;
    resp.stop_reason = choice.finish_reason.as_deref().map(map_stop_reason);
    Ok(resp)
}

fn usage_from(usage: OpenAiUsage, header_cached_tokens: Option<u32>) -> Usage {
    Usage {
        input_tokens: usage.prompt_tokens.unwrap_or(0),
        output_tokens: usage.completion_tokens.unwrap_or(0),
        cache_creation_tokens: None,
        cache_read_tokens: usage
            .prompt_tokens_details
            .and_then(|d| d.cached_tokens)
            .or(header_cached_tokens),
    }
}

/// A tool call as the API returns it, before its arguments are parsed.
struct RawToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Parse tool calls' arguments, naming a call that has no id after its
/// position.
///
/// A response cut off by the output limit ends mid-way through its last
/// tool call's arguments; that call is dropped, and the stop reason tells the
/// caller why.
fn build_tool_calls(
    raw_calls: Vec<RawToolCall>,
    truncated: bool,
) -> Result<Vec<ToolCall>, InferenceError> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    for (index, raw) in raw_calls.into_iter().enumerate() {
        // Some servers send no arguments at all for a call that takes none.
        let arguments = if raw.arguments.trim().is_empty() {
            Ok(serde_json::json!({}))
        } else {
            serde_json::from_str(&raw.arguments)
        };
        match arguments {
            Ok(arguments) => calls.push(ToolCall {
                id: if raw.id.is_empty() {
                    format!("call_{index}")
                } else {
                    raw.id
                },
                name: raw.name,
                arguments,
                server: None,
            }),
            Err(e) if truncated => {
                tracing::warn!(
                    tool = %raw.name,
                    error = %e,
                    "dropping a tool call whose arguments were cut off by the output limit"
                );
            }
            Err(e) => {
                return Err(InferenceError::Parse(format!(
                    "failed to parse tool arguments for '{}': {e} (raw: {})",
                    raw.name, raw.arguments
                )));
            }
        }
    }
    Ok(calls)
}

/// Whether `host` is `OpenAI`'s own service, which accepts only the fields
/// its API documents.
fn is_official_openai_host(host: &str) -> bool {
    host == "api.openai.com" || host.ends_with(".openai.com") || host.ends_with(".openai.azure.com")
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
        self.run(messages, tools, options, None).await
    }

    /// Like `complete`, streaming the response's text and reasoning into
    /// `sink` as they arrive.
    ///
    /// # Errors
    /// Returns the errors `complete` does, plus `InferenceError::Stalled`
    /// when the stream goes quiet for the configured timeout and
    /// `InferenceError::StreamInterrupted` when it breaks or ends early.
    #[tracing::instrument(skip_all, fields(model = %self.model, message_count = messages.len(), tool_count = tools.len()))]
    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        self.run(messages, tools, options, Some(sink)).await
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

// --- Streaming ---

/// Read a streamed chat completion to its end, pushing text and reasoning to
/// `sink` as they arrive, and return it as the response a non-streaming
/// request would have returned.
async fn read_stream(
    response: reqwest::Response,
    idle_secs: u64,
    sink: &dyn StreamSink,
    header_cached_tokens: Option<u32>,
) -> Result<InferenceResponse, InferenceError> {
    let mut assembler = StreamAssembler::new(sink);
    read_sse(response, idle_secs, |event| assembler.handle(&event)).await?;
    assembler.finish(header_cached_tokens)
}

struct StreamAssembler<'a> {
    reply: ReplyAssembler<'a>,
    tool_calls: BTreeMap<u32, RawToolCall>,
    usage: Option<OpenAiUsage>,
    finish_reason: Option<String>,
    /// The `[DONE]` marker arrived.
    done: bool,
}

impl<'a> StreamAssembler<'a> {
    fn new(sink: &'a dyn StreamSink) -> Self {
        Self {
            reply: ReplyAssembler::new(Some(sink)),
            tool_calls: BTreeMap::new(),
            usage: None,
            finish_reason: None,
            done: false,
        }
    }

    fn handle(&mut self, event: &SseEvent) -> Result<Flow, InferenceError> {
        if event.data.trim() == "[DONE]" {
            self.done = true;
            return Ok(Flow::Done);
        }
        let chunk: StreamChunk = serde_json::from_str(&event.data).map_err(|e| {
            InferenceError::Parse(format!("failed to parse openai stream chunk: {e}"))
        })?;
        if let Some(error) = chunk.error {
            let kind = error.r#type.filter(|k| !k.is_empty());
            return Err(InferenceError::Api(match kind {
                Some(kind) => format!("openai stream error ({kind}): {}", error.message),
                None => format!("openai stream error: {}", error.message),
            }));
        }
        if let Some(usage) = chunk.usage {
            self.usage = Some(usage);
        }
        // Only the first choice is used: a request never asks for more.
        if let Some(choice) = chunk.choices.into_iter().next() {
            self.apply_delta(choice.delta);
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason;
            }
        }
        Ok(Flow::Continue)
    }

    fn apply_delta(&mut self, delta: ChunkDelta) {
        self.reply.reasoning(&delta.reasoning.text());
        if let Some(content) = delta.content {
            self.reply.content(&content);
        }
        for fragment in delta.tool_calls.unwrap_or_default() {
            let call = self
                .tool_calls
                .entry(fragment.index)
                .or_insert_with(|| RawToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
            if let Some(id) = fragment.id.filter(|id| !id.is_empty())
                && call.id.is_empty()
            {
                call.id = id;
            }
            if let Some(function) = fragment.function {
                if let Some(name) = function.name.filter(|n| !n.is_empty())
                    && call.name.is_empty()
                {
                    call.name = name;
                }
                if let Some(arguments) = function.arguments {
                    call.arguments.push_str(&arguments);
                }
            }
        }
    }

    /// Assemble the response a non-streaming request would have returned.
    /// Fails when the stream stopped short of its end.
    fn finish(
        self,
        header_cached_tokens: Option<u32>,
    ) -> Result<InferenceResponse, InferenceError> {
        if !self.done && self.finish_reason.is_none() {
            return Err(InferenceError::StreamInterrupted(
                "the stream ended before the response was complete".to_string(),
            ));
        }
        let (content, thinking) = self.reply.finish();
        let truncated = self.finish_reason.as_deref() == Some("length");
        let tool_calls = build_tool_calls(self.tool_calls.into_values().collect(), truncated)?;

        let mut resp = InferenceResponse::new(content, tool_calls);
        resp.usage = self.usage.map(|u| usage_from(u, header_cached_tokens));
        resp.thinking = thinking;
        resp.stop_reason = self.finish_reason.as_deref().map(map_stop_reason);
        Ok(resp)
    }
}

// --- OpenAI API request/response types ---

/// What stays the same across a request's attempts.
struct PreparedRequest {
    messages: Vec<OpenAiMessage>,
    tools: Option<Vec<OpenAiToolEntry>>,
    response_format: Option<OpenAiResponseFormat>,
    temperature: Option<f32>,
    reasoning_effort: Option<String>,
}

impl PreparedRequest {
    /// The request body for one attempt.
    fn request<'a>(&'a self, model: &'a str, stream: bool) -> ChatCompletionRequest<'a> {
        ChatCompletionRequest {
            model,
            messages: &self.messages,
            tools: self.tools.as_deref(),
            tool_choice: self.tools.is_some().then_some("auto"),
            response_format: self.response_format.as_ref(),
            temperature: self.temperature,
            reasoning_effort: self.reasoning_effort.as_deref(),
            stream: stream.then_some(true),
            stream_options: stream.then_some(StreamOptions {
                include_usage: true,
            }),
        }
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [OpenAiMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [OpenAiToolEntry]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a OpenAiResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

/// Asks a streamed response to end with its token usage.
#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
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
    /// The reasoning behind an assistant message, for hosts that want it
    /// back within a tool-use exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
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
            reasoning_content: None,
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
    /// Some compatible servers send none.
    #[serde(default)]
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
    finish_reason: Option<String>,
}

/// Map `OpenAI`'s `finish_reason` to the provider-agnostic [`StopReason`].
fn map_stop_reason(raw: &str) -> StopReason {
    match raw {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(flatten)]
    reasoning: ReasoningFields,
}

/// The ways compatible servers return a model's reasoning beside its text:
/// `reasoning_content` (Fireworks, `DeepSeek`), `reasoning` (vLLM,
/// `OpenRouter`) and `reasoning_details` (`OpenRouter`). `OpenAI` itself
/// returns none.
#[derive(Deserialize, Default)]
struct ReasoningFields {
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_details: Option<Vec<ReasoningDetail>>,
}

#[derive(Deserialize)]
struct ReasoningDetail {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

impl ReasoningFields {
    /// The reasoning text, from whichever field carries it. A server that
    /// sends the same reasoning both as a string and as details is read once.
    fn text(&self) -> String {
        let plain = [&self.reasoning_content, &self.reasoning]
            .into_iter()
            .flatten()
            .find(|text| !text.is_empty());
        if let Some(text) = plain {
            return text.clone();
        }
        self.reasoning_details
            .iter()
            .flatten()
            .filter_map(|detail| detail.text.as_deref().or(detail.summary.as_deref()))
            .collect()
    }
}

// --- Streamed chunks ---

#[derive(Deserialize)]
struct StreamChunk {
    /// Empty on the final chunk, which carries only usage.
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
    /// A failure reported in the stream after the response began.
    #[serde(default)]
    error: Option<ChunkError>,
}

#[derive(Deserialize)]
struct ChunkError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChunkToolCall>>,
    #[serde(flatten)]
    reasoning: ReasoningFields,
}

/// A fragment of a tool call. The call's id and name come in its first
/// fragment and its arguments are spread over all of them.
#[derive(Deserialize)]
struct ChunkToolCall {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChunkFunction>,
}

#[derive(Deserialize)]
struct ChunkFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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
    use crate::inference::test_support::{
        RecordingSink, ScriptedServer, Step, assert_same_response, at, json_response, split_bytes,
        sse_chunks, sse_response,
    };
    use crate::inference::{StreamDelta, ThinkingBlock};
    use serde_json::{Value, json};
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
                server: None,
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
        assert_eq!(
            response.stop_reason, None,
            "a response with no finish_reason field must parse, not error"
        );
    }

    #[tokio::test]
    async fn stop_reason_length_maps_to_truncation() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "cut off mid-sen"
                    },
                    "finish_reason": "length"
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let response = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(response.stop_reason, Some(StopReason::MaxTokens));
        assert!(
            response.was_truncated(),
            "length must be reported as a truncation"
        );
    }

    #[tokio::test]
    async fn stop_reason_stop_maps_to_end_turn() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "done"
                    },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&mock_server)
            .await;

        let client = make_client(mock_server.uri(), "gpt-4");
        let response = client
            .complete(
                &[Message::user("Hello")],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert!(!response.was_truncated());
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

    // --- Streaming ---

    fn sse_data(data: impl Into<Value>) -> String {
        let data: Value = data.into();
        format!("data: {data}\n\n")
    }

    fn delta_chunk(delta: impl Into<Value>) -> String {
        let delta: Value = delta.into();
        sse_data(json!({"choices": [{"index": 0, "delta": delta, "finish_reason": null}]}))
    }

    fn finish_chunk(reason: &str) -> String {
        sse_data(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}))
    }

    fn usage_chunk(prompt: u32, completion: u32) -> String {
        sse_data(json!({
            "choices": [],
            "usage": {"prompt_tokens": prompt, "completion_tokens": completion}
        }))
    }

    const DONE: &str = "data: [DONE]\n\n";

    async fn stream_from(
        script: Vec<Step>,
        client: impl FnOnce(String) -> OpenAiClient,
    ) -> (
        Result<InferenceResponse, InferenceError>,
        RecordingSink,
        ScriptedServer,
    ) {
        let server = ScriptedServer::start(vec![script]).await;
        let client = client(server.uri());
        let sink = RecordingSink::default();
        let result = client
            .complete_streaming(
                &[Message::user("hi")],
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await;
        (result, sink, server)
    }

    fn plain_client(url: String) -> OpenAiClient {
        make_client(url, "test-model")
    }

    /// Reasoning text as `plain_client`'s model returns it.
    fn test_model_text(text: &str) -> ThinkingBlock {
        ThinkingBlock::text(text).from_origin(&ThinkingOrigin::new(
            ProviderApi::OpenAiCompatible,
            "test-model",
        ))
    }

    /// The same reply, streamed and whole.
    async fn whole_response(body: Value) -> InferenceResponse {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        make_client(server.uri(), "test-model")
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn streamed_text_matches_the_non_streaming_response() {
        let stream = [
            delta_chunk(json!({"role": "assistant", "content": ""})),
            delta_chunk(json!({"content": "Hello w"})),
            delta_chunk(json!({"content": "\u{f6}rld"})),
            finish_chunk("stop"),
            usage_chunk(11, 4),
            DONE.to_string(),
        ]
        .concat();
        let (streamed, sink, server) = stream_from(sse_response(&[stream]), plain_client).await;
        let streamed = streamed.unwrap();

        let whole = whole_response(json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello w\u{f6}rld"},
                "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 11, "completion_tokens": 4}
        }))
        .await;

        assert_same_response(&streamed, &whole);
        assert_eq!(streamed.content, "Hello w\u{f6}rld", "text assembled");
        assert_eq!(sink.text(), "Hello w\u{f6}rld", "text streamed");
        assert_eq!(
            streamed.usage.unwrap().input_tokens,
            11,
            "usage from the last chunk"
        );
        assert_eq!(
            streamed.stop_reason,
            Some(StopReason::EndTurn),
            "finish reason"
        );

        let requests = server.requests();
        let body = requests.first().unwrap().json();
        assert_eq!(at(&body, "/stream"), &json!(true), "asks for a stream");
        assert_eq!(
            at(&body, "/stream_options/include_usage"),
            &json!(true),
            "asks for usage at the end"
        );
    }

    #[tokio::test]
    async fn a_server_that_ignores_the_stream_request_is_read_whole() {
        let body = json!({
            "choices": [{"message": {"role": "assistant", "content": "Whole answer"},
                "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 2}
        });
        let (streamed, sink, _server) =
            stream_from(json_response(200, &body.to_string()), plain_client).await;
        let streamed = streamed.unwrap();

        assert_same_response(&streamed, &whole_response(body).await);
        assert_eq!(
            streamed.content, "Whole answer",
            "the whole body is the reply"
        );
        assert!(
            sink.deltas().is_empty(),
            "nothing streamed: {:?}",
            sink.deltas()
        );
    }

    #[tokio::test]
    async fn complete_does_not_ask_for_a_stream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"role": "assistant", "content": "ok"}}]
            })))
            .mount(&server)
            .await;
        make_client(server.uri(), "m")
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("stream").is_none() && body.get("stream_options").is_none(),
            "a plain completion carries no streaming fields: {body}"
        );
    }

    #[tokio::test]
    async fn streamed_tool_call_fragments_are_assembled_per_index() {
        let stream = [
            delta_chunk(json!({"role": "assistant", "content": null, "tool_calls": [
                {"index": 0, "id": "call_a", "type": "function",
                 "function": {"name": "exec", "arguments": ""}}
            ]})),
            delta_chunk(json!({"tool_calls": [
                {"index": 1, "id": "call_b", "type": "function",
                 "function": {"name": "read", "arguments": "{\"pa"}}
            ]})),
            delta_chunk(json!({"tool_calls": [
                {"index": 0, "function": {"arguments": "{\"command\":"}}
            ]})),
            delta_chunk(json!({"tool_calls": [
                {"index": 1, "function": {"arguments": "th\":\"a.txt\"}"}}
            ]})),
            delta_chunk(json!({"tool_calls": [
                {"index": 0, "function": {"arguments": "\"ls\"}"}}
            ]})),
            finish_chunk("tool_calls"),
            usage_chunk(20, 12),
            DONE.to_string(),
        ]
        .concat();
        let (streamed, _, _) = stream_from(sse_response(&[stream]), plain_client).await;
        let streamed = streamed.unwrap();

        let whole = whole_response(json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_a", "type": "function",
                 "function": {"name": "exec", "arguments": "{\"command\":\"ls\"}"}},
                {"id": "call_b", "type": "function",
                 "function": {"name": "read", "arguments": "{\"path\":\"a.txt\"}"}}
            ]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 12}
        }))
        .await;

        assert_same_response(&streamed, &whole);
        let names: Vec<&str> = streamed
            .tool_calls
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["exec", "read"], "calls in index order");
        assert_eq!(
            streamed.tool_calls.first().unwrap().arguments,
            json!({"command": "ls"}),
            "fragments joined and parsed"
        );
        assert_eq!(
            streamed.stop_reason,
            Some(StopReason::ToolUse),
            "stop reason"
        );
    }

    #[tokio::test]
    async fn tool_calls_without_ids_or_arguments_are_still_usable() {
        let stream = [
            delta_chunk(json!({"tool_calls": [
                {"index": 0, "function": {"name": "now"}}
            ]})),
            finish_chunk("tool_calls"),
            DONE.to_string(),
        ]
        .concat();
        let (streamed, _, _) = stream_from(sse_response(&[stream]), plain_client).await;
        let streamed = streamed.unwrap();
        let call = streamed.tool_calls.first().unwrap();
        assert_eq!(call.id, "call_0", "a call with no id is named by position");
        assert_eq!(
            call.arguments,
            json!({}),
            "no arguments is the empty object"
        );

        let whole = whole_response(json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
                {"type": "function", "function": {"name": "now", "arguments": ""}}
            ]}, "finish_reason": "tool_calls"}]
        }))
        .await;
        assert_same_response(&streamed, &whole);
    }

    #[tokio::test]
    async fn tool_arguments_cut_off_by_the_length_limit_drop_that_call() {
        let stream = [
            delta_chunk(json!({"content": "Working"})),
            delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "c", "function": {"name": "exec", "arguments": "{\"command\": \"l"}}
            ]})),
            finish_chunk("length"),
            DONE.to_string(),
        ]
        .concat();
        let (streamed, _, _) = stream_from(sse_response(&[stream]), plain_client).await;
        let streamed = streamed.unwrap();
        assert!(
            streamed.tool_calls.is_empty(),
            "the half-written call is dropped"
        );
        assert!(streamed.was_truncated(), "the stop reason says why");
        assert_eq!(streamed.content, "Working", "the text before it is kept");
    }

    #[tokio::test]
    async fn every_reasoning_field_streams_as_thinking() {
        let variants = [
            ("reasoning_content", json!({"reasoning_content": "weigh "})),
            ("reasoning", json!({"reasoning": "weigh "})),
            (
                "reasoning_details",
                json!({"reasoning_details": [{"type": "reasoning.text", "text": "weigh "}]}),
            ),
            (
                "both a string and details",
                json!({"reasoning": "weigh ",
                    "reasoning_details": [{"type": "reasoning.text", "text": "weigh "}]}),
            ),
        ];
        for (label, first) in variants {
            let stream = [
                delta_chunk(first),
                delta_chunk(json!({"reasoning_content": null, "reasoning": "it"})),
                delta_chunk(json!({"content": "Answer"})),
                finish_chunk("stop"),
                DONE.to_string(),
            ]
            .concat();
            let (streamed, sink, _) = stream_from(sse_response(&[stream]), plain_client).await;
            let streamed = streamed.unwrap();
            assert_eq!(
                streamed.thinking,
                vec![test_model_text("weigh it")],
                "{label}: reasoning is captured once"
            );
            assert_eq!(sink.thinking(), "weigh it", "{label}: and streamed");
            assert_eq!(
                streamed.content, "Answer",
                "{label}: content is only the answer"
            );
        }
    }

    #[tokio::test]
    async fn non_streaming_reasoning_fields_are_captured() {
        for message in [
            json!({"content": "A", "reasoning_content": "because"}),
            json!({"content": "A", "reasoning": "because"}),
            json!({"content": "A", "reasoning_details": [{"summary": "because"}]}),
        ] {
            let response = whole_response(json!({"choices": [{"message": message}]})).await;
            assert_eq!(
                response.thinking,
                vec![test_model_text("because")],
                "reasoning comes through complete"
            );
        }
    }

    #[tokio::test]
    async fn inline_think_blocks_become_thinking_even_when_a_tag_is_split() {
        let stream = [
            delta_chunk(json!({"content": "<thi"})),
            delta_chunk(json!({"content": "nk>\nplan it"})),
            delta_chunk(json!({"content": "</th"})),
            delta_chunk(json!({"content": "ink>\n\nThe an"})),
            delta_chunk(json!({"content": "swer"})),
            finish_chunk("stop"),
            DONE.to_string(),
        ]
        .concat();
        let (streamed, sink, _) = stream_from(sse_response(&[stream]), plain_client).await;
        let streamed = streamed.unwrap();
        assert_eq!(streamed.content, "The answer", "the content has no tags");
        assert_eq!(
            streamed.thinking,
            vec![test_model_text("plan it")],
            "the tagged text is the thinking"
        );
        assert_eq!(sink.text(), "The answer", "only the answer streams as text");
        assert!(
            !sink.text().contains('<') && !sink.thinking().contains('<'),
            "no tag fragment leaks into either stream"
        );

        let whole = whole_response(json!({
            "choices": [{"message": {"content": "<think>\nplan it</think>\n\nThe answer"},
                "finish_reason": "stop"}]
        }))
        .await;
        assert_same_response(&streamed, &whole);
    }

    #[tokio::test]
    async fn stream_parses_at_every_chunk_boundary_including_inside_characters() {
        let stream = [
            delta_chunk(json!({"reasoning_content": "caf\u{e9} "})),
            delta_chunk(json!({"content": "<think>x</think>Hello w\u{f6}rld \u{1f600}"})),
            delta_chunk(json!({"tool_calls": [
                {"index": 0, "id": "c1", "function": {"name": "exec", "arguments": "{\"a\":"}}
            ]})),
            delta_chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "1}"}}]})),
            finish_chunk("tool_calls"),
            usage_chunk(3, 4),
            DONE.to_string(),
        ]
        .concat();
        let (whole, _, _) =
            stream_from(sse_response(std::slice::from_ref(&stream)), plain_client).await;
        let whole = whole.unwrap();
        for size in [1, 2, 3, 5, 17] {
            let chunks = split_bytes(&stream, size);
            let (result, sink, _) = stream_from(sse_response(&chunks), plain_client).await;
            let result = result.unwrap();
            assert_same_response(&result, &whole);
            assert_eq!(
                sink.text(),
                "Hello w\u{f6}rld \u{1f600}",
                "chunks of {size} bytes keep characters whole"
            );
        }
    }

    #[tokio::test]
    async fn stream_ending_after_the_finish_reason_without_done_is_complete() {
        let stream = [delta_chunk(json!({"content": "hi"})), finish_chunk("stop")].concat();
        let (result, _, _) = stream_from(sse_response(&[stream]), plain_client).await;
        assert_eq!(
            result.unwrap().content,
            "hi",
            "servers that skip the [DONE] marker still finish"
        );
    }

    #[tokio::test]
    async fn stream_cut_off_before_it_finishes_is_interrupted() {
        let partial = delta_chunk(json!({"content": "half an ans"}));

        let (clean_end, _, _) =
            stream_from(sse_response(std::slice::from_ref(&partial)), plain_client).await;
        let err = clean_end.unwrap_err();
        assert!(
            matches!(err, InferenceError::StreamInterrupted(_)) && err.is_retryable(),
            "ending with no finish reason is an interrupted stream: {err:?}"
        );

        let (dropped, sink, _) = stream_from(sse_chunks(&[partial]), plain_client).await;
        let dropped_err = dropped.unwrap_err();
        assert!(
            matches!(dropped_err, InferenceError::StreamInterrupted(_)),
            "a dropped connection is an interrupted stream: {dropped_err:?}"
        );
        assert_eq!(sink.text(), "", "what streamed is voided");
    }

    #[tokio::test]
    async fn in_stream_error_surfaces_with_its_type() {
        let stream = [
            delta_chunk(json!({"content": "partial"})),
            sse_data(
                json!({"error": {"message": "The server had an error", "type": "server_error"}}),
            ),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[stream]), plain_client).await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("server_error")
                && err.to_string().contains("The server had an error"),
            "the error carries its type and message: {err}"
        );
        assert!(err.is_retryable(), "a server error is retried");
    }

    #[tokio::test]
    async fn stalled_stream_fails_after_the_idle_timeout() {
        let mut script = sse_chunks(&[delta_chunk(json!({"content": "partial"}))]);
        script.push(Step::pause(std::time::Duration::from_secs(5)));
        let (result, sink, _) =
            stream_from(script, |url| make_client_with_timeout(url, "m", 1)).await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Stalled(1)),
            "a silent stream is a stall: {err:?}"
        );
        assert_eq!(sink.text(), "", "the partial text is voided");
    }

    #[tokio::test]
    async fn stream_longer_than_the_timeout_completes_while_bytes_keep_arriving() {
        let mut script = vec![Step::head(200, "text/event-stream")];
        for word in ["one ", "two ", "three ", "four "] {
            script.push(Step::chunk(delta_chunk(json!({"content": word}))));
            script.push(Step::pause(std::time::Duration::from_millis(600)));
        }
        script.push(Step::chunk(finish_chunk("stop") + DONE));
        script.push(Step::end());
        let (result, _, _) = stream_from(script, |url| make_client_with_timeout(url, "m", 1)).await;
        assert_eq!(
            result.unwrap().content,
            "one two three four ",
            "the idle timeout does not cap the whole stream"
        );
    }

    #[tokio::test]
    async fn retry_after_partial_output_restarts_the_stream() {
        let first = [
            delta_chunk(json!({"content": "par"})),
            sse_data(json!({"error": {"message": "overloaded", "type": "server_error"}})),
        ]
        .concat();
        let second = [
            delta_chunk(json!({"content": "whole answer"})),
            finish_chunk("stop"),
            DONE.to_string(),
        ]
        .concat();
        let server =
            ScriptedServer::start(vec![sse_response(&[first]), sse_response(&[second])]).await;
        let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
        let client = OpenAiClient::with_http_client(
            http,
            server.uri(),
            "m",
            RetryConfig {
                max_retries: 1,
                initial_delay: std::time::Duration::from_millis(5),
                max_delay: std::time::Duration::from_millis(5),
                backoff_multiplier: 1.0,
            },
        );
        let sink = RecordingSink::default();
        let response = client
            .complete_streaming(
                &[Message::user("hi")],
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await
            .unwrap();
        assert_eq!(response.content, "whole answer", "the retry's answer");
        assert_eq!(
            sink.deltas(),
            vec![
                StreamDelta::Text("par".to_string()),
                StreamDelta::Restart,
                StreamDelta::Text("whole answer".to_string()),
            ],
            "the partial text is voided before the retry streams"
        );
    }

    #[tokio::test]
    async fn status_errors_before_the_stream_surface_like_non_streaming_ones() {
        let (result, sink, _) = stream_from(
            json_response(401, r#"{"error":{"message":"Incorrect API key provided"}}"#),
            plain_client,
        )
        .await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("401") && err.to_string().contains("Incorrect API key"),
            "status and message are kept: {err}"
        );
        assert!(sink.deltas().is_empty(), "nothing streamed");
    }

    #[tokio::test]
    async fn fireworks_cache_header_is_used_for_streamed_usage() {
        let stream = [
            delta_chunk(json!({"content": "x"})),
            finish_chunk("stop"),
            usage_chunk(10, 2),
            DONE.to_string(),
        ]
        .concat();
        let script = vec![
            Step::Write(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                  fireworks-cached-prompt-tokens: 7\r\ntransfer-encoding: chunked\r\n\
                  connection: close\r\n\r\n"
                    .to_vec(),
            ),
            Step::chunk(stream),
            Step::end(),
        ];
        let (result, _, _) = stream_from(script, |url| {
            let http = SharedHttpClient::new(&HttpClientConfig::default()).unwrap();
            OpenAiClient::with_http_client(http, url, "m", RetryConfig::no_retry()).with_dialect(
                OpenAiDialect::Fireworks {
                    session_affinity: None,
                },
            )
        })
        .await;
        assert_eq!(
            result.unwrap().usage.unwrap().cache_read_tokens,
            Some(7),
            "cache hits come from the header when the body lacks them"
        );
    }

    // --- Reasoning replay ---

    /// Where the reasoning of the clients in these tests comes from.
    fn test_origin() -> ThinkingOrigin {
        ThinkingOrigin::new(ProviderApi::OpenAiCompatible, "m")
    }

    fn reasoning_exchange() -> Vec<Message> {
        let call = |id: &str| ToolCall {
            id: id.to_string(),
            name: "exec".to_string(),
            arguments: json!({}),
            server: None,
        };
        let own = |text: &str| ThinkingBlock::text(text).from_origin(&test_origin());
        let gemini = ThinkingOrigin::new(ProviderApi::Gemini, "gemini-3-pro-preview");
        let mut old = Message::assistant("old", Some(vec![call("c0")]));
        old.thinking = vec![own("old reasoning")];
        let mut plain = Message::assistant("done", None);
        plain.thinking = vec![own("wrap-up reasoning")];
        let mut current = Message::assistant("", Some(vec![call("c1")]));
        current.thinking = vec![
            own("current reasoning"),
            ThinkingBlock::text("readable text from another provider").from_origin(&gemini),
            ThinkingBlock::text("readable text saved with no origin"),
            ThinkingBlock {
                text: "signed elsewhere".to_string(),
                signature: Some("sig".to_string()),
                ..ThinkingBlock::default()
            }
            .from_origin(&gemini),
        ];
        vec![
            Message::user("one"),
            old,
            Message::tool("r0", "c0"),
            plain,
            Message::user("two"),
            current,
            Message::tool("r1", "c1"),
        ]
    }

    fn replayed(client: &OpenAiClient) -> Vec<Option<String>> {
        client
            .convert_messages(&reasoning_exchange())
            .into_iter()
            .map(|m| m.reasoning_content)
            .collect()
    }

    fn client_at(url: &str) -> OpenAiClient {
        make_client(url, "m")
    }

    #[test]
    fn hosts_that_want_reasoning_get_it_back_for_the_current_exchange_only() {
        for url in [
            "http://localhost:8000/v1",
            "https://api.deepseek.com/v1",
            "https://openrouter.ai/api/v1",
        ] {
            let replayed = replayed(&client_at(url));
            assert_eq!(
                replayed,
                vec![
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("current reasoning".to_string()),
                    None
                ],
                "{url}: only the tool-calling message of the exchange in progress carries reasoning, \
                 and only what an OpenAI-compatible API returned for it, not another provider's text"
            );
        }
    }

    #[test]
    fn fireworks_gets_reasoning_back() {
        let client = client_at("https://api.fireworks.ai/inference/v1").with_dialect(
            OpenAiDialect::Fireworks {
                session_affinity: None,
            },
        );
        assert_eq!(
            replayed(&client).get(5).cloned().flatten(),
            Some("current reasoning".to_string()),
            "Fireworks understands reasoning_content"
        );
    }

    #[test]
    fn openai_itself_never_gets_unknown_fields() {
        for url in [
            "https://api.openai.com/v1",
            "https://API.OPENAI.COM/v1/",
            "https://my-resource.openai.azure.com/openai/v1",
        ] {
            assert!(
                replayed(&client_at(url)).iter().all(Option::is_none),
                "{url} must not receive reasoning_content"
            );
        }
    }

    #[test]
    fn an_unparseable_base_url_is_treated_like_openai() {
        assert!(
            replayed(&client_at("not a url"))
                .iter()
                .all(Option::is_none),
            "when the host is unknown, send nothing OpenAI would reject"
        );
    }

    #[tokio::test]
    async fn replayed_reasoning_reaches_the_wire_as_reasoning_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&server)
            .await;
        make_client(server.uri(), "m")
            .complete(&reasoning_exchange(), &[], &CompletionOptions::default())
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert_eq!(
            at(&body, "/messages/5/reasoning_content"),
            &json!("current reasoning"),
            "the field is on the assistant message that made the tool call"
        );
        assert!(
            body.pointer("/messages/1/reasoning_content").is_none(),
            "and not on an earlier exchange's"
        );
    }
}

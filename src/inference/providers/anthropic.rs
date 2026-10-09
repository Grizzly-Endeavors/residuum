//! Anthropic Messages API provider implementation.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::inference::http::{
    SharedHttpClient, map_request_error, map_stream_request_error, read_error_body,
    warn_if_insecure_remote,
};
use crate::inference::retry::{RetryConfig, with_retry};
use crate::inference::stream::{Flow, SseEvent, TrackedSink, read_sse};
use crate::inference::types::current_exchange_start;
use crate::inference::{
    CompletionOptions, ImageData, InferenceError, InferenceProvider, InferenceResponse, Message,
    ResponseFormat, Role, StopReason, StreamDelta, StreamSink, ThinkingBlock, ThinkingConfig,
    ThinkingLevel, ToolCall, ToolDefinition, Usage,
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

/// Lets a model that takes a manual thinking budget reason between tool calls.
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// The least thinking budget the API accepts in manual mode.
const MIN_THINKING_BUDGET: u32 = 1024;

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
    /// Set once the API has refused adaptive thinking for this client's
    /// model, after which requests ask for a manual budget straight away.
    manual_thinking: AtomicBool,
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
            manual_thinking: AtomicBool::new(false),
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
    ///
    /// With `replay_thinking`, the reasoning behind an assistant message that
    /// made tool calls goes back ahead of its other blocks while the
    /// tool-use exchange it belongs to is still going on, because the API
    /// rejects the next request otherwise. Reasoning behind earlier replies
    /// is not sent, and none is when the request has thinking off.
    fn convert_messages(
        messages: &[Message],
        replay_thinking: bool,
    ) -> (Option<String>, Vec<AnthropicMessage>) {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut api_messages: Vec<AnthropicMessage> = Vec::new();
        let exchange_start = current_exchange_start(messages);

        for (index, msg) in messages.iter().enumerate() {
            match msg.role {
                Role::System => {
                    if system_parts.is_empty() {
                        system_parts.push(&msg.content);
                    } else {
                        api_messages.push(AnthropicMessage {
                            role: String::from("user"),
                            content: AnthropicContent::Text(format!("System: {}", msg.content)),
                        });
                    }
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

                    if replay_thinking && index >= exchange_start {
                        blocks.extend(replayable_thinking(&msg.thinking));
                    }

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
        web_search: Option<&crate::inference::WebSearchNativeConfig>,
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

    /// Build everything about a request that stays the same across retries.
    fn prepare(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> PreparedRequest {
        let max_tokens = options.max_tokens.unwrap_or(self.max_tokens);
        let thinking_on = build_thinking(options.thinking.as_ref(), max_tokens, false)
            .thinking
            .is_some();
        let (system, messages) = Self::convert_messages(messages, thinking_on);
        let has_web_search = options.web_search.is_some();
        let tools = (!tools.is_empty() || has_web_search)
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

        let output_format = match &options.response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonSchema { schema, .. } => Some(AnthropicOutputFormat {
                r#type: "json_schema".to_string(),
                schema: schema.clone(),
            }),
        };

        PreparedRequest {
            system,
            messages,
            tools,
            output_format,
            temperature: options.temperature,
            max_tokens,
            thinking: options.thinking.clone(),
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

        with_retry(&self.retry, || async {
            let result = self
                .send_with_thinking_fallback(&prepared, tracked.as_ref())
                .await;
            if result.is_err()
                && let Some(tracked) = &tracked
            {
                tracked.restart_if_needed();
            }
            result
        })
        .await
    }

    /// Send a request asking for adaptive thinking, and again with a manual
    /// budget if the model turns out not to support adaptive thinking.
    async fn send_with_thinking_fallback(
        &self,
        prepared: &PreparedRequest,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        let mut manual = self.manual_thinking.load(Ordering::Relaxed);
        loop {
            let request = prepared.request(&self.model, sink.is_some(), manual);
            match self.send(&request, sink).await {
                Err(e) if !manual && request.thinking.is_some() && is_adaptive_unsupported(&e) => {
                    if !self.manual_thinking.swap(true, Ordering::Relaxed) {
                        info!(
                            model = %self.model,
                            "model does not support adaptive thinking; using a manual thinking budget from now on"
                        );
                    }
                    manual = true;
                }
                result => return result,
            }
        }
    }

    /// Send a pre-built request to the Anthropic API and parse the response.
    #[tracing::instrument(skip_all, fields(
        model = %request.model,
        message_count = request.messages.len(),
        tool_count = request.tools.map_or(0, <[AnthropicToolEntry]>::len),
        streaming = sink.is_some(),
    ))]
    async fn send(
        &self,
        request: &AnthropicRequest<'_>,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        debug!(
            max_tokens = request.max_tokens,
            "sending anthropic completion request"
        );

        let timeout_secs = self.http.timeout_secs();
        let client = if sink.is_some() {
            self.http.streaming_client()
        } else {
            self.http.client()
        };
        let mut req_builder = client
            .post(self.endpoint())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");

        // OAuth tokens (sk-ant-oat01-*) use Bearer auth + Claude Code identity
        // headers; standard API keys use x-api-key.
        let oauth = is_oauth_key(&self.api_key);
        if oauth {
            req_builder = req_builder
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("user-agent", OAUTH_USER_AGENT)
                .header("x-app", "cli");
        } else {
            req_builder = req_builder.header("x-api-key", &self.api_key);
        }

        let mut betas: Vec<&str> = Vec::new();
        if oauth {
            betas.push(OAUTH_BETA);
        }
        if request
            .thinking
            .as_ref()
            .is_some_and(AnthropicThinking::is_manual)
        {
            betas.push(INTERLEAVED_THINKING_BETA);
        }
        if !betas.is_empty() {
            req_builder = req_builder.header("anthropic-beta", betas.join(","));
        }

        let request_json = serde_json::to_string(request)
            .map_err(|e| InferenceError::Parse(format!("failed to serialize request: {e}")))?;

        let response = req_builder
            .body(request_json.clone())
            .send()
            .await
            .map_err(|e| {
                if sink.is_some() {
                    map_stream_request_error(e, timeout_secs)
                } else {
                    map_request_error(e, timeout_secs)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = read_error_body(response).await;
            let error = api_error(status, &body);
            if is_adaptive_unsupported(&error) {
                debug!(error = %error, "model refused adaptive thinking");
            } else {
                warn!(
                    status = %status,
                    response_body = %body,
                    request_body = %request_json,
                    "anthropic API error — full request/response for diagnosis"
                );
            }
            return Err(error);
        }

        let api_response = if let Some(sink) = sink {
            read_stream(response, timeout_secs, sink).await?
        } else {
            let body = response
                .text()
                .await
                .map_err(|e| map_request_error(e, timeout_secs))?;
            serde_json::from_str::<AnthropicResponse>(&body).map_err(|e| {
                InferenceError::Parse(format!("failed to parse anthropic response: {e}"))
            })?
        };

        let result = Self::parse_response(api_response);

        info!(
            model = %request.model,
            content_len = result.content.len(),
            tool_calls = result.tool_calls.len(),
            "anthropic completion received"
        );

        Ok(result)
    }

    /// Parse the API response into our generic `InferenceResponse`.
    fn parse_response(response: AnthropicResponse) -> InferenceResponse {
        let mut text_parts: Vec<String> = Vec::new();
        let mut thinking: Vec<ThinkingBlock> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in response.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    text_parts.push(text);
                }
                AnthropicContentBlock::Thinking {
                    thinking: text,
                    signature,
                } => {
                    // A block with neither text nor a signature has nothing to
                    // show and nothing to send back.
                    if !text.is_empty() || signature.is_some() {
                        thinking.push(ThinkingBlock {
                            text,
                            signature,
                            ..ThinkingBlock::default()
                        });
                    }
                }
                AnthropicContentBlock::RedactedThinking { data } => {
                    thinking.push(ThinkingBlock {
                        redacted: Some(data),
                        ..ThinkingBlock::default()
                    });
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                        server: None,
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

        let usage = response.usage.map(|u| Usage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_tokens: u.cache_creation_input_tokens,
            cache_read_tokens: u.cache_read_input_tokens,
        });

        let mut resp = InferenceResponse::new(text_parts.join(""), tool_calls);
        resp.usage = usage;
        resp.thinking = thinking;
        resp.stop_reason = response.stop_reason.as_deref().map(map_stop_reason);
        resp
    }
}

#[async_trait]
impl InferenceProvider for AnthropicClient {
    /// Send a completion request to the Anthropic Messages API.
    ///
    /// # Errors
    /// Returns `InferenceError::Timeout` if the request exceeds the configured timeout,
    /// `InferenceError::Api` if the API returns an error status, `InferenceError::Parse` if
    /// the response body is malformed, or `InferenceError::Request` for network failures.
    #[tracing::instrument(skip_all, fields(model = %self.model, message_count = messages.len(), tool_count = tools.len()))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.run(messages, tools, options, None).await
    }

    /// Like `complete`, streaming the response's text and thinking into
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

pub(crate) fn is_oauth_key(key: &str) -> bool {
    key.starts_with("sk-ant-oat01-")
}

/// Whether the API refused a request because the model does not accept
/// adaptive thinking.
fn is_adaptive_unsupported(error: &InferenceError) -> bool {
    let InferenceError::Api(message) = error else {
        return false;
    };
    let lower = message.to_lowercase();
    lower.contains("adaptive") && lower.contains("not supported")
}

/// Turn a failed response into the error callers see.
fn api_error(status: reqwest::StatusCode, body: &str) -> InferenceError {
    let message = serde_json::from_str::<AnthropicErrorResponse>(body).map_or_else(
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
    InferenceError::Api(message)
}

// ---------------------------------------------------------------------------
// Thinking
// ---------------------------------------------------------------------------

/// The blocks of an assistant message's reasoning that the API accepts back:
/// each readable block with its signature, and redacted blocks unchanged. A
/// block without either (one from another provider after a failover, or a
/// Gemini signature, which is tied to a part of its own response) would be
/// rejected, so it is left out.
fn replayable_thinking(thinking: &[ThinkingBlock]) -> impl Iterator<Item = AnthropicContentBlock> {
    thinking.iter().filter_map(|block| {
        if block.part.is_some() {
            return None;
        }
        match (&block.redacted, &block.signature) {
            (Some(data), _) => Some(AnthropicContentBlock::RedactedThinking { data: data.clone() }),
            (None, Some(signature)) if !signature.is_empty() => {
                Some(AnthropicContentBlock::Thinking {
                    thinking: block.text.clone(),
                    signature: Some(signature.clone()),
                })
            }
            (None, _) => None,
        }
    })
}

/// How a request asks for thinking: the `thinking` field and the effort that
/// goes beside it in `output_config`.
struct ThinkingRequest {
    thinking: Option<AnthropicThinking>,
    effort: Option<&'static str>,
}

/// Decide how to ask for the configured thinking.
///
/// Current models take only `adaptive` thinking, whose depth is the separate
/// `effort`; older ones take only a manual `budget_tokens`. Adaptive is tried
/// first, and `manual` is set once a model has refused it.
fn build_thinking(
    config: Option<&ThinkingConfig>,
    max_tokens: u32,
    manual: bool,
) -> ThinkingRequest {
    let off = ThinkingRequest {
        thinking: None,
        effort: None,
    };
    let Some(config) = config else {
        return off;
    };
    if matches!(config, ThinkingConfig::Toggle(false)) {
        return off;
    }
    if !manual {
        return ThinkingRequest {
            thinking: Some(AnthropicThinking::adaptive()),
            effort: match config {
                ThinkingConfig::Level(ThinkingLevel::Low) => Some("low"),
                ThinkingConfig::Level(ThinkingLevel::Medium) => Some("medium"),
                ThinkingConfig::Level(ThinkingLevel::High) => Some("high"),
                ThinkingConfig::Toggle(_) => None,
            },
        };
    }

    let share = match config {
        ThinkingConfig::Level(ThinkingLevel::Low) => max_tokens / 4,
        ThinkingConfig::Level(ThinkingLevel::Medium) | ThinkingConfig::Toggle(_) => max_tokens / 2,
        ThinkingConfig::Level(ThinkingLevel::High) => max_tokens - max_tokens / 4,
    };
    // The budget must be at least 1024 tokens and leave room below max_tokens.
    if max_tokens <= MIN_THINKING_BUDGET {
        warn!(
            max_tokens,
            "max_tokens leaves no room for a thinking budget on this model; sending the request without thinking"
        );
        return off;
    }
    ThinkingRequest {
        thinking: Some(AnthropicThinking::manual(
            share.clamp(MIN_THINKING_BUDGET, max_tokens - 1),
        )),
        effort: None,
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Read a streamed Messages response to its end, pushing text and thinking to
/// `sink` as it arrives, and return it as the response a non-streaming
/// request would have returned.
async fn read_stream(
    response: reqwest::Response,
    idle_secs: u64,
    sink: &dyn StreamSink,
) -> Result<AnthropicResponse, InferenceError> {
    let mut assembler = StreamAssembler::new(sink);
    read_sse(response, idle_secs, |event| assembler.handle(&event)).await?;
    assembler.finish()
}

/// One streamed event, by its `type`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart {
        message: StreamMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: Value,
    },
    ContentBlockDelta {
        index: usize,
        delta: BlockDelta,
    },
    ContentBlockStop {},
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<PartialUsage>,
    },
    MessageStop {},
    Ping {},
    Error {
        error: StreamErrorBody,
    },
    /// An event type this client does not know; the API may add more.
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct StreamMessageStart {
    #[serde(default)]
    usage: Option<PartialUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    /// A delta type this client does not use, such as `citations_delta`.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamErrorBody {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    message: String,
}

/// Token counts as they stream in. Every report is cumulative and may omit
/// fields, so each field keeps the last value it was given.
#[derive(Deserialize, Default, Clone, Copy)]
struct PartialUsage {
    #[serde(rename = "input_tokens")]
    input: Option<u32>,
    #[serde(rename = "output_tokens")]
    output: Option<u32>,
    #[serde(rename = "cache_creation_input_tokens")]
    cache_creation: Option<u32>,
    #[serde(rename = "cache_read_input_tokens")]
    cache_read: Option<u32>,
}

impl PartialUsage {
    fn merge(&mut self, latest: Self) {
        self.input = latest.input.or(self.input);
        self.output = latest.output.or(self.output);
        self.cache_creation = latest.cache_creation.or(self.cache_creation);
        self.cache_read = latest.cache_read.or(self.cache_read);
    }
}

/// A content block as it builds up from its deltas.
struct StreamBlock {
    /// `None` for a block type this client does not know.
    block: Option<AnthropicContentBlock>,
    /// The tool input, which arrives as fragments of JSON text and is parsed
    /// once the whole response is in.
    partial_json: String,
}

struct StreamAssembler<'a> {
    sink: &'a dyn StreamSink,
    blocks: BTreeMap<usize, StreamBlock>,
    usage: Option<PartialUsage>,
    stop_reason: Option<String>,
    stopped: bool,
}

impl<'a> StreamAssembler<'a> {
    fn new(sink: &'a dyn StreamSink) -> Self {
        Self {
            sink,
            blocks: BTreeMap::new(),
            usage: None,
            stop_reason: None,
            stopped: false,
        }
    }

    fn handle(&mut self, event: &SseEvent) -> Result<Flow, InferenceError> {
        let parsed: StreamEvent = serde_json::from_str(&event.data).map_err(|e| {
            InferenceError::Parse(format!("failed to parse anthropic stream event: {e}"))
        })?;
        match parsed {
            StreamEvent::MessageStart { message } => {
                if let Some(usage) = message.usage {
                    self.merge_usage(usage);
                }
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.start_block(index, content_block),
            StreamEvent::ContentBlockDelta { index, delta } => self.apply_delta(index, delta),
            StreamEvent::MessageDelta { delta, usage } => {
                if delta.stop_reason.is_some() {
                    self.stop_reason = delta.stop_reason;
                }
                if let Some(usage) = usage {
                    self.merge_usage(usage);
                }
            }
            StreamEvent::MessageStop {} => {
                self.stopped = true;
                return Ok(Flow::Done);
            }
            StreamEvent::Error { error } => {
                return Err(InferenceError::Api(format!(
                    "anthropic stream error ({}): {}",
                    error.r#type, error.message
                )));
            }
            StreamEvent::ContentBlockStop {} | StreamEvent::Ping {} => {}
            StreamEvent::Unknown => {
                debug!(event_type = ?event.event, "ignoring unknown anthropic stream event");
            }
        }
        Ok(Flow::Continue)
    }

    fn merge_usage(&mut self, latest: PartialUsage) {
        self.usage
            .get_or_insert_with(PartialUsage::default)
            .merge(latest);
    }

    fn start_block(&mut self, index: usize, content_block: Value) {
        let block = match serde_json::from_value::<AnthropicContentBlock>(content_block) {
            Ok(block) => Some(block),
            Err(e) => {
                debug!(index, error = %e, "ignoring anthropic content block of an unsupported type");
                None
            }
        };
        self.blocks.insert(
            index,
            StreamBlock {
                block,
                partial_json: String::new(),
            },
        );
    }

    fn apply_delta(&mut self, index: usize, delta: BlockDelta) {
        let Some(entry) = self.blocks.get_mut(&index) else {
            debug!(
                index,
                "ignoring a delta for a content block that never started"
            );
            return;
        };
        match (entry.block.as_mut(), delta) {
            (Some(AnthropicContentBlock::Text { text }), BlockDelta::TextDelta { text: more }) => {
                text.push_str(&more);
                self.sink.push(StreamDelta::Text(more));
            }
            (
                Some(AnthropicContentBlock::Thinking { thinking, .. }),
                BlockDelta::ThinkingDelta { thinking: more },
            ) => {
                thinking.push_str(&more);
                self.sink.push(StreamDelta::Thinking(more));
            }
            (
                Some(AnthropicContentBlock::Thinking { signature, .. }),
                BlockDelta::SignatureDelta { signature: more },
            ) => signature.get_or_insert_with(String::new).push_str(&more),
            (
                Some(
                    AnthropicContentBlock::ToolUse { .. }
                    | AnthropicContentBlock::ServerToolUse { .. },
                ),
                BlockDelta::InputJsonDelta { partial_json },
            ) => entry.partial_json.push_str(&partial_json),
            (_, BlockDelta::Other) => {}
            (_, BlockDelta::TextDelta { .. }) => {
                debug!(index, "ignoring a text delta for a block that is not text");
            }
            (_, BlockDelta::ThinkingDelta { .. } | BlockDelta::SignatureDelta { .. }) => {
                debug!(
                    index,
                    "ignoring a thinking delta for a block that is not thinking"
                );
            }
            (_, BlockDelta::InputJsonDelta { .. }) => {
                debug!(
                    index,
                    "ignoring an input delta for a block that is not a tool call"
                );
            }
        }
    }

    /// Assemble the blocks into the response a non-streaming request would
    /// have returned. Fails when the stream stopped short of `message_stop`.
    fn finish(self) -> Result<AnthropicResponse, InferenceError> {
        if !self.stopped {
            return Err(InferenceError::StreamInterrupted(
                "the stream ended before the response was complete".to_string(),
            ));
        }
        let truncated = self.stop_reason.as_deref() == Some("max_tokens");
        let mut content = Vec::with_capacity(self.blocks.len());
        for entry in self.blocks.into_values() {
            let Some(mut block) = entry.block else {
                continue;
            };
            if !entry.partial_json.is_empty() {
                match &mut block {
                    AnthropicContentBlock::ToolUse { name, input, .. }
                    | AnthropicContentBlock::ServerToolUse { name, input, .. } => {
                        match serde_json::from_str(&entry.partial_json) {
                            Ok(parsed) => *input = parsed,
                            // A response cut off by max_tokens ends mid-way
                            // through a tool call's input; that call is lost,
                            // and the stop reason tells the caller why.
                            Err(e) if truncated => {
                                warn!(
                                    tool = %name,
                                    error = %e,
                                    "dropping a tool call whose input was cut off by max_tokens"
                                );
                                continue;
                            }
                            Err(e) => {
                                return Err(InferenceError::Parse(format!(
                                    "failed to parse tool arguments for '{name}': {e} (raw: {})",
                                    entry.partial_json
                                )));
                            }
                        }
                    }
                    AnthropicContentBlock::Text { .. }
                    | AnthropicContentBlock::Thinking { .. }
                    | AnthropicContentBlock::RedactedThinking { .. }
                    | AnthropicContentBlock::Image { .. }
                    | AnthropicContentBlock::ToolResult { .. }
                    | AnthropicContentBlock::WebSearchToolResult { .. } => {}
                }
            }
            content.push(block);
        }
        let usage = self.usage.map(|u| AnthropicUsage {
            input_tokens: u.input.unwrap_or(0),
            output_tokens: u.output.unwrap_or(0),
            cache_creation_input_tokens: u.cache_creation,
            cache_read_input_tokens: u.cache_read,
        });
        Ok(AnthropicResponse {
            content,
            usage,
            stop_reason: self.stop_reason,
        })
    }
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

impl AnthropicCacheControl {
    const DEFAULT: Self = Self {
        r#type: "ephemeral",
    };
}

/// The request's `thinking` field.
#[derive(Debug, Clone, Serialize)]
struct AnthropicThinking {
    r#type: &'static str,
    /// How the reasoning comes back. Current models omit it unless asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

impl AnthropicThinking {
    /// The model decides how much to think; the response carries a summary.
    fn adaptive() -> Self {
        Self {
            r#type: "adaptive",
            display: Some("summarized"),
            budget_tokens: None,
        }
    }

    /// Think for up to `budget_tokens`.
    fn manual(budget_tokens: u32) -> Self {
        Self {
            r#type: "enabled",
            display: None,
            budget_tokens: Some(budget_tokens),
        }
    }

    fn is_manual(&self) -> bool {
        self.budget_tokens.is_some()
    }
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

/// What stays the same across a request's attempts.
struct PreparedRequest {
    system: Option<AnthropicSystem>,
    messages: Vec<AnthropicMessage>,
    tools: Option<Vec<AnthropicToolEntry>>,
    output_format: Option<AnthropicOutputFormat>,
    temperature: Option<f32>,
    max_tokens: u32,
    thinking: Option<ThinkingConfig>,
}

impl PreparedRequest {
    /// The request body for one attempt.
    fn request<'a>(
        &'a self,
        model: &'a str,
        stream: bool,
        manual_thinking: bool,
    ) -> AnthropicRequest<'a> {
        let ThinkingRequest { thinking, effort } =
            build_thinking(self.thinking.as_ref(), self.max_tokens, manual_thinking);
        let output_config =
            (self.output_format.is_some() || effort.is_some()).then_some(AnthropicOutputConfig {
                format: self.output_format.as_ref(),
                effort,
            });
        AnthropicRequest {
            model,
            max_tokens: self.max_tokens,
            system: self.system.as_ref(),
            messages: &self.messages,
            tools: self.tools.as_deref(),
            output_config,
            temperature: self.temperature,
            thinking,
            stream: stream.then_some(true),
            cache_control: AnthropicCacheControl::DEFAULT,
        }
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a AnthropicSystem>,
    messages: &'a [AnthropicMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [AnthropicToolEntry]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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
        /// Proof the reasoning came from the model; the API requires it back
        /// with the block. Absent from a response that carries none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Reasoning the API returns encrypted; it has to be sent back unchanged.
    RedactedThinking {
        data: String,
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

/// The request's `output_config`: the shape of the answer and how hard the
/// model should work on it.
#[derive(Debug, Serialize)]
struct AnthropicOutputConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a AnthropicOutputFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'static str>,
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
    stop_reason: Option<String>,
}

/// Map Anthropic's `stop_reason` to the provider-agnostic [`StopReason`].
fn map_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        other => StopReason::Other(other.to_string()),
    }
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
#[expect(
    clippy::get_unwrap,
    reason = "test code uses get().unwrap() for clarity"
)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::inference::http::HttpClientConfig;
    use crate::inference::retry::RetryConfig;
    use crate::inference::test_support::{
        RecordingSink, ScriptedServer, Step, assert_same_response, at, json_response, split_bytes,
        sse_chunks, sse_response,
    };

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
        assert_eq!(
            resp.stop_reason,
            Some(StopReason::EndTurn),
            "end_turn should map to StopReason::EndTurn"
        );
        assert!(!resp.was_truncated(), "end_turn is not a truncation");
    }

    #[tokio::test]
    async fn stop_reason_max_tokens_maps_to_truncation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "cut off mid-sen"}],
                "stop_reason": "max_tokens",
                "usage": {"input_tokens": 10, "output_tokens": 1024}
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_eq!(
            resp.stop_reason,
            Some(StopReason::MaxTokens),
            "max_tokens should map to StopReason::MaxTokens"
        );
        assert!(
            resp.was_truncated(),
            "max_tokens must be reported as a truncation"
        );
    }

    #[tokio::test]
    async fn stop_reason_tool_use_maps_correctly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "tool_use", "id": "call_1", "name": "exec", "input": {}}],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert!(!resp.was_truncated());
    }

    #[tokio::test]
    async fn stop_reason_missing_from_response_is_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "hi"}],
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let resp = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_eq!(
            resp.stop_reason, None,
            "a response with no stop_reason field must parse, not error"
        );
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
        let (system, api_msgs) = AnthropicClient::convert_messages(&messages, true);
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
                    server: None,
                }]),
            ),
            Message::tool("Rust is a systems programming language.", "toolu_abc123"),
        ];

        let (system, api_msgs) = AnthropicClient::convert_messages(&messages, true);
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
            matches!(err, InferenceError::Timeout(_)),
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
            response_format: crate::inference::ResponseFormat::JsonSchema {
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
    async fn thinking_level_sends_adaptive_thinking_with_effort() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "thinking": {"type": "adaptive", "display": "summarized"},
                "output_config": {"effort": "medium"}
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

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            at(&body, "/thinking").get("budget_tokens").is_none(),
            "adaptive thinking carries no budget: {body}"
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
            resp.thinking,
            vec![ThinkingBlock::text("Let me reason about this...")],
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
            web_search: Some(crate::inference::WebSearchNativeConfig {
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
            web_search: Some(crate::inference::WebSearchNativeConfig::default()),
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
                        server: None,
                    },
                    ToolCall {
                        id: "tool_2".to_string(),
                        name: "search".to_string(),
                        arguments: json!({"q": "b"}),
                        server: None,
                    },
                ]),
            ),
            Message::tool("Result A", "tool_1"),
            Message::tool("Result B", "tool_2"),
        ];

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages, true);

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
                    server: None,
                }]),
            ),
            Message::tool("Tool output", "tool_1"),
            Message::user("Thanks, now do something else"),
        ];

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages, true);

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

        let (_system, api_msgs) = AnthropicClient::convert_messages(&messages, true);

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

    #[test]
    fn adaptive_thinking_maps_levels_to_effort() {
        for (level, effort) in [
            (ThinkingLevel::Low, "low"),
            (ThinkingLevel::Medium, "medium"),
            (ThinkingLevel::High, "high"),
        ] {
            let request = build_thinking(Some(&ThinkingConfig::Level(level)), 8192, false);
            assert_eq!(request.effort, Some(effort), "{level:?} maps to {effort}");
            assert!(
                request.thinking.is_some_and(|t| !t.is_manual()),
                "{level:?} asks for adaptive thinking"
            );
        }
    }

    #[test]
    fn adaptive_toggle_on_sends_no_effort() {
        let request = build_thinking(Some(&ThinkingConfig::Toggle(true)), 8192, false);
        assert!(request.thinking.is_some(), "thinking is on");
        assert_eq!(request.effort, None, "an on/off toggle sets no depth");
    }

    #[test]
    fn thinking_off_sends_nothing() {
        for config in [None, Some(ThinkingConfig::Toggle(false))] {
            for manual in [false, true] {
                let request = build_thinking(config.as_ref(), 8192, manual);
                assert!(
                    request.thinking.is_none() && request.effort.is_none(),
                    "{config:?} (manual: {manual}) sends no thinking"
                );
            }
        }
    }

    #[test]
    fn manual_thinking_budget_stays_within_the_api_bounds() {
        let budget = |config: ThinkingConfig, max_tokens: u32| {
            build_thinking(Some(&config), max_tokens, true)
                .thinking
                .and_then(|t| t.budget_tokens)
        };
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::Low), 16384),
            Some(4096),
            "Low is a quarter of max_tokens"
        );
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::Medium), 16384),
            Some(8192),
            "Medium is half of max_tokens"
        );
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::High), 16384),
            Some(12288),
            "High is three quarters of max_tokens"
        );
        assert_eq!(
            budget(ThinkingConfig::Toggle(true), 16384),
            Some(8192),
            "an on toggle is half of max_tokens"
        );
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::Low), 2048),
            Some(1024),
            "the budget is never below 1024"
        );
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::High), 1025),
            Some(1024),
            "the budget stays below max_tokens"
        );
        assert_eq!(
            budget(ThinkingConfig::Level(ThinkingLevel::High), 1024),
            None,
            "no room for a budget means no thinking"
        );
    }

    #[tokio::test]
    async fn thinking_toggle_false_absent_from_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Toggle(false)),
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(
            result.is_ok(),
            "request with Toggle(false) should succeed: {result:?}"
        );

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert!(
            body.get("thinking").is_none(),
            "thinking should be absent from request body when Toggle(false)"
        );
    }

    #[test]
    fn second_system_message_becomes_user() {
        let messages = vec![
            Message::system("First instruction."),
            Message::system("Second instruction."),
            Message::user("Hello"),
        ];

        let (system, api_msgs) = AnthropicClient::convert_messages(&messages, true);
        assert_eq!(
            system.as_deref(),
            Some("First instruction."),
            "only first system message goes to system field"
        );
        // The extra system-as-user message and the real user message are
        // consecutive same-role messages, so merge_consecutive_messages
        // combines them into one.
        assert_eq!(api_msgs.len(), 1, "merged into single user message");
        let merged = api_msgs.first().unwrap();
        assert_eq!(merged.role, "user");
        let content_json = serde_json::to_value(&merged.content).unwrap();
        let blocks = content_json.as_array().unwrap();
        assert_eq!(blocks.len(), 2, "should have two content blocks");
        assert_eq!(
            blocks.first().unwrap().get("text").unwrap(),
            "System: Second instruction.",
            "first block is the converted system message"
        );
        assert_eq!(
            blocks.get(1).unwrap().get("text").unwrap(),
            "Hello",
            "second block is the original user message"
        );
    }

    #[tokio::test]
    async fn oauth_key_uses_bearer_auth_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        let client = AnthropicClient::new(
            http,
            server.uri(),
            "sk-ant-oat01-test-key",
            "claude-sonnet-4-20250514",
            1024,
            RetryConfig::no_retry(),
        );

        let result = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "OAuth request should succeed");

        let requests = server.received_requests().await.unwrap();
        let req = requests.first().unwrap();

        let auth = req
            .headers
            .iter()
            .find(|(name, _)| name.as_str().eq_ignore_ascii_case("authorization"))
            .map_or("", |(_, v)| v.to_str().unwrap_or(""));
        assert_eq!(
            auth, "Bearer sk-ant-oat01-test-key",
            "should use Bearer auth for OAuth key"
        );

        let beta = req
            .headers
            .iter()
            .find(|(name, _)| name.as_str().eq_ignore_ascii_case("anthropic-beta"))
            .map_or("", |(_, v)| v.to_str().unwrap_or(""));
        assert_eq!(
            beta, OAUTH_BETA,
            "should include anthropic-beta header for OAuth key"
        );

        let has_x_api_key = req
            .headers
            .iter()
            .any(|(name, _)| name.as_str().eq_ignore_ascii_case("x-api-key"));
        assert!(!has_x_api_key, "x-api-key should be absent for OAuth key");
    }

    #[tokio::test]
    async fn oauth_key_system_uses_blocks_with_identity_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        let client = AnthropicClient::new(
            http,
            server.uri(),
            "sk-ant-oat01-test-key",
            "claude-sonnet-4-20250514",
            1024,
            RetryConfig::no_retry(),
        );

        let messages = vec![Message::system("Be helpful."), Message::user("Hello")];
        let result = client
            .complete(&messages, &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "OAuth client should succeed");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        let system = body.get("system").unwrap();
        assert!(
            system.is_array(),
            "system should be an array of blocks for OAuth key"
        );
        let blocks = system.as_array().unwrap();
        assert_eq!(
            blocks.len(),
            2,
            "should have identity block + user system block"
        );
        assert_eq!(
            blocks
                .first()
                .unwrap()
                .get("text")
                .unwrap()
                .as_str()
                .unwrap(),
            OAUTH_IDENTITY,
            "first block should be OAuth identity"
        );
        assert_eq!(
            blocks
                .get(1)
                .unwrap()
                .get("text")
                .unwrap()
                .as_str()
                .unwrap(),
            "Be helpful.",
            "second block should be the user system message"
        );
    }

    // --- Thinking: request shape, fallback, replay ---

    fn thinking_options(max_tokens: u32) -> CompletionOptions {
        CompletionOptions {
            max_tokens: Some(max_tokens),
            thinking: Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
            ..CompletionOptions::default()
        }
    }

    fn adaptive_unsupported_body() -> Value {
        json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "adaptive thinking is not supported on this model"
            }
        })
    }

    #[tokio::test]
    async fn toggle_on_sends_adaptive_thinking_without_effort() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Toggle(true)),
            ..CompletionOptions::default()
        };
        client
            .complete(&simple_user_message(), &[], &options)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.first().unwrap().body).unwrap();
        assert_eq!(
            at(&body, "/thinking"),
            &json!({"type": "adaptive", "display": "summarized"}),
            "an on toggle asks for adaptive thinking"
        );
        assert!(
            body.get("output_config").is_none(),
            "an on toggle sets no effort: {body}"
        );
    }

    #[tokio::test]
    async fn effort_and_json_schema_share_output_config() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "output_config": {
                    "effort": "high",
                    "format": {"type": "json_schema"}
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = CompletionOptions {
            thinking: Some(ThinkingConfig::Level(ThinkingLevel::High)),
            response_format: ResponseFormat::JsonSchema {
                name: "answer".to_string(),
                schema: json!({"type": "object"}),
            },
            ..CompletionOptions::default()
        };
        let result = client.complete(&simple_user_message(), &[], &options).await;
        assert!(
            result.is_ok(),
            "effort and format are both sent: {result:?}"
        );
    }

    #[tokio::test]
    async fn refused_adaptive_thinking_falls_back_to_a_manual_budget_and_remembers_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "thinking": {"type": "adaptive"}
            })))
            .respond_with(ResponseTemplate::new(400).set_body_json(adaptive_unsupported_body()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "thinking": {"type": "enabled", "budget_tokens": 2048}
            })))
            .and(header("anthropic-beta", INTERLEAVED_THINKING_BETA))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let options = thinking_options(4096);
        client
            .complete(&simple_user_message(), &[], &options)
            .await
            .unwrap();
        client
            .complete(&simple_user_message(), &[], &options)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let types: Vec<String> = requests
            .iter()
            .map(|r| {
                let body: Value = serde_json::from_slice(&r.body).unwrap();
                at(&body, "/thinking/type").as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(
            types,
            vec!["adaptive", "enabled", "enabled"],
            "the refusal costs one extra request; the next call goes straight to a manual budget"
        );
        let manual: Value = serde_json::from_slice(&requests.get(2).unwrap().body).unwrap();
        assert!(
            manual.get("output_config").is_none(),
            "a manual budget sets no effort: {manual}"
        );
    }

    #[tokio::test]
    async fn refused_adaptive_thinking_with_no_room_for_a_budget_sends_no_thinking() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({
                "thinking": {"type": "adaptive"}
            })))
            .respond_with(ResponseTemplate::new(400).set_body_json(adaptive_unsupported_body()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_response_body()))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &thinking_options(1024))
            .await;
        assert!(result.is_ok(), "the request still succeeds: {result:?}");

        let requests = server.received_requests().await.unwrap();
        let retried: Value = serde_json::from_slice(&requests.get(1).unwrap().body).unwrap();
        assert!(
            retried.get("thinking").is_none(),
            "max_tokens of 1024 cannot hold a budget, so thinking is dropped: {retried}"
        );
    }

    #[tokio::test]
    async fn other_bad_requests_are_not_mistaken_for_refused_thinking() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": "max_tokens: too large"}
            })))
            .mount(&server)
            .await;

        let client = test_client(&server.uri());
        let result = client
            .complete(&simple_user_message(), &[], &thinking_options(4096))
            .await;
        assert!(result.is_err(), "the 400 surfaces");
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "an unrelated 400 is not retried as a manual-budget request"
        );
    }

    #[test]
    fn complete_requests_do_not_ask_for_a_stream() {
        let prepared = test_client("http://localhost").prepare(
            &simple_user_message(),
            &[],
            &CompletionOptions::default(),
        );
        let plain = serde_json::to_value(prepared.request("m", false, false)).unwrap();
        assert!(plain.get("stream").is_none(), "no stream flag: {plain}");
        let streaming = serde_json::to_value(prepared.request("m", true, false)).unwrap();
        assert_eq!(
            at(&streaming, "/stream"),
            &json!(true),
            "streaming asks for a stream"
        );
    }

    fn signed(text: &str, signature: &str) -> ThinkingBlock {
        ThinkingBlock {
            text: text.to_string(),
            signature: Some(signature.to_string()),
            ..ThinkingBlock::default()
        }
    }

    fn redacted(data: &str) -> ThinkingBlock {
        ThinkingBlock {
            redacted: Some(data.to_string()),
            ..ThinkingBlock::default()
        }
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "exec".to_string(),
            arguments: json!({"command": "ls"}),
            server: None,
        }
    }

    fn assistant_with_thinking(
        content: &str,
        calls: Option<Vec<ToolCall>>,
        thinking: Vec<ThinkingBlock>,
    ) -> Message {
        let mut message = Message::assistant(content, calls);
        message.thinking = thinking;
        message
    }

    /// The block types of each message's content, in order, comma-joined.
    fn block_types(messages: &[AnthropicMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|m| {
                let content = serde_json::to_value(&m.content).unwrap();
                content
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .map(|b| at(b, "/type").as_str().unwrap())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn current_tool_exchange_replays_its_thinking_first_and_unchanged() {
        let messages = vec![
            Message::user("list files"),
            assistant_with_thinking(
                "On it.",
                Some(vec![call("toolu_1")]),
                vec![signed("plan", "sig-1"), redacted("ENCRYPTED")],
            ),
            Message::tool("a.txt", "toolu_1"),
        ];
        let (_, api) = AnthropicClient::convert_messages(&messages, true);
        assert_eq!(
            block_types(&api).get(1).unwrap(),
            "thinking,redacted_thinking,text,tool_use",
            "thinking blocks lead the assistant message"
        );
        let blocks = serde_json::to_value(&api.get(1).unwrap().content).unwrap();
        assert_eq!(
            at(&blocks, "/0"),
            &json!({"type": "thinking", "thinking": "plan", "signature": "sig-1"}),
            "the signed block goes back with its signature"
        );
        assert_eq!(
            at(&blocks, "/1"),
            &json!({"type": "redacted_thinking", "data": "ENCRYPTED"}),
            "the redacted block goes back unchanged"
        );
    }

    #[test]
    fn thinking_before_the_last_plain_reply_is_not_replayed() {
        let messages = vec![
            Message::user("first"),
            assistant_with_thinking(
                "old answer",
                Some(vec![call("toolu_0")]),
                vec![signed("old plan", "sig-old")],
            ),
            Message::tool("done", "toolu_0"),
            assistant_with_thinking("final", None, vec![signed("wrap-up", "sig-final")]),
            Message::user("second"),
            assistant_with_thinking(
                "",
                Some(vec![call("toolu_1")]),
                vec![signed("new plan", "sig-new")],
            ),
            Message::tool("ok", "toolu_1"),
        ];
        let (_, api) = AnthropicClient::convert_messages(&messages, true);
        let types = block_types(&api);
        assert_eq!(
            types.get(1).unwrap(),
            "text,tool_use",
            "an earlier exchange's tool-call message loses its thinking"
        );
        assert_eq!(
            types.get(3).unwrap(),
            "text",
            "a plain reply never carries thinking back"
        );
        assert_eq!(
            types.get(5).unwrap(),
            "thinking,tool_use",
            "the exchange in progress keeps its thinking"
        );
    }

    #[test]
    fn every_assistant_message_of_the_current_exchange_replays_its_thinking() {
        let messages = vec![
            Message::user("go"),
            assistant_with_thinking("", Some(vec![call("t1")]), vec![signed("one", "s1")]),
            Message::tool("r1", "t1"),
            assistant_with_thinking("", Some(vec![call("t2")]), vec![signed("two", "s2")]),
            Message::tool("r2", "t2"),
        ];
        let (_, api) = AnthropicClient::convert_messages(&messages, true);
        let types = block_types(&api);
        assert_eq!(
            types.get(1).unwrap(),
            "thinking,tool_use",
            "step one replays"
        );
        assert_eq!(
            types.get(3).unwrap(),
            "thinking,tool_use",
            "step two replays"
        );
    }

    #[test]
    fn thinking_is_replayed_only_when_the_request_has_thinking_on() {
        let messages = vec![
            Message::user("go"),
            assistant_with_thinking("", Some(vec![call("t1")]), vec![signed("plan", "s1")]),
            Message::tool("r1", "t1"),
        ];
        let client = test_client("http://localhost");

        let on = client.prepare(&messages, &[], &thinking_options(4096));
        assert_eq!(
            block_types(&on.messages).get(1).unwrap(),
            "thinking,tool_use",
            "thinking blocks go back while thinking is on"
        );
        let off = client.prepare(&messages, &[], &CompletionOptions::default());
        assert_eq!(
            block_types(&off.messages).get(1).unwrap(),
            "tool_use",
            "with thinking off there is nothing to pair them with, so they are left out"
        );
    }

    #[test]
    fn thinking_blocks_the_api_would_reject_are_never_sent() {
        let foreign = ThinkingBlock {
            text: String::new(),
            signature: Some("gemini-signature".to_string()),
            part: Some("call_0".to_string()),
            ..ThinkingBlock::default()
        };
        let messages = vec![
            Message::user("go"),
            assistant_with_thinking(
                "",
                Some(vec![call("t1")]),
                vec![
                    ThinkingBlock::text("unsigned summary from another provider"),
                    signed("keeps a signature", ""),
                    foreign,
                    signed("", "sig-empty-text"),
                ],
            ),
            Message::tool("r1", "t1"),
        ];
        let (_, api) = AnthropicClient::convert_messages(&messages, true);
        let blocks = serde_json::to_value(&api.get(1).unwrap().content).unwrap();
        assert_eq!(
            blocks,
            json!([
                {"type": "thinking", "thinking": "", "signature": "sig-empty-text"},
                {"type": "tool_use", "id": "t1", "name": "exec", "input": {"command": "ls"}}
            ]),
            "only a block with a real signature is sent, and empty text with a signature is valid"
        );
    }

    #[test]
    fn non_streaming_response_captures_signature_and_redacted_blocks() {
        let response: AnthropicResponse = serde_json::from_value(json!({
            "content": [
                {"type": "thinking", "thinking": "step one", "signature": "sig-1"},
                {"type": "redacted_thinking", "data": "ENC"},
                {"type": "thinking", "thinking": "", "signature": "sig-2"},
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": "answer"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        }))
        .unwrap();
        let parsed = AnthropicClient::parse_response(response);
        assert_eq!(
            parsed.thinking,
            vec![
                signed("step one", "sig-1"),
                redacted("ENC"),
                signed("", "sig-2")
            ],
            "signed and redacted blocks are kept in order; a block with nothing in it is not"
        );
        assert_eq!(parsed.content, "answer", "text is unaffected");
    }

    // --- Streaming ---

    fn event(name: &str, data: impl Into<Value>) -> String {
        let data: Value = data.into();
        format!("event: {name}\ndata: {data}\n\n")
    }

    fn delta(index: usize, delta: impl Into<Value>) -> String {
        let delta: Value = delta.into();
        event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index, "delta": delta}),
        )
    }

    fn block_start(index: usize, block: impl Into<Value>) -> String {
        let block: Value = block.into();
        event(
            "content_block_start",
            json!({"type": "content_block_start", "index": index, "content_block": block}),
        )
    }

    fn block_stop(index: usize) -> String {
        event(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        )
    }

    fn message_start(usage: impl Into<Value>) -> String {
        let usage: Value = usage.into();
        event(
            "message_start",
            json!({"type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": "claude-test", "stop_reason": null, "usage": usage
            }}),
        )
    }

    fn message_end(stop_reason: &str, usage: impl Into<Value>) -> String {
        let usage: Value = usage.into();
        event(
            "message_delta",
            json!({"type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": usage}),
        ) + &event("message_stop", json!({"type": "message_stop"}))
    }

    /// A reply with thinking, text and a tool call, as the API streams it.
    fn full_stream() -> String {
        [
            message_start(json!({
                "input_tokens": 25, "output_tokens": 1, "cache_read_input_tokens": 3
            })),
            block_start(
                0,
                json!({"type": "thinking", "thinking": "", "signature": ""}),
            ),
            event("ping", json!({"type": "ping"})),
            delta(0, json!({"type": "thinking_delta", "thinking": "Let me "})),
            delta(0, json!({"type": "thinking_delta", "thinking": "think."})),
            delta(
                0,
                json!({"type": "signature_delta", "signature": "sig-abc"}),
            ),
            block_stop(0),
            block_start(1, json!({"type": "redacted_thinking", "data": "ENC"})),
            block_stop(1),
            block_start(2, json!({"type": "text", "text": ""})),
            delta(2, json!({"type": "text_delta", "text": "Hello w"})),
            delta(2, json!({"type": "text_delta", "text": "\u{f6}rld"})),
            delta(2, json!({"type": "citations_delta", "citation": {"x": 1}})),
            block_stop(2),
            block_start(
                3,
                json!({"type": "tool_use", "id": "toolu_1", "name": "exec", "input": {}}),
            ),
            delta(3, json!({"type": "input_json_delta", "partial_json": ""})),
            delta(
                3,
                json!({"type": "input_json_delta", "partial_json": "{\"comm"}),
            ),
            delta(
                3,
                json!({"type": "input_json_delta", "partial_json": "and\":\"ls\"}"}),
            ),
            block_stop(3),
            event("future_event", json!({"type": "future_event", "x": 1})),
            message_end("tool_use", json!({"output_tokens": 42})),
        ]
        .concat()
    }

    /// The same reply as `full_stream`, as a non-streaming response.
    fn full_response_body() -> Value {
        json!({
            "content": [
                {"type": "thinking", "thinking": "Let me think.", "signature": "sig-abc"},
                {"type": "redacted_thinking", "data": "ENC"},
                {"type": "text", "text": "Hello w\u{f6}rld"},
                {"type": "tool_use", "id": "toolu_1", "name": "exec", "input": {"command": "ls"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 25, "output_tokens": 42, "cache_read_input_tokens": 3}
        })
    }

    async fn stream_from(
        script: Vec<Step>,
        client: impl FnOnce(&str) -> AnthropicClient,
    ) -> (
        Result<InferenceResponse, InferenceError>,
        RecordingSink,
        ScriptedServer,
    ) {
        let server = ScriptedServer::start(vec![script]).await;
        let client = client(&server.uri());
        let sink = RecordingSink::default();
        let result = client
            .complete_streaming(
                &simple_user_message(),
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await;
        (result, sink, server)
    }

    #[tokio::test]
    async fn streamed_response_matches_the_non_streaming_one() {
        let (streamed, sink, server) =
            stream_from(sse_response(&[full_stream()]), test_client).await;
        let streamed = streamed.unwrap();

        let whole_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full_response_body()))
            .mount(&whole_server)
            .await;
        let whole = test_client(&whole_server.uri())
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_same_response(&streamed, &whole);
        assert_eq!(streamed.content, "Hello w\u{f6}rld", "text assembled");
        assert_eq!(
            streamed.thinking,
            vec![signed("Let me think.", "sig-abc"), redacted("ENC")],
            "thinking keeps its signature and the redacted block"
        );
        assert_eq!(streamed.tool_calls.len(), 1, "one tool call");
        assert_eq!(
            streamed.tool_calls.first().unwrap().arguments,
            json!({"command": "ls"}),
            "input fragments are joined and parsed once"
        );
        let usage = streamed.usage.unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens
            ),
            (25, 42, Some(3)),
            "message_start and message_delta usage merge, the later output count winning"
        );
        assert_eq!(
            streamed.stop_reason,
            Some(StopReason::ToolUse),
            "stop reason"
        );

        assert_eq!(
            sink.text(),
            "Hello w\u{f6}rld",
            "text deltas reach the sink"
        );
        assert_eq!(
            sink.thinking(),
            "Let me think.",
            "thinking deltas reach the sink"
        );
        assert!(
            !sink.deltas().contains(&StreamDelta::Restart),
            "a clean stream never restarts"
        );

        let requests = server.requests();
        let request = requests.first().unwrap();
        let request_body = request.json();
        assert_eq!(request.target, "/v1/messages", "the messages endpoint");
        assert_eq!(
            request.header("x-api-key"),
            Some("test-api-key"),
            "the key is sent as for any request"
        );
        assert_eq!(
            at(&request_body, "/stream"),
            &json!(true),
            "the request asks for a stream"
        );
        assert!(
            request
                .headers
                .iter()
                .all(|(name, _)| name != "anthropic-beta"),
            "no beta header is needed without manual thinking"
        );
    }

    #[tokio::test]
    async fn stream_parses_at_every_chunk_boundary_including_inside_characters() {
        let stream = full_stream();
        let whole = {
            let (result, _, _) =
                stream_from(sse_response(std::slice::from_ref(&stream)), test_client).await;
            result.unwrap()
        };
        for size in [1, 2, 3, 7, 64] {
            let chunks = split_bytes(&stream, size);
            let (result, sink, _) = stream_from(sse_response(&chunks), test_client).await;
            let streamed = result.unwrap();
            assert_same_response(&streamed, &whole);
            assert_eq!(
                sink.text(),
                "Hello w\u{f6}rld",
                "chunks of {size} bytes must not corrupt text"
            );
        }
    }

    #[tokio::test]
    async fn stream_without_message_stop_is_an_interrupted_stream() {
        let partial = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "half an ans"})),
        ]
        .concat();

        let (cut_clean, _, _) =
            stream_from(sse_response(std::slice::from_ref(&partial)), test_client).await;
        let err = cut_clean.unwrap_err();
        assert!(
            matches!(err, InferenceError::StreamInterrupted(_)),
            "a stream that ends cleanly but early is interrupted: {err:?}"
        );
        assert!(err.is_retryable(), "an interrupted stream is retried");

        let (dropped, sink, _) = stream_from(sse_chunks(&[partial]), test_client).await;
        let dropped_err = dropped.unwrap_err();
        assert!(
            matches!(dropped_err, InferenceError::StreamInterrupted(_)),
            "a body cut off mid-stream is interrupted: {dropped_err:?}"
        );
        assert!(
            sink.deltas().contains(&StreamDelta::Restart),
            "the half-streamed answer is voided when the call fails"
        );
    }

    #[tokio::test]
    async fn in_stream_overloaded_error_is_retryable() {
        let events = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            event(
                "error",
                json!({"type": "error",
                    "error": {"type": "overloaded_error", "message": "Overloaded"}}),
            ),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[events]), test_client).await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, InferenceError::Api(message) if message.contains("overloaded_error")),
            "the error event surfaces with its type: {err:?}"
        );
        assert!(err.is_retryable(), "overloaded_error is retryable");
    }

    #[tokio::test]
    async fn in_stream_invalid_request_error_is_not_retryable() {
        let events = event(
            "error",
            json!({"type": "error",
                "error": {"type": "invalid_request_error", "message": "bad input"}}),
        );
        let (result, _, _) = stream_from(sse_response(&[events]), test_client).await;
        let err = result.unwrap_err();
        assert!(
            !err.is_retryable(),
            "a rejected request stays rejected: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_errors_before_the_stream_surface_like_non_streaming_ones() {
        let script = json_response(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        );
        let (result, sink, _) = stream_from(script, test_client).await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("429") && err.to_string().contains("slow down"),
            "status and message are kept: {err}"
        );
        assert!(err.is_retryable(), "429 is retryable");
        assert!(sink.deltas().is_empty(), "nothing was streamed");
    }

    #[tokio::test]
    async fn stalled_stream_fails_after_the_idle_timeout_not_a_total_one() {
        let started = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "partial"})),
        ]
        .concat();
        let mut script = sse_chunks(&[started]);
        script.push(Step::pause(std::time::Duration::from_secs(5)));

        let (result, sink, _) = stream_from(script, |url| {
            let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(1)).unwrap();
            AnthropicClient::new(
                http,
                url,
                "test-api-key",
                "claude-test",
                1024,
                RetryConfig::no_retry(),
            )
        })
        .await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Stalled(1)),
            "no bytes for the configured time is a stall: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "the model stopped responding for 1s",
            "the message is plain"
        );
        assert_eq!(sink.text(), "", "what had streamed is voided");
    }

    #[tokio::test]
    async fn long_stream_that_keeps_producing_is_not_cut_off_by_the_timeout() {
        let mut script = vec![Step::head(200, "text/event-stream")];
        script.push(Step::chunk(message_start(
            json!({"input_tokens": 5, "output_tokens": 1}),
        )));
        script.push(Step::chunk(block_start(
            0,
            json!({"type": "text", "text": ""}),
        )));
        for word in ["one ", "two ", "three ", "four "] {
            script.push(Step::pause(std::time::Duration::from_millis(600)));
            script.push(Step::chunk(delta(
                0,
                json!({"type": "text_delta", "text": word}),
            )));
        }
        script.push(Step::chunk(block_stop(0)));
        script.push(Step::chunk(message_end(
            "end_turn",
            json!({"output_tokens": 9}),
        )));
        script.push(Step::end());

        let (result, sink, _) = stream_from(script, |url| {
            let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(1)).unwrap();
            AnthropicClient::new(
                http,
                url,
                "test-api-key",
                "claude-test",
                1024,
                RetryConfig::no_retry(),
            )
        })
        .await;
        let response = result.unwrap();
        assert_eq!(
            response.content, "one two three four ",
            "a stream longer than the timeout completes while bytes keep arriving"
        );
        assert_eq!(sink.text(), "one two three four ", "and streams all of it");
    }

    #[tokio::test]
    async fn retry_after_partial_output_restarts_the_stream() {
        let first = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "par"})),
            event(
                "error",
                json!({"type": "error",
                    "error": {"type": "overloaded_error", "message": "Overloaded"}}),
            ),
        ]
        .concat();
        let second = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "whole answer"})),
            block_stop(0),
            message_end("end_turn", json!({"output_tokens": 3})),
        ]
        .concat();
        let server =
            ScriptedServer::start(vec![sse_response(&[first]), sse_response(&[second])]).await;
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        let client = AnthropicClient::new(
            http,
            server.uri(),
            "test-api-key",
            "claude-test",
            1024,
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
                &simple_user_message(),
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await
            .unwrap();

        assert_eq!(
            response.content, "whole answer",
            "the retry's answer is used"
        );
        assert_eq!(
            sink.deltas(),
            vec![
                StreamDelta::Text("par".to_string()),
                StreamDelta::Restart,
                StreamDelta::Text("whole answer".to_string()),
            ],
            "the partial text is voided before the retry streams"
        );
        assert_eq!(server.requests().len(), 2, "the request was sent twice");
    }

    #[tokio::test]
    async fn retry_before_any_output_does_not_restart() {
        let ok = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "fine"})),
            block_stop(0),
            message_end("end_turn", json!({"output_tokens": 3})),
        ]
        .concat();
        let server = ScriptedServer::start(vec![
            json_response(
                529,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            ),
            sse_response(&[ok]),
        ])
        .await;
        let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(5)).unwrap();
        let client = AnthropicClient::new(
            http,
            server.uri(),
            "test-api-key",
            "claude-test",
            1024,
            RetryConfig {
                max_retries: 1,
                initial_delay: std::time::Duration::from_millis(5),
                max_delay: std::time::Duration::from_millis(5),
                backoff_multiplier: 1.0,
            },
        );
        let sink = RecordingSink::default();
        client
            .complete_streaming(
                &simple_user_message(),
                &[],
                &CompletionOptions::default(),
                &sink,
            )
            .await
            .unwrap();
        assert_eq!(
            sink.deltas(),
            vec![StreamDelta::Text("fine".to_string())],
            "nothing had streamed, so there was nothing to void"
        );
    }

    #[tokio::test]
    async fn tool_input_cut_off_by_max_tokens_drops_that_call_and_reports_truncation() {
        let events = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(0, json!({"type": "text", "text": ""})),
            delta(0, json!({"type": "text_delta", "text": "Running it"})),
            block_stop(0),
            block_start(
                1,
                json!({"type": "tool_use", "id": "toolu_1", "name": "exec", "input": {}}),
            ),
            delta(
                1,
                json!({"type": "input_json_delta", "partial_json": "{\"command\": \"l"}),
            ),
            block_stop(1),
            message_end("max_tokens", json!({"output_tokens": 1024})),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[events]), test_client).await;
        let response = result.unwrap();
        assert!(
            response.tool_calls.is_empty(),
            "the half-written call is dropped"
        );
        assert!(
            response.was_truncated(),
            "the stop reason tells the caller why"
        );
        assert_eq!(response.content, "Running it", "the text before it is kept");
    }

    #[tokio::test]
    async fn malformed_tool_input_without_truncation_is_a_parse_error() {
        let events = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(
                0,
                json!({"type": "tool_use", "id": "toolu_1", "name": "exec", "input": {}}),
            ),
            delta(
                0,
                json!({"type": "input_json_delta", "partial_json": "{not json"}),
            ),
            block_stop(0),
            message_end("tool_use", json!({"output_tokens": 4})),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[events]), test_client).await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, InferenceError::Parse(m) if m.contains("exec")),
            "the tool is named: {err:?}"
        );
        assert!(!err.is_retryable(), "garbage input will not improve");
    }

    #[tokio::test]
    async fn tool_call_with_no_arguments_keeps_its_empty_input() {
        let events = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(
                0,
                json!({"type": "tool_use", "id": "toolu_1", "name": "now", "input": {}}),
            ),
            delta(0, json!({"type": "input_json_delta", "partial_json": ""})),
            block_stop(0),
            message_end("tool_use", json!({"output_tokens": 4})),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[events]), test_client).await;
        let response = result.unwrap();
        assert_eq!(
            response.tool_calls.first().unwrap().arguments,
            json!({}),
            "an input that streams as only an empty fragment is the empty object"
        );
    }

    #[tokio::test]
    async fn streamed_server_tool_use_and_search_results_do_not_become_tool_calls() {
        let events = [
            message_start(json!({"input_tokens": 5, "output_tokens": 1})),
            block_start(
                0,
                json!({"type": "server_tool_use", "id": "srvtoolu_1",
                    "name": "web_search", "input": {}}),
            ),
            delta(
                0,
                json!({"type": "input_json_delta", "partial_json": "{\"query\":\"rust\"}"}),
            ),
            block_stop(0),
            block_start(
                1,
                json!({"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
                    "content": [{"type": "web_search_result", "url": "https://example.com"}]}),
            ),
            block_stop(1),
            block_start(2, json!({"type": "text", "text": ""})),
            delta(2, json!({"type": "text_delta", "text": "Found it."})),
            block_stop(2),
            block_start(3, json!({"type": "mystery_block", "x": 1})),
            delta(3, json!({"type": "text_delta", "text": "ignored"})),
            block_stop(3),
            message_end("end_turn", json!({"output_tokens": 8})),
        ]
        .concat();
        let (result, sink, _) = stream_from(sse_response(&[events]), test_client).await;
        let response = result.unwrap();
        assert_eq!(
            response.content, "Found it.",
            "only the text block is content"
        );
        assert!(
            response.tool_calls.is_empty(),
            "server tools are not our calls"
        );
        assert_eq!(sink.text(), "Found it.", "nothing else is streamed as text");
    }

    #[tokio::test]
    async fn complete_still_sends_a_plain_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full_response_body()))
            .mount(&server)
            .await;
        let client = test_client(&server.uri());
        let response = client
            .complete(&simple_user_message(), &[], &CompletionOptions::default())
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(
            &server
                .received_requests()
                .await
                .unwrap()
                .first()
                .unwrap()
                .body,
        )
        .unwrap();
        assert!(
            body.get("stream").is_none(),
            "complete does not stream: {body}"
        );
        assert_eq!(
            response.thinking.len(),
            2,
            "thinking comes through complete too"
        );
    }
}

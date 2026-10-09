//! Google Gemini API provider implementation.
//!
//! Uses the Gemini `generateContent` REST API, and `streamGenerateContent`
//! for streaming. Authentication is via an API key passed as a query
//! parameter. System messages are extracted and sent as the top-level
//! `systemInstruction` field. Tool results are sent as `functionResponse`
//! parts in user-role messages.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::inference::embedding::{EmbeddingProvider, EmbeddingResponse};
use crate::inference::http::{
    SharedHttpClient, map_request_error, map_stream_request_error, read_error_body,
    warn_if_insecure_remote,
};
use crate::inference::retry::{RetryConfig, with_retry};
use crate::inference::stream::{Flow, SseEvent, TrackedSink, answered_whole, read_sse};
use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ResponseFormat, Role, StopReason, StreamDelta, StreamSink, ThinkingBlock, ThinkingConfig,
    ThinkingLevel, ToolCall, ToolDefinition, Usage,
};

/// The `part` of a [`ThinkingBlock`] whose signature belongs to the
/// response's text rather than to a tool call.
const TEXT_PART: &str = "text";

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

    /// Parse a successful Gemini response into our generic `InferenceResponse`.
    ///
    /// Reasoning summaries (parts marked `thought`) are kept apart from the
    /// answer, and each thought signature is recorded with the part it came
    /// on so it can be sent back there.
    fn parse_response(
        gemini_response: GeminiResponse,
    ) -> Result<InferenceResponse, InferenceError> {
        let candidate = gemini_response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| {
                InferenceError::Parse("Gemini API response contained no candidates".to_string())
            })?;

        let mut content_text = String::new();
        let mut thinking: Vec<ThinkingBlock> = Vec::new();
        // The summary block that further summary text can still be added to:
        // a summary arrives in pieces, until a part that signs it.
        let mut open_summary: Option<usize> = None;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let finish_reason = candidate.finish_reason;

        for (idx, part) in candidate.content.parts.into_iter().enumerate() {
            let signature = part.thought_signature.filter(|s| !s.is_empty());
            if let Some(function_call) = part.function_call {
                // Gemini does not return IDs for function calls; synthesize them.
                let id = format!("call_{}", tool_calls.len());
                if let Some(signature) = signature {
                    thinking.push(ThinkingBlock {
                        signature: Some(signature),
                        part: Some(id.clone()),
                        ..ThinkingBlock::default()
                    });
                }
                open_summary = None;
                tool_calls.push(ToolCall {
                    id,
                    name: function_call.name,
                    arguments: function_call.args,
                    server: None,
                });
            } else if let Some(text) = part.text {
                if part.thought == Some(true) {
                    let index = open_summary.unwrap_or_else(|| {
                        thinking.push(ThinkingBlock::default());
                        thinking.len() - 1
                    });
                    open_summary = Some(index);
                    if let Some(block) = thinking.get_mut(index) {
                        block.text.push_str(&text);
                        if signature.is_some() {
                            block.signature = signature;
                            block.part = Some(TEXT_PART.to_string());
                            open_summary = None;
                        }
                    }
                } else {
                    content_text.push_str(&text);
                    open_summary = None;
                    if let Some(signature) = signature {
                        thinking.push(text_signature(signature));
                    }
                }
            } else if let Some(signature) = signature {
                open_summary = None;
                thinking.push(text_signature(signature));
            } else if part.function_response.is_some() {
                debug!(
                    part_index = idx,
                    "unexpected functionResponse part in Gemini model output"
                );
            } else if part.inline_data.is_some() {
                debug!(
                    part_index = idx,
                    "unexpected inlineData part in Gemini model output"
                );
            }
        }

        // Thought tokens are billed as output but reported apart from the
        // candidates' own, so they are added in, as Anthropic's and
        // OpenAI's output counts already include theirs.
        let usage = gemini_response.usage_metadata.map(|u| Usage {
            input_tokens: u.prompt_token_count,
            output_tokens: u
                .candidates_token_count
                .saturating_add(u.thoughts_token_count),
            cache_creation_tokens: None,
            cache_read_tokens: u.cached_content_token_count,
        });

        let mut model_response = InferenceResponse::new(content_text, tool_calls);
        model_response.usage = usage;
        model_response.thinking = thinking;
        model_response.stop_reason = finish_reason.as_deref().map(map_stop_reason);
        Ok(model_response)
    }

    /// Build the full endpoint URL with the API key query parameter.
    fn endpoint(&self) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        )
    }

    /// Build the streaming endpoint URL, which answers with server-sent events.
    fn stream_endpoint(&self) -> String {
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, self.model, self.api_key
        )
    }

    /// Convert generic messages into Gemini API format.
    ///
    /// System messages are extracted and returned separately; Gemini uses a
    /// top-level `systemInstruction` field rather than including system content
    /// in the `contents` array. Multiple system messages are concatenated.
    ///
    /// Tool result messages (`Role::Tool`) become `functionResponse` parts of a
    /// user-role message, one message for a run of results, named for the
    /// function whose call they answer. An assistant message goes back as the
    /// model sent it: its thought signatures on the parts they came with, and
    /// its function calls in their original order.
    fn convert_messages(
        messages: &[Message],
    ) -> (Option<GeminiSystemInstruction>, Vec<GeminiContent>) {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut contents: Vec<GeminiContent> = Vec::new();
        // The function names of the latest assistant message's tool calls, by
        // id. Ids are synthesized per response, so a result is only matched
        // against the calls just before it.
        let mut call_names: HashMap<&str, &str> = HashMap::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    if system_parts.is_empty() {
                        system_parts.push(&msg.content);
                    } else {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![GeminiPart::text(format!("System: {}", msg.content))],
                        });
                    }
                }
                Role::User => {
                    if msg.images.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![GeminiPart::text(msg.content.clone())],
                        });
                    } else {
                        let mut parts: Vec<GeminiPart> = Vec::new();

                        if !msg.content.is_empty() {
                            parts.push(GeminiPart::text(msg.content.clone()));
                        }

                        for img in &msg.images {
                            parts.push(GeminiPart {
                                inline_data: Some(GeminiInlineData {
                                    mime_type: img.media_type.clone(),
                                    data: img.data.clone(),
                                }),
                                ..GeminiPart::default()
                            });
                        }

                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts,
                        });
                    }
                }
                Role::Assistant => {
                    call_names = msg
                        .tool_calls
                        .iter()
                        .flatten()
                        .map(|tc| (tc.id.as_str(), tc.name.as_str()))
                        .collect();
                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts: model_parts(msg),
                    });
                }
                Role::Tool => {
                    // Gemini expects functionResponse in a user-role message.
                    // The response must be a JSON object; wrap plain strings.
                    // The results of one round of calls share one message: the
                    // API wants as many response parts as there were calls.
                    let call_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                    let part = GeminiPart {
                        function_response: Some(GeminiFunctionResponse {
                            name: call_names
                                .get(call_id)
                                .copied()
                                .unwrap_or(call_id)
                                .to_string(),
                            response: serde_json::json!({ "result": msg.content }),
                        }),
                        ..GeminiPart::default()
                    };
                    match contents.last_mut() {
                        Some(last)
                            if last.role == "user"
                                && last.parts.iter().all(|p| p.function_response.is_some()) =>
                        {
                            last.parts.push(part);
                        }
                        _ => contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: vec![part],
                        }),
                    }
                }
            }
        }

        let system_instruction = (!system_parts.is_empty()).then(|| GeminiSystemInstruction {
            parts: vec![GeminiPart::text(system_parts.join("\n\n"))],
        });

        (system_instruction, contents)
    }

    /// Build the request body, which stays the same across retries.
    fn prepare(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> GeminiRequest {
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

        let (response_mime_type, response_schema) = match &options.response_format {
            ResponseFormat::Text => (None, None),
            ResponseFormat::JsonSchema { schema, .. } => (
                Some("application/json".to_string()),
                Some(strip_unsupported_schema_fields(schema.clone())),
            ),
        };

        // Thinking summaries are only returned when asked for.
        let thinking_config = options.thinking.as_ref().and_then(|tc| {
            let thinking_budget = match tc {
                ThinkingConfig::Level(ThinkingLevel::Low) => 1024,
                ThinkingConfig::Level(ThinkingLevel::Medium) => 8192,
                ThinkingConfig::Level(ThinkingLevel::High) => 32768,
                ThinkingConfig::Toggle(true) => GEMINI_THINKING_BUDGET_DYNAMIC,
                ThinkingConfig::Toggle(false) => return None,
            };
            Some(GeminiThinkingConfig {
                thinking_budget,
                include_thoughts: true,
            })
        });

        GeminiRequest {
            contents,
            system_instruction,
            tools: gemini_tools,
            generation_config: GeminiGenerationConfig {
                max_output_tokens: options.max_tokens.unwrap_or(self.max_tokens),
                response_mime_type,
                response_schema,
                temperature: options.temperature,
                thinking_config,
            },
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
        let request = self.prepare(messages, tools, options);
        let tracked = sink.map(TrackedSink::new);

        with_retry(&self.retry, || async {
            let result = self.send(&request, tracked.as_ref()).await;
            if result.is_err()
                && let Some(tracked) = &tracked
            {
                tracked.restart_if_needed();
            }
            result
        })
        .await
    }

    #[tracing::instrument(skip_all, fields(
        model = %self.model,
        message_count = request.contents.len(),
        tool_count = request.tools.as_ref().map_or(0, Vec::len),
        streaming = sink.is_some(),
    ))]
    async fn send(
        &self,
        request: &GeminiRequest,
        sink: Option<&TrackedSink<'_>>,
    ) -> Result<InferenceResponse, InferenceError> {
        let timeout_secs = self.http.timeout_secs();
        let request_json = serde_json::to_string(request)
            .map_err(|e| InferenceError::Parse(format!("failed to serialize request: {e}")))?;

        debug!(
            max_output_tokens = request.generation_config.max_output_tokens,
            message_count = request.contents.len(),
            tool_count = request.tools.as_ref().map_or(0, Vec::len),
            "sending gemini generateContent request"
        );

        let (client, url) = if sink.is_some() {
            (self.http.streaming_client(), self.stream_endpoint())
        } else {
            (self.http.client(), self.endpoint())
        };
        let response = client
            .post(url)
            .body(request_json.clone())
            .header("content-type", "application/json")
            .send()
            .await
            .map_err(|e| {
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
                "gemini API error — full request/response for diagnosis"
            );
            let error_body = serde_json::from_str::<GeminiErrorResponse>(&raw_body)
                .map_or_else(|_| raw_body, |e| e.error.message);
            return Err(InferenceError::Api(format!("{status}: {error_body}")));
        }

        let gemini_response = if let Some(sink) = sink.filter(|_| !answered_whole(&response)) {
            read_stream(response, timeout_secs, sink).await?
        } else {
            let text = response
                .text()
                .await
                .map_err(|e| map_request_error(e, timeout_secs))?;
            serde_json::from_str(&text).map_err(|e| {
                InferenceError::Parse(format!("failed to parse gemini response: {e}"))
            })?
        };
        let result = Self::parse_response(gemini_response)?;
        info!(
            model = %self.model,
            content_len = result.content.len(),
            tool_calls = result.tool_calls.len(),
            "gemini completion received"
        );
        Ok(result)
    }
}

/// A block recording a signature that belongs to the response's text.
fn text_signature(signature: String) -> ThinkingBlock {
    ThinkingBlock {
        signature: Some(signature),
        part: Some(TEXT_PART.to_string()),
        ..ThinkingBlock::default()
    }
}

/// The parts an assistant message goes back as: its text, then its function
/// calls in order, each carrying the thought signature Gemini attached to it.
///
/// Gemini 3 refuses a request whose function calls lost their signatures, and
/// recommends sending the others back too.
fn model_parts(msg: &Message) -> Vec<GeminiPart> {
    let signature_for = |part: &str| {
        msg.thinking
            .iter()
            .rev()
            .find(|block| block.part.as_deref() == Some(part))
            .and_then(|block| block.signature.clone())
    };

    let mut parts: Vec<GeminiPart> = Vec::new();
    let text_signature = signature_for(TEXT_PART);
    if !msg.content.is_empty() || text_signature.is_some() {
        // A signature can arrive on a part with no text of its own.
        parts.push(GeminiPart {
            text: Some(msg.content.clone()),
            thought_signature: text_signature,
            ..GeminiPart::default()
        });
    }
    for tc in msg.tool_calls.iter().flatten() {
        parts.push(GeminiPart {
            function_call: Some(GeminiFunctionCall {
                name: tc.name.clone(),
                args: tc.arguments.clone(),
            }),
            thought_signature: signature_for(&tc.id),
            ..GeminiPart::default()
        });
    }
    parts
}

#[async_trait]
impl InferenceProvider for GeminiClient {
    #[tracing::instrument(skip_all, fields(model = %self.model, message_count = messages.len(), tool_count = tools.len()))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        self.run(messages, tools, options, None).await
    }

    /// Like `complete`, streaming the response's text and reasoning summary
    /// into `sink` as they arrive.
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

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Read a streamed response to its end, pushing text and reasoning summary to
/// `sink` as they arrive, and return it as the response `generateContent`
/// would have returned.
async fn read_stream(
    response: reqwest::Response,
    idle_secs: u64,
    sink: &dyn StreamSink,
) -> Result<GeminiResponse, InferenceError> {
    let mut assembler = StreamAssembler {
        sink,
        parts: Vec::new(),
        finish_reason: None,
        usage: None,
        saw_candidate: false,
    };
    read_sse(response, idle_secs, |event| assembler.handle(&event)).await?;
    assembler.finish()
}

/// One streamed event: the next piece of the response.
#[derive(Deserialize)]
struct GeminiStreamEvent {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
    /// A failure reported in the stream after the response began.
    #[serde(default)]
    error: Option<GeminiStreamError>,
}

#[derive(Deserialize)]
struct GeminiStreamError {
    #[serde(default)]
    code: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: String,
}

struct StreamAssembler<'a> {
    sink: &'a dyn StreamSink,
    parts: Vec<GeminiPart>,
    finish_reason: Option<String>,
    usage: Option<GeminiUsageMetadata>,
    saw_candidate: bool,
}

impl StreamAssembler<'_> {
    fn handle(&mut self, event: &SseEvent) -> Result<Flow, InferenceError> {
        let parsed: GeminiStreamEvent = serde_json::from_str(&event.data).map_err(|e| {
            InferenceError::Parse(format!("failed to parse gemini stream event: {e}"))
        })?;
        if let Some(error) = parsed.error {
            let code = error.code.map(|c| c.to_string()).unwrap_or_default();
            let status = error.status.unwrap_or_default();
            return Err(InferenceError::Api(format!(
                "{} {}: {}",
                code, status, error.message
            )));
        }
        if parsed.usage_metadata.is_some() {
            self.usage = parsed.usage_metadata;
        }
        // Only the first candidate is used: a request never asks for more.
        if let Some(candidate) = parsed.candidates.into_iter().next() {
            self.saw_candidate = true;
            for part in candidate.content.parts {
                if let Some(text) = &part.text {
                    self.sink.push(if part.thought == Some(true) {
                        StreamDelta::Thinking(text.clone())
                    } else {
                        StreamDelta::Text(text.clone())
                    });
                }
                self.parts.push(part);
            }
            if candidate.finish_reason.is_some() {
                self.finish_reason = candidate.finish_reason;
            }
        }
        Ok(Flow::Continue)
    }

    /// Assemble the response `generateContent` would have returned. Fails
    /// when the stream stopped short of a `finishReason`.
    fn finish(self) -> Result<GeminiResponse, InferenceError> {
        if self.saw_candidate && self.finish_reason.is_none() {
            return Err(InferenceError::StreamInterrupted(
                "the stream ended before the response was complete".to_string(),
            ));
        }
        // With no candidate at all (a blocked prompt, say) the parse step
        // reports it, as it does for a response that arrives whole.
        let candidates = if self.saw_candidate {
            vec![GeminiCandidate {
                content: GeminiResponseContent { parts: self.parts },
                finish_reason: self.finish_reason,
            }]
        } else {
            Vec::new()
        };
        Ok(GeminiResponse {
            candidates,
            usage_metadata: self.usage,
        })
    }
}

// ---------------------------------------------------------------------------
// Gemini API request types
// ---------------------------------------------------------------------------

/// Gemini's sentinel value for "dynamic" thinking budget (let the model decide).
const GEMINI_THINKING_BUDGET_DYNAMIC: i32 = -1;

#[derive(Serialize, Clone)]
struct GeminiThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    thinking_budget: i32,
    /// Ask for a summary of the reasoning to come back with the answer.
    #[serde(rename = "includeThoughts")]
    include_thoughts: bool,
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

/// One part of a message. A part carries one kind of content, so every
/// field is optional; a response part that is a reasoning summary is told
/// apart from the answer by `thought`.
#[derive(Serialize, Deserialize, Clone, Default)]
struct GeminiPart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// The text is a summary of the model's reasoning, not part of its answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    /// Proof of the reasoning behind this part, to be sent back with it.
    #[serde(
        default,
        rename = "thoughtSignature",
        skip_serializing_if = "Option::is_none"
    )]
    thought_signature: Option<String>,
    #[serde(
        default,
        rename = "functionCall",
        skip_serializing_if = "Option::is_none"
    )]
    function_call: Option<GeminiFunctionCall>,
    #[serde(
        default,
        rename = "functionResponse",
        skip_serializing_if = "Option::is_none"
    )]
    function_response: Option<GeminiFunctionResponse>,
    #[serde(
        default,
        rename = "inlineData",
        skip_serializing_if = "Option::is_none"
    )]
    inline_data: Option<GeminiInlineData>,
}

impl GeminiPart {
    fn text(text: String) -> Self {
        Self {
            text: Some(text),
            ..Self::default()
        }
    }
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
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

// ---------------------------------------------------------------------------
// Gemini API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    /// Absent when the candidate was stopped before producing anything.
    #[serde(default)]
    content: GeminiResponseContent,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}

/// Map Gemini's `finishReason` to the provider-agnostic [`StopReason`].
fn map_stop_reason(raw: &str) -> StopReason {
    match raw {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            StopReason::ContentFilter
        }
        other => StopReason::Other(other.to_string()),
    }
}

#[derive(Deserialize, Default)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
#[expect(clippy::struct_field_names, reason = "field names match Gemini API")]
struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: u32,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: u32,
    #[serde(default, rename = "thoughtsTokenCount")]
    thoughts_token_count: u32,
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

    async fn embed_single(&self, text: String) -> Result<EmbeddingResponse, InferenceError> {
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
                        InferenceError::Parse(format!("failed to parse gemini embed response: {e}"))
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

    async fn embed_batch(
        &self,
        owned_texts: Vec<String>,
    ) -> Result<EmbeddingResponse, InferenceError> {
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
                        InferenceError::Parse(format!(
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
    #[tracing::instrument(skip_all, fields(model = %self.model, count = texts.len()))]
    async fn embed(&self, texts: &[&str]) -> Result<EmbeddingResponse, InferenceError> {
        if texts.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: Vec::new(),
                dimensions: 0,
            });
        }

        if let [text] = texts {
            self.embed_single((*text).to_string()).await
        } else {
            let owned_texts: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();
            self.embed_batch(owned_texts).await
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Parse a Gemini error response into a `InferenceError::Api`.
async fn parse_gemini_embed_error(response: reqwest::Response) -> InferenceError {
    let status = response.status();
    let raw_body = read_error_body(response).await;
    let error_body = serde_json::from_str::<GeminiErrorResponse>(&raw_body)
        .map_or_else(|_| raw_body, |e| e.error.message);
    InferenceError::Api(format!("{status}: {error_body}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::CompletionOptions;
    use crate::inference::retry::RetryConfig;
    use crate::inference::test_support::{
        RecordingSink, ScriptedServer, Step, assert_same_response, at, json_response, split_bytes,
        sse_chunks, sse_response,
    };
    use crate::inference::{StreamDelta, ThinkingBlock};
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(base_url: &str) -> GeminiClient {
        let http =
            SharedHttpClient::new(&crate::inference::http::HttpClientConfig::with_timeout(60))
                .unwrap();
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
            SharedHttpClient::new(&crate::inference::http::HttpClientConfig::with_timeout(60))
                .unwrap();
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
        assert_eq!(
            first_part.text.as_deref(),
            Some("You are helpful."),
            "system part should be the system text"
        );

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
        let function_response = part
            .function_response
            .as_ref()
            .expect("part should be functionResponse");
        assert_eq!(
            function_response.response,
            serde_json::json!({"result": "command output"}),
            "response should wrap content"
        );
    }

    #[test]
    fn convert_messages_assistant_with_tool_calls() {
        let messages = vec![Message::assistant(
            "thinking",
            Some(vec![ToolCall {
                id: "call_0".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
                server: None,
            }]),
        )];
        let (_, contents) = GeminiClient::convert_messages(&messages);

        let entry = contents.first().unwrap();
        assert_eq!(entry.role, "model", "assistant maps to model role");
        assert_eq!(entry.parts.len(), 2, "text + function call parts");
        assert!(
            entry.parts.first().is_some_and(|p| p.text.is_some()),
            "first part should be text"
        );
        assert!(
            entry
                .parts
                .get(1)
                .is_some_and(|p| p.function_call.is_some()),
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
        assert_eq!(
            response.stop_reason,
            Some(StopReason::EndTurn),
            "STOP should map to StopReason::EndTurn"
        );
        assert!(!response.was_truncated());
    }

    #[tokio::test]
    async fn stop_reason_max_tokens_maps_to_truncation() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "cut off mid-sen"}]
                    },
                    "finishReason": "MAX_TOKENS"
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 1024,
                    "totalTokenCount": 1034
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

        assert_eq!(response.stop_reason, Some(StopReason::MaxTokens));
        assert!(
            response.was_truncated(),
            "MAX_TOKENS must be reported as a truncation"
        );
    }

    #[tokio::test]
    async fn stop_reason_missing_from_response_is_none() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "hi"}]
                    }
                }]
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

        assert_eq!(
            response.stop_reason, None,
            "a response with no finishReason field must parse, not error"
        );
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
        assert!(
            matches!(err, InferenceError::Api(_)),
            "should be an Api error"
        );
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
            matches!(err, InferenceError::Parse(_)),
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
            SharedHttpClient::new(&crate::inference::http::HttpClientConfig::with_timeout(1))
                .unwrap();
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
            matches!(result.unwrap_err(), InferenceError::Timeout(1)),
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
                "generationConfig": {
                    "thinkingConfig": {
                        "thinkingBudget": 8192,
                        "includeThoughts": true
                    }
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

    // --- Thinking, signatures and replay ---

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({"command": "ls"}),
            server: None,
        }
    }

    fn signature_block(signature: &str, part: &str) -> ThinkingBlock {
        ThinkingBlock {
            signature: Some(signature.to_string()),
            part: Some(part.to_string()),
            ..ThinkingBlock::default()
        }
    }

    fn assistant(content: &str, calls: Vec<ToolCall>, thinking: Vec<ThinkingBlock>) -> Message {
        let mut message = Message::assistant(content, (!calls.is_empty()).then_some(calls));
        message.thinking = thinking;
        message
    }

    fn wire(contents: &[GeminiContent]) -> Value {
        serde_json::to_value(contents).unwrap()
    }

    #[tokio::test]
    async fn thinking_requests_include_thoughts_inside_generation_config() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/.+:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "ok"}], "role": "model"}}]
            })))
            .mount(&mock_server)
            .await;
        let client = make_client(&mock_server.uri());

        for (config, budget) in [
            (ThinkingConfig::Level(ThinkingLevel::Low), 1024),
            (ThinkingConfig::Level(ThinkingLevel::High), 32768),
            (ThinkingConfig::Toggle(true), -1),
        ] {
            let options = CompletionOptions {
                thinking: Some(config.clone()),
                ..CompletionOptions::default()
            };
            client
                .complete(&[Message::user("hi")], &[], &options)
                .await
                .unwrap();
            let requests = mock_server.received_requests().await.unwrap();
            let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
            assert_eq!(
                at(&body, "/generationConfig/thinkingConfig"),
                &json!({"thinkingBudget": budget, "includeThoughts": true}),
                "{config:?} asks for thought summaries inside generationConfig"
            );
            assert!(
                body.get("thinkingConfig").is_none(),
                "{config:?}: thinkingConfig is not a top-level field: {body}"
            );
        }
    }

    #[tokio::test]
    async fn thinking_off_sends_no_thinking_config() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/.+:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"parts": [{"text": "ok"}], "role": "model"}}]
            })))
            .mount(&mock_server)
            .await;
        let client = make_client(&mock_server.uri());
        for thinking in [None, Some(ThinkingConfig::Toggle(false))] {
            let options = CompletionOptions {
                thinking,
                ..CompletionOptions::default()
            };
            client
                .complete(&[Message::user("hi")], &[], &options)
                .await
                .unwrap();
        }
        for request in mock_server.received_requests().await.unwrap() {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert!(
                body.pointer("/generationConfig/thinkingConfig").is_none(),
                "no thinking config when thinking is off: {body}"
            );
        }
    }

    #[test]
    fn thought_parts_are_kept_out_of_the_answer() {
        let response: GeminiResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Weighing the options", "thought": true},
                {"text": " carefully.", "thought": true},
                {"text": "The answer.", "thoughtSignature": "SIG-TEXT"}
            ]}, "finishReason": "STOP"}]
        }))
        .unwrap();
        let parsed = GeminiClient::parse_response(response).unwrap();
        assert_eq!(
            parsed.content, "The answer.",
            "summaries never reach the content"
        );
        assert_eq!(
            parsed.thinking,
            vec![
                ThinkingBlock::text("Weighing the options carefully."),
                signature_block("SIG-TEXT", TEXT_PART),
            ],
            "the summary is one block and the text's signature another"
        );
    }

    #[test]
    fn function_call_signature_is_recorded_against_its_call() {
        let response: GeminiResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Checking.", "thought": true},
                {"functionCall": {"name": "bash", "args": {"command": "ls"}},
                 "thoughtSignature": "SIG-FC1"},
                {"functionCall": {"name": "read", "args": {"path": "a"}}}
            ]}, "finishReason": "STOP"}]
        }))
        .unwrap();
        let parsed = GeminiClient::parse_response(response).unwrap();
        let ids: Vec<&str> = parsed.tool_calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["call_0", "call_1"], "calls are numbered in order");
        assert_eq!(
            parsed.thinking,
            vec![
                ThinkingBlock::text("Checking."),
                signature_block("SIG-FC1", "call_0"),
            ],
            "only the first call carries a signature, and it is tied to that call"
        );
    }

    #[test]
    fn signature_in_a_part_with_no_text_is_not_lost() {
        let response: GeminiResponse = serde_json::from_value(json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Hello"},
                {"text": "", "thoughtSignature": "SIG-LATE"}
            ]}, "finishReason": "STOP"}]
        }))
        .unwrap();
        let parsed = GeminiClient::parse_response(response).unwrap();
        assert_eq!(parsed.content, "Hello", "text unaffected");
        assert_eq!(
            parsed.thinking,
            vec![signature_block("SIG-LATE", TEXT_PART)],
            "the late signature is kept"
        );
    }

    #[test]
    fn function_calls_go_back_with_their_signatures_before_their_results() {
        let messages = vec![
            Message::user("do two things"),
            assistant(
                "On it.",
                vec![call("call_0", "bash"), call("call_1", "read")],
                vec![
                    ThinkingBlock::text("a summary that is not replayed"),
                    signature_block("SIG-FC1", "call_0"),
                    signature_block("SIG-TEXT", TEXT_PART),
                ],
            ),
            Message::tool("out-1", "call_0"),
            Message::tool("out-2", "call_1"),
        ];
        let (_, contents) = GeminiClient::convert_messages(&messages);
        let wire = wire(&contents);
        assert_eq!(
            wire,
            json!([
                {"role": "user", "parts": [{"text": "do two things"}]},
                {"role": "model", "parts": [
                    {"text": "On it.", "thoughtSignature": "SIG-TEXT"},
                    {"functionCall": {"name": "bash", "args": {"command": "ls"}},
                     "thoughtSignature": "SIG-FC1"},
                    {"functionCall": {"name": "read", "args": {"command": "ls"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "bash", "response": {"result": "out-1"}}},
                    {"functionResponse": {"name": "read", "response": {"result": "out-2"}}}
                ]}
            ]),
            "calls keep their order and signatures, then the results follow in one message, \
             each named for its function"
        );
    }

    #[test]
    fn results_are_matched_to_the_calls_just_before_them() {
        let messages = vec![
            Message::user("go"),
            assistant("", vec![call("call_0", "bash")], vec![]),
            Message::tool("first", "call_0"),
            assistant("", vec![call("call_0", "read")], vec![]),
            Message::tool("second", "call_0"),
        ];
        let (_, contents) = GeminiClient::convert_messages(&messages);
        let wire = wire(&contents);
        assert_eq!(
            at(&wire, "/2/parts/0/functionResponse/name"),
            &json!("bash"),
            "the first result answers the first call"
        );
        assert_eq!(
            at(&wire, "/4/parts/0/functionResponse/name"),
            &json!("read"),
            "ids repeat across responses, so the second result answers the second call"
        );
    }

    #[test]
    fn a_signature_with_no_text_goes_back_on_an_empty_text_part() {
        let messages = vec![assistant(
            "",
            vec![],
            vec![signature_block("SIG-ONLY", TEXT_PART)],
        )];
        let (_, contents) = GeminiClient::convert_messages(&messages);
        assert_eq!(
            wire(&contents),
            json!([{"role": "model", "parts": [{"text": "", "thoughtSignature": "SIG-ONLY"}]}]),
            "the signature is not dropped with its empty part"
        );
    }

    #[test]
    fn blocks_from_other_providers_are_not_replayed_as_signatures() {
        let messages = vec![assistant(
            "Hi",
            vec![call("call_0", "bash")],
            vec![
                ThinkingBlock {
                    text: "anthropic".to_string(),
                    signature: Some("ANTHROPIC-SIG".to_string()),
                    ..ThinkingBlock::default()
                },
                ThinkingBlock {
                    redacted: Some("ENC".to_string()),
                    ..ThinkingBlock::default()
                },
            ],
        )];
        let (_, contents) = GeminiClient::convert_messages(&messages);
        let text = serde_json::to_string(&contents).unwrap();
        assert!(
            !text.contains("ANTHROPIC-SIG") && !text.contains("ENC"),
            "only Gemini's own part-tagged signatures go back: {text}"
        );
    }

    #[tokio::test]
    async fn signatures_round_trip_through_complete() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/models/.+:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"role": "model", "parts": [
                    {"functionCall": {"name": "bash", "args": {"command": "ls"}},
                     "thoughtSignature": "SIG-FC1"}
                ]}, "finishReason": "STOP"}]
            })))
            .mount(&mock_server)
            .await;
        let client = make_client(&mock_server.uri());
        let first = client
            .complete(&[Message::user("ls")], &[], &CompletionOptions::default())
            .await
            .unwrap();

        let mut reply = Message::assistant("", Some(first.tool_calls.clone()));
        reply.thinking = first.thinking.clone();
        client
            .complete(
                &[
                    Message::user("ls"),
                    reply,
                    Message::tool("a.txt", &first.tool_calls.first().unwrap().id),
                ],
                &[],
                &CompletionOptions::default(),
            )
            .await
            .unwrap();

        let requests = mock_server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        assert_eq!(
            at(&body, "/contents/1/parts/0/thoughtSignature"),
            &json!("SIG-FC1"),
            "the signature the model sent comes back on its function call"
        );
    }

    // --- Streaming ---

    fn event(candidate: impl Into<Value>) -> String {
        let candidate: Value = candidate.into();
        format!("data: {}\n\n", json!({"candidates": [candidate]}))
    }

    fn parts_event(parts: impl Into<Value>) -> String {
        let parts: Value = parts.into();
        event(json!({"content": {"role": "model", "parts": parts}, "index": 0}))
    }

    fn last_event(parts: impl Into<Value>) -> String {
        let parts: Value = parts.into();
        format!(
            "data: {}\n\n",
            json!({
                "candidates": [{"content": {"role": "model", "parts": parts},
                    "finishReason": "STOP", "index": 0}],
                "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 9,
                    "thoughtsTokenCount": 5, "totalTokenCount": 35,
                    "cachedContentTokenCount": 4}
            })
        )
    }

    /// A reply with a reasoning summary, text and two function calls, the
    /// first of which is signed, as Gemini streams it.
    fn full_stream() -> String {
        [
            parts_event(json!([{"text": "Weighing ", "thought": true}])),
            parts_event(json!([{"text": "it.", "thought": true}])),
            parts_event(json!([{"text": "Hello w"}])),
            parts_event(json!([{"text": "\u{f6}rld \u{1f600}"}])),
            parts_event(
                json!([{"functionCall": {"name": "bash", "args": {"command": "ls"}},
                "thoughtSignature": "SIG-FC1"}]),
            ),
            last_event(json!([
                {"functionCall": {"name": "read", "args": {"path": "a"}}},
                {"text": "", "thoughtSignature": "SIG-LATE"}
            ])),
        ]
        .concat()
    }

    fn full_response_body() -> Value {
        json!({
            "candidates": [{"content": {"role": "model", "parts": [
                {"text": "Weighing it.", "thought": true},
                {"text": "Hello w\u{f6}rld \u{1f600}"},
                {"functionCall": {"name": "bash", "args": {"command": "ls"}},
                 "thoughtSignature": "SIG-FC1"},
                {"functionCall": {"name": "read", "args": {"path": "a"}}},
                {"text": "", "thoughtSignature": "SIG-LATE"}
            ]}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 9,
                "thoughtsTokenCount": 5, "totalTokenCount": 35,
                "cachedContentTokenCount": 4}
        })
    }

    async fn stream_from(
        script: Vec<Step>,
        client: impl FnOnce(&str) -> GeminiClient,
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
                &[Message::user("hi")],
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
            stream_from(sse_response(&[full_stream()]), make_client).await;
        let streamed = streamed.unwrap();

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(full_response_body()))
            .mount(&mock_server)
            .await;
        let whole = make_client(&mock_server.uri())
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();

        assert_same_response(&streamed, &whole);
        assert_eq!(streamed.content, "Hello w\u{f6}rld \u{1f600}", "text");
        assert_eq!(
            streamed.thinking,
            vec![
                ThinkingBlock::text("Weighing it."),
                signature_block("SIG-FC1", "call_0"),
                signature_block("SIG-LATE", TEXT_PART),
            ],
            "summary, the first call's signature, and the late text signature"
        );
        let ids: Vec<&str> = streamed.tool_calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["call_0", "call_1"],
            "calls numbered across events"
        );
        let usage = streamed.usage.unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens
            ),
            (21, 14, Some(4)),
            "usage from the last event, with the 5 thought tokens counted as output"
        );
        assert_eq!(
            streamed.stop_reason,
            Some(StopReason::EndTurn),
            "finish reason"
        );
        assert_eq!(sink.text(), "Hello w\u{f6}rld \u{1f600}", "text streamed");
        assert_eq!(
            sink.thinking(),
            "Weighing it.",
            "summary streamed as thinking"
        );

        let requests = server.requests();
        let request = requests.first().unwrap();
        assert!(
            request.target.contains(":streamGenerateContent")
                && request.target.contains("alt=sse")
                && request.target.contains("key=test-api-key"),
            "the streaming endpoint is used with SSE framing: {}",
            request.target
        );
    }

    #[tokio::test]
    async fn stream_parses_at_every_chunk_boundary_including_inside_characters() {
        let stream = full_stream();
        let (whole, _, _) =
            stream_from(sse_response(std::slice::from_ref(&stream)), make_client).await;
        let whole = whole.unwrap();
        for size in [1, 2, 3, 11, 64] {
            let chunks = split_bytes(&stream, size);
            let (result, sink, _) = stream_from(sse_response(&chunks), make_client).await;
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
    async fn stream_without_a_finish_reason_is_interrupted() {
        let partial = parts_event(json!([{"text": "half an ans"}]));
        let (clean_end, _, _) =
            stream_from(sse_response(std::slice::from_ref(&partial)), make_client).await;
        let err = clean_end.unwrap_err();
        assert!(
            matches!(err, InferenceError::StreamInterrupted(_)) && err.is_retryable(),
            "ending before a finish reason is an interrupted stream: {err:?}"
        );

        let (dropped, sink, _) = stream_from(sse_chunks(&[partial]), make_client).await;
        let dropped_err = dropped.unwrap_err();
        assert!(
            matches!(dropped_err, InferenceError::StreamInterrupted(_)),
            "a dropped connection is an interrupted stream: {dropped_err:?}"
        );
        assert_eq!(sink.text(), "", "the partial text is voided");
    }

    #[tokio::test]
    async fn blocked_prompt_in_a_stream_reports_no_candidates() {
        let blocked = format!(
            "data: {}\n\n",
            json!({"promptFeedback": {"blockReason": "SAFETY"}})
        );
        let (result, _, _) = stream_from(sse_response(&[blocked]), make_client).await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, InferenceError::Parse(m) if m.contains("no candidates")),
            "same error as a response that arrives whole: {err:?}"
        );
    }

    #[tokio::test]
    async fn in_stream_error_surfaces_and_unavailable_is_retryable() {
        let stream = [
            parts_event(json!([{"text": "partial"}])),
            format!(
                "data: {}\n\n",
                json!({"error": {"code": 503, "status": "UNAVAILABLE",
                    "message": "The model is overloaded."}})
            ),
        ]
        .concat();
        let (result, _, _) = stream_from(sse_response(&[stream]), make_client).await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("overloaded") && err.is_retryable(),
            "the error event surfaces and 503 is retried: {err}"
        );
    }

    #[tokio::test]
    async fn stalled_stream_fails_after_the_idle_timeout() {
        let mut script = sse_chunks(&[parts_event(json!([{"text": "partial"}]))]);
        script.push(Step::pause(std::time::Duration::from_secs(5)));
        let (result, _, _) = stream_from(script, |url| {
            let http =
                SharedHttpClient::new(&crate::inference::http::HttpClientConfig::with_timeout(1))
                    .unwrap();
            GeminiClient::new(
                http,
                url,
                "k",
                "gemini-2.0-flash",
                1024,
                RetryConfig::no_retry(),
            )
        })
        .await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, InferenceError::Stalled(1)),
            "a silent stream is a stall: {err:?}"
        );
    }

    #[tokio::test]
    async fn retry_after_partial_output_restarts_the_stream() {
        let second = [
            parts_event(json!([{"text": "whole answer"}])),
            last_event(json!([])),
        ]
        .concat();
        let server = ScriptedServer::start(vec![
            sse_chunks(&[parts_event(json!([{"text": "par"}]))]),
            sse_response(&[second]),
        ])
        .await;
        let http =
            SharedHttpClient::new(&crate::inference::http::HttpClientConfig::default()).unwrap();
        let client = GeminiClient::new(
            http,
            server.uri(),
            "k",
            "gemini-2.0-flash",
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
    async fn a_server_that_ignores_the_stream_request_is_read_whole() {
        let body = json!({
            "candidates": [{"content": {"role": "model", "parts": [{"text": "Whole answer"}]},
                "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 2}
        });
        let (result, sink, _) =
            stream_from(json_response(200, &body.to_string()), make_client).await;
        let response = result.unwrap();
        assert_eq!(
            response.content, "Whole answer",
            "the whole body is the reply"
        );
        assert!(
            sink.deltas().is_empty(),
            "nothing streamed: {:?}",
            sink.deltas()
        );
    }

    #[tokio::test]
    async fn status_errors_before_the_stream_surface_the_servers_message() {
        let (result, sink, _) = stream_from(
            json_response(
                429,
                r#"{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED"}}"#,
            ),
            make_client,
        )
        .await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("429") && err.to_string().contains("exhausted"),
            "status and message are kept: {err}"
        );
        assert!(err.is_retryable(), "429 is retried");
        assert!(sink.deltas().is_empty(), "nothing streamed");
    }

    #[tokio::test]
    async fn thought_tokens_count_as_output_without_a_stream_too() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"content": {"role": "model", "parts": [{"text": "ok"}]},
                    "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 3,
                    "thoughtsTokenCount": 40, "totalTokenCount": 53}
            })))
            .mount(&mock_server)
            .await;
        let usage = make_client(&mock_server.uri())
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap()
            .usage
            .unwrap();
        assert_eq!(usage.input_tokens, 10, "input tokens unchanged");
        assert_eq!(
            usage.output_tokens, 43,
            "the 40 thought tokens are output tokens beside the 3 of the answer"
        );
    }

    #[tokio::test]
    async fn candidate_stopped_without_content_parses_as_an_empty_response() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/models/gemini-2\.0-flash:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{"finishReason": "SAFETY"}]
            })))
            .mount(&mock_server)
            .await;
        let response = make_client(&mock_server.uri())
            .complete(&[Message::user("hi")], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert!(response.content.is_empty(), "nothing to show");
        assert_eq!(
            response.stop_reason,
            Some(StopReason::ContentFilter),
            "the reason it stopped is reported"
        );
    }

    // --- Embedding tests ---

    use crate::inference::embedding::EmbeddingProvider;

    fn make_embedding_client(base_url: &str) -> GeminiEmbeddingClient {
        let http =
            SharedHttpClient::new(&crate::inference::http::HttpClientConfig::with_timeout(60))
                .unwrap();
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
        assert!(
            matches!(err, InferenceError::Api(_)),
            "should be an Api error"
        );
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
    async fn embed_empty_input_returns_early() {
        let client = make_embedding_client("http://127.0.0.1:1");
        let result = client.embed(&[]).await;
        assert!(
            result.is_ok(),
            "empty input should return Ok without API call"
        );
        let resp = result.unwrap();
        assert!(resp.embeddings.is_empty(), "should have no embeddings");
        assert_eq!(resp.dimensions, 0, "dimensions should be 0");
    }

    #[test]
    fn strip_unsupported_removes_additional_properties_at_root() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "additionalProperties": false
        });
        let result = strip_unsupported_schema_fields(input);
        assert!(
            result.get("additionalProperties").is_none(),
            "additionalProperties should be removed"
        );
        assert!(
            result.get("properties").is_some(),
            "properties should remain"
        );
        assert!(result.get("type").is_some(), "type should remain");
    }

    #[test]
    fn strip_unsupported_removes_nested_additional_properties() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                    "additionalProperties": false
                }
            }
        });
        let result = strip_unsupported_schema_fields(input);
        let inner = result.get("properties").unwrap().get("inner").unwrap();
        assert!(
            inner.get("additionalProperties").is_none(),
            "nested additionalProperties should be removed"
        );
        assert!(inner.get("type").is_some(), "nested type should remain");
    }

    #[test]
    fn strip_unsupported_handles_array_of_objects() {
        let input = serde_json::json!([
            {
                "type": "object",
                "additionalProperties": false
            }
        ]);
        let result = strip_unsupported_schema_fields(input);
        let first = result.as_array().unwrap().first().unwrap();
        assert!(
            first.get("additionalProperties").is_none(),
            "additionalProperties in array items should be removed"
        );
        assert!(
            first.get("type").is_some(),
            "type in array items should remain"
        );
    }

    #[test]
    fn strip_unsupported_passes_non_object_through() {
        let string_val = serde_json::json!("hello");
        assert_eq!(
            strip_unsupported_schema_fields(string_val.clone()),
            string_val,
            "string values should pass through unchanged"
        );

        let number_val = serde_json::json!(42);
        assert_eq!(
            strip_unsupported_schema_fields(number_val.clone()),
            number_val,
            "number values should pass through unchanged"
        );

        let bool_val = serde_json::json!(true);
        assert_eq!(
            strip_unsupported_schema_fields(bool_val.clone()),
            bool_val,
            "bool values should pass through unchanged"
        );
    }
}

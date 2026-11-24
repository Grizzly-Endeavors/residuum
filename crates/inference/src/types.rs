//! Shared vocabulary for inference requests and responses, and the
//! provider contract every adapter implements.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::InferenceError;
use residuum_config::ThinkingConfig;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, rename = "ImageAttachment")]
pub struct ImageData {
    pub media_type: String,
    pub data: String,
}

impl ImageData {
    #[must_use]
    pub fn request_media_type(&self) -> &str {
        sniff_image_media_type_base64(&self.data).unwrap_or(&self.media_type)
    }
}

#[must_use]
pub fn sniff_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("image/webp")
    } else {
        None
    }
}

fn sniff_image_media_type_base64(data: &str) -> Option<&'static str> {
    if data.starts_with("iVBOR") {
        Some("image/png")
    } else if data.starts_with("/9j/") {
        Some("image/jpeg")
    } else if data.starts_with("R0lGO") {
        Some("image/gif")
    } else if data.starts_with("UklGR") && data.get(11..16) == Some("XRUJQ") {
        Some("image/webp")
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageAttachment {
    pub item_id: String,
    pub filename: String,
    #[ts(type = "number")]
    pub size: u64,
    pub content_type: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<MessageSender>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_sender: Option<AgentSender>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<ThinkingBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub context: bool,
    #[serde(skip)]
    pub cache_boundary: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThinkingBlock {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Opaque token the provider requires back, unchanged, when the
    /// conversation continues (Anthropic's `signature`, Gemini's
    /// `thoughtSignature`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<String>,
    /// Which part of the response the provider attached `signature` to, for
    /// a provider that signs individual parts (Gemini: the id of the tool
    /// call it belongs to, or `text` for the response's text). Replayed so
    /// each signature goes back on the part it came with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ThinkingOrigin>,
    /// A fingerprint of everything the request that produced the block sent
    /// ahead of it, for a provider that accepts a block back only while that
    /// part of the conversation is unchanged (Anthropic's current models).
    /// The provider compares it with the same fingerprint of a later request
    /// to tell whether the block still belongs to that conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

impl ThinkingBlock {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_origin(mut self, origin: &ThinkingOrigin) -> Self {
        self.origin = Some(origin.clone());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    Anthropic,
    Gemini,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    Ollama,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingOrigin {
    pub provider: ProviderApi,
    pub model: String,
}

impl ThinkingOrigin {
    #[must_use]
    pub fn new(provider: ProviderApi, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplayScope {
    SameProvider,
    SameModel,
}

pub(crate) fn blocks_produced_at<'a>(
    thinking: &'a [ThinkingBlock],
    reader: &ThinkingOrigin,
    scope: ReplayScope,
) -> Vec<&'a ThinkingBlock> {
    let own: Vec<&ThinkingBlock> = thinking
        .iter()
        .filter(|block| {
            block.origin.as_ref().is_some_and(|origin| {
                origin.provider == reader.provider
                    && (scope == ReplayScope::SameProvider || origin.model == reader.model)
            })
        })
        .collect();
    let skipped = thinking.len() - own.len();
    if skipped > 0 {
        tracing::debug!(
            skipped,
            provider = ?reader.provider,
            model = %reader.model,
            "left out thinking blocks this provider and model did not produce"
        );
    }
    own
}

pub(crate) fn current_exchange_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|msg| {
            msg.role == Role::Assistant && msg.tool_calls.as_ref().is_none_or(Vec::is_empty)
        })
        .map_or(0, |last_reply| last_reply + 1)
}

#[must_use]
pub fn readable_thinking(blocks: &[ThinkingBlock]) -> Vec<&str> {
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect()
}

#[must_use]
pub fn joined_thinking_text(blocks: &[ThinkingBlock]) -> Option<String> {
    let parts = readable_thinking(blocks);
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageSender {
    pub name: String,
    pub id: String,
    pub interface: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSender {
    pub address: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<String>,
}

impl std::fmt::Display for MessageSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} via {}", self.name, self.interface)?;
        if let Some(location) = &self.location {
            write!(f, " ({location})")?;
        }
        Ok(())
    }
}

impl Message {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            images: Vec::new(),
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn user_with_images(content: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            images,
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            images: Vec::new(),
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn system_with_cache_boundary(stable: impl Into<String>, rest: &str) -> Self {
        let mut content = stable.into();
        let boundary = content.len();
        content.push_str(rest);
        Self {
            cache_boundary: Some(boundary),
            ..Self::system(content)
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            images: Vec::new(),
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            images: Vec::new(),
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn tool_with_images(
        content: impl Into<String>,
        tool_call_id: impl Into<String>,
        images: Vec<ImageData>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            images,
            sender: None,
            agent_sender: None,
            thinking: Vec::new(),
            id: None,
            attachments: Vec::new(),
            context: false,
            cache_boundary: None,
            status_line: None,
        }
    }

    #[must_use]
    pub fn with_sender(mut self, sender: Option<MessageSender>) -> Self {
        self.sender = sender;
        self
    }

    #[must_use]
    pub fn with_agent_sender(mut self, agent_sender: Option<AgentSender>) -> Self {
        self.agent_sender = agent_sender;
        self
    }

    #[must_use]
    pub fn with_id(mut self, id: Option<String>) -> Self {
        self.id = id;
        self
    }

    #[must_use]
    pub fn as_context(mut self) -> Self {
        self.context = true;
        self
    }

    #[must_use]
    pub fn with_thinking(mut self, thinking: Vec<ThinkingBlock>) -> Self {
        self.thinking = thinking;
        self
    }

    #[must_use]
    pub fn without_replay_data(&self) -> Self {
        Self {
            thinking: readable_thinking(&self.thinking)
                .into_iter()
                .map(ThinkingBlock::text)
                .collect(),
            ..self.clone()
        }
    }

    #[must_use]
    pub fn attributed_content(&self) -> std::borrow::Cow<'_, str> {
        match &self.sender {
            Some(sender) => format!("[From: {sender}]\n{}", self.content).into(),
            None => std::borrow::Cow::Borrowed(&self.content),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncached_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_long_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

impl Usage {
    #[must_use]
    pub fn total_input_tokens(&self) -> Option<u32> {
        [
            self.uncached_input_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ]
        .into_iter()
        .flatten()
        .reduce(u32::saturating_add)
    }
}

pub(crate) fn uncached_from_inclusive_prompt(
    prompt_tokens: Option<u32>,
    cache_read_tokens: Option<u32>,
    cache_write_tokens: Option<u32>,
) -> Option<u32> {
    prompt_tokens.map(|prompt| {
        prompt
            .saturating_sub(cache_read_tokens.unwrap_or(0))
            .saturating_sub(cache_write_tokens.unwrap_or(0))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptUsage {
    pub provider: String,
    pub model: String,
    pub outcome: AttemptOutcome,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Succeeded,
    /// The attempt failed: an error status, a broken stream, or a response
    /// that could not be used.
    Failed,
}

/// Why a model response ended, normalized across providers. Each provider
/// reports this under its own name on the wire (Anthropic's `stop_reason`,
/// Gemini's `finishReason`, `OpenAI`'s `finish_reason`, Ollama's
/// `done_reason`) and maps it to this common vocabulary when parsing its
/// response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// Generation was cut off by the configured output-token limit —
    /// the response is incomplete.
    MaxTokens,
    ContentFilter,
    StopSequence,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct InferenceResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub thinking: Vec<ThinkingBlock>,
    pub stop_reason: Option<StopReason>,
    pub images_omitted: Option<super::ImagesOmitted>,
    /// How many replayed reasoning blocks the provider dropped from the
    /// request instead of reading them, because the conversation before them
    /// changed (Anthropic's `thinking_dropped` input transformation). The
    /// model answered without that reasoning.
    pub thinking_dropped: usize,
    pub attempts: Vec<AttemptUsage>,
}

impl InferenceResponse {
    #[must_use]
    pub fn new(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content,
            tool_calls,
            usage: None,
            thinking: Vec::new(),
            stop_reason: None,
            images_omitted: None,
            thinking_dropped: 0,
            attempts: Vec::new(),
        }
    }

    #[must_use]
    pub fn answered_by(&self) -> Option<&AttemptUsage> {
        self.attempts.last()
    }

    #[must_use]
    pub(crate) fn produced_at(mut self, origin: &ThinkingOrigin) -> Self {
        for block in &mut self.thinking {
            block.origin = Some(origin.clone());
        }
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_complete(&self) -> bool {
        self.tool_calls.is_empty() && !self.content.is_empty()
    }

    #[must_use]
    pub fn was_truncated(&self) -> bool {
        self.stop_reason == Some(StopReason::MaxTokens)
    }
}

#[derive(Debug, Clone, Default)]
pub enum ResponseFormat {
    #[default]
    Text,
    JsonSchema {
        /// Schema name (required by `OpenAI`, ignored by other providers).
        name: String,
        schema: serde_json::Value,
    },
}

#[derive(Debug, Clone, Default)]
pub struct WebSearchNativeConfig {
    /// Maximum web search invocations per request (Anthropic).
    pub max_uses: Option<u32>,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
    /// Search context size (`OpenAI`: `"low"`, `"medium"`, `"high"`).
    pub search_context_size: Option<String>,
    /// Domains to exclude from Google Search grounding (Gemini).
    pub exclude_domains: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheHints {
    pub conversation_key: Option<String>,
    /// How long the provider should keep this call's cached prefix.
    pub retention: CacheRetention,
}

/// How long a provider should keep a call's cached prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheRetention {
    /// The provider's default (five minutes on Anthropic): suits calls that
    /// follow each other closely, such as a session's tool loop.
    #[default]
    Short,
    /// An hour on Anthropic: suits a conversation with a person, who often
    /// pauses longer than five minutes between messages. Cache writes cost
    /// more (2x base input on Anthropic, against 1.25x).
    Long,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionOptions {
    pub max_tokens: Option<u32>,
    pub response_format: ResponseFormat,
    pub temperature: Option<f32>,
    pub thinking: Option<ThinkingConfig>,
    pub web_search: Option<WebSearchNativeConfig>,
    pub cache: CacheHints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    Text(String),
    Thinking(String),
    Restart,
}

pub trait StreamSink: Send + Sync {
    fn push(&self, delta: StreamDelta);
}

/// Trait for model provider implementations.
#[async_trait]
pub trait InferenceProvider: Send + Sync {
    /// Send a conversation to the model and get a response.
    ///
    /// # Errors
    /// Returns `InferenceError` if the request fails, times out, or the response is malformed.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError>;

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: &dyn StreamSink,
    ) -> Result<InferenceResponse, InferenceError> {
        let _ = sink;
        self.complete(messages, tools, options).await
    }

    async fn complete_as_hop(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
        sink: Option<&dyn StreamSink>,
        image_fallback: bool,
    ) -> Result<InferenceResponse, InferenceError> {
        let _ = image_fallback;
        match sink {
            Some(sink) => {
                self.complete_streaming(messages, tools, options, sink)
                    .await
            }
            None => self.complete(messages, tools, options).await,
        }
    }

    fn takes_images(&self) -> bool {
        true
    }

    fn replayed_reasoning(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Vec<String> {
        let _ = (messages, tools, options);
        Vec::new()
    }

    fn model_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_input_sums_the_reported_parts() {
        let usage = Usage {
            uncached_input_tokens: Some(10),
            cache_read_tokens: Some(200),
            cache_write_tokens: Some(30),
            ..Usage::default()
        };
        assert_eq!(usage.total_input_tokens(), Some(240));
        let only_read = Usage {
            cache_read_tokens: Some(5),
            ..Usage::default()
        };
        assert_eq!(
            only_read.total_input_tokens(),
            Some(5),
            "missing parts count as absent, not as unknown totals"
        );
        assert_eq!(
            Usage::default().total_input_tokens(),
            None,
            "nothing reported means the total is unknown"
        );
    }

    #[test]
    fn uncached_from_inclusive_prompt_subtracts_cache_and_saturates() {
        assert_eq!(
            uncached_from_inclusive_prompt(Some(100), Some(30), Some(20)),
            Some(50)
        );
        assert_eq!(
            uncached_from_inclusive_prompt(Some(100), None, None),
            Some(100)
        );
        assert_eq!(
            uncached_from_inclusive_prompt(Some(10), Some(30), None),
            Some(0),
            "an inconsistent report saturates instead of wrapping"
        );
        assert_eq!(uncached_from_inclusive_prompt(None, Some(30), None), None);
    }

    fn jane_in_channel() -> MessageSender {
        MessageSender {
            name: "Jane Doe".to_string(),
            id: "aad-123".to_string(),
            interface: "teams".to_string(),
            location: Some("#eng-team".to_string()),
        }
    }

    #[test]
    fn attributed_content_prefixes_known_sender() {
        let msg = Message::user("can you check the build?").with_sender(Some(jane_in_channel()));
        assert_eq!(
            msg.attributed_content(),
            "[From: Jane Doe via teams (#eng-team)]\ncan you check the build?"
        );
    }

    #[test]
    fn attributed_content_omits_missing_location() {
        let sender = MessageSender {
            location: None,
            ..jane_in_channel()
        };
        let msg = Message::user("hi").with_sender(Some(sender));
        assert_eq!(msg.attributed_content(), "[From: Jane Doe via teams]\nhi");
    }

    #[test]
    fn attributed_content_is_plain_without_sender() {
        let msg = Message::user("hi");
        assert_eq!(msg.attributed_content(), "hi");
    }

    #[test]
    fn sender_round_trips_and_is_optional_on_disk() {
        let msg = Message::user("hi").with_sender(Some(jane_in_channel()));
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sender, Some(jane_in_channel()));

        let legacy: Message = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(legacy.sender, None);
        let plain = serde_json::to_string(&Message::user("hi")).unwrap();
        assert!(!plain.contains("sender"), "absent sender is not serialized");
    }

    #[test]
    fn message_id_round_trips_and_is_optional_on_disk() {
        let msg = Message::user("hi").with_id(Some("m-1".to_string()));
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id.as_deref(), Some("m-1"));

        let without_id: Message =
            serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(without_id.id, None);
        let plain = serde_json::to_value(Message::user("hi")).unwrap();
        assert!(plain.get("id").is_none(), "absent id is not serialized");
    }

    #[test]
    fn context_mark_round_trips_and_is_optional_on_disk() {
        let msg = Message::user("[Resumed session] previous run").as_context();
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json.get("context"), Some(&serde_json::Value::Bool(true)));
        let back: Message = serde_json::from_value(json).unwrap();
        assert!(back.context);

        let unmarked: Message = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert!(!unmarked.context);
        let plain = serde_json::to_value(Message::user("hi")).unwrap();
        assert!(
            plain.get("context").is_none(),
            "an unmarked entry is not serialized with the mark"
        );
    }

    #[test]
    fn agent_sender_run_id_round_trips_and_is_optional_on_disk() {
        let session = AgentSender {
            address: "spawned-research".to_string(),
            category: "spawned".to_string(),
            run_id: Some("run-1".to_string()),
        };
        let msg = Message::user("found it").with_agent_sender(Some(session.clone()));
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_sender, Some(session));

        let saved = r#"{"role":"user","content":"status?","agent_sender":{"address":"agent:scout","category":"teammate"}}"#;
        let without_run: Message = serde_json::from_str(saved).unwrap();
        assert_eq!(
            without_run.agent_sender,
            Some(AgentSender {
                address: "agent:scout".to_string(),
                category: "teammate".to_string(),
                run_id: None,
            }),
            "an agent sender saved without a run id still loads"
        );
        let teammate = serde_json::to_string(&without_run).unwrap();
        assert!(
            !teammate.contains("run_id"),
            "an absent run id is not serialized"
        );
    }

    #[test]
    fn inference_response_is_complete() {
        let complete = InferenceResponse::new("hello".to_string(), vec![]);
        assert!(complete.is_complete(), "text-only response is complete");

        let with_tools = InferenceResponse::new(
            String::new(),
            vec![ToolCall {
                id: "1".to_string(),
                name: "test".to_string(),
                arguments: serde_json::Value::Null,
                server: None,
            }],
        );
        assert!(
            !with_tools.is_complete(),
            "response with tool calls is not complete"
        );

        let empty = InferenceResponse::new(String::new(), vec![]);
        assert!(
            !empty.is_complete(),
            "empty response with no tools is not complete"
        );
    }

    #[test]
    fn message_user_constructor() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, Role::User, "role should be User");
        assert_eq!(msg.content, "hello", "content should match");
        assert!(msg.tool_calls.is_none(), "tool_calls should be None");
        assert!(msg.tool_call_id.is_none(), "tool_call_id should be None");
    }

    #[test]
    fn message_system_constructor() {
        let msg = Message::system("you are a test agent");
        assert_eq!(msg.role, Role::System, "role should be System");
        assert_eq!(msg.content, "you are a test agent", "content should match");
        assert_eq!(
            msg.cache_boundary, None,
            "a plain system message has no boundary"
        );
    }

    #[test]
    fn a_cache_boundary_sits_between_the_parts_it_joins() {
        let msg = Message::system_with_cache_boundary("stable", "\n\nrest");
        assert_eq!(msg.role, Role::System);
        assert_eq!(
            msg.content, "stable\n\nrest",
            "the content is both parts, unchanged"
        );
        assert_eq!(msg.cache_boundary, Some("stable".len()));

        let all_stable = Message::system_with_cache_boundary("stable", "");
        assert_eq!(all_stable.cache_boundary, Some(all_stable.content.len()));
    }

    #[test]
    fn a_cache_boundary_is_never_stored() {
        let msg = Message::system_with_cache_boundary("stable", "rest");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("cache_boundary"), "{json}");
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cache_boundary, None);
    }

    #[test]
    fn message_assistant_constructor() {
        let msg = Message::assistant("response text", None);
        assert_eq!(msg.role, Role::Assistant, "role should be Assistant");
        assert_eq!(msg.content, "response text", "content should match");
        assert!(msg.tool_calls.is_none(), "tool_calls should be None");

        let with_tools = Message::assistant(
            "thinking",
            Some(vec![ToolCall {
                id: "c1".to_string(),
                name: "exec".to_string(),
                arguments: serde_json::Value::Null,
                server: None,
            }]),
        );
        assert!(
            with_tools.tool_calls.is_some(),
            "tool_calls should be present"
        );
    }

    #[test]
    fn message_tool_constructor() {
        let msg = Message::tool("output", "call_1");
        assert_eq!(msg.role, Role::Tool, "role should be Tool");
        assert_eq!(msg.content, "output", "content should match");
        assert_eq!(
            msg.tool_call_id,
            Some("call_1".to_string()),
            "tool_call_id should be set"
        );
    }

    #[test]
    fn message_constructors_accept_owned_string() {
        let owned = String::from("owned content");
        let msg = Message::user(owned);
        assert_eq!(msg.content, "owned content", "should accept String");
    }

    #[test]
    fn message_user_with_images_constructor() {
        let images = vec![ImageData {
            media_type: "image/jpeg".to_string(),
            data: "base64data".to_string(),
        }];
        let msg = Message::user_with_images("describe this", images);
        assert_eq!(msg.role, Role::User, "role should be User");
        assert_eq!(msg.images.len(), 1, "should have one image");
    }

    #[test]
    fn message_user_with_empty_images_is_empty() {
        let msg = Message::user_with_images("no images", vec![]);
        assert!(
            msg.images.is_empty(),
            "empty images vec should remain empty"
        );
    }

    #[test]
    fn message_tool_with_images_constructor() {
        let images = vec![ImageData {
            media_type: "image/png".to_string(),
            data: "pngdata".to_string(),
        }];
        let msg = Message::tool_with_images("image content", "call_1", images);
        assert_eq!(msg.role, Role::Tool, "role should be Tool");
        assert_eq!(msg.images.len(), 1, "should have one image");
        assert_eq!(
            msg.tool_call_id,
            Some("call_1".to_string()),
            "tool_call_id should be set"
        );
    }

    #[test]
    fn message_serde_without_images_backward_compat() {
        let json = r#"{"role":"user","content":"hello"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content, "hello");
        assert!(
            msg.images.is_empty(),
            "missing images should deserialize as empty vec"
        );
    }

    #[test]
    fn message_serde_with_images_roundtrip() {
        let images = vec![ImageData {
            media_type: "image/jpeg".to_string(),
            data: "abc123".to_string(),
        }];
        let msg = Message::user_with_images("look at this", images);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("images"),
            "serialized JSON should contain images"
        );

        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.images.len(), 1, "images should roundtrip");
        assert_eq!(restored.images.first().unwrap().media_type, "image/jpeg");
    }

    #[test]
    fn sniff_image_media_type_recognizes_each_format() {
        let cases: [(&[u8], &str); 5] = [
            (b"\x89PNG\r\n\x1A\nrest", "image/png"),
            (b"\xFF\xD8\xFF\xE0rest", "image/jpeg"),
            (b"GIF89a rest", "image/gif"),
            (b"GIF87a rest", "image/gif"),
            (b"RIFF\x24\x00\x00\x00WEBPVP8 ", "image/webp"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(sniff_image_media_type(bytes), Some(expected), "{expected}");
        }
    }

    #[test]
    fn sniff_image_media_type_returns_none_for_unknown_bytes() {
        let cases: [&[u8]; 6] = [
            b"",
            b"plain text",
            b"\xFF\xD8",
            b"RIFF",
            b"RIFF\x24\x00\x00\x00WAVEfmt ",
            b"\x89PNX",
        ];
        for bytes in cases {
            assert_eq!(sniff_image_media_type(bytes), None, "{bytes:?}");
        }
    }

    fn image_with_label(media_type: &str, data: &str) -> ImageData {
        ImageData {
            media_type: media_type.to_string(),
            data: data.to_string(),
        }
    }

    #[test]
    fn request_media_type_follows_the_bytes_over_the_label() {
        let cases = [
            ("iVBORw0KGgoAAAANSUhEUg==", "image/png"),
            ("/9j/4AAQSkZJRgABAQ==", "image/jpeg"),
            ("R0lGODlhAQABAA==", "image/gif"),
            ("R0lGODdhAQABAA==", "image/gif"),
            ("UklGRiQAAABXRUJQVlA4", "image/webp"),
        ];
        for (data, expected) in cases {
            let mislabeled = image_with_label("image/bmp", data);
            assert_eq!(mislabeled.request_media_type(), expected, "{data}");
        }
    }

    #[test]
    fn request_media_type_heals_png_label_over_jpeg_bytes() {
        let image = image_with_label("image/png", "/9j/4AAQSkZJRgABAQ==");
        assert_eq!(image.request_media_type(), "image/jpeg");
    }

    #[test]
    fn request_media_type_keeps_the_label_when_bytes_are_unknown() {
        let cases = [
            "",
            "aGVsbG8gd29ybGQ=",
            "UklGRiQAAABXQVZFZm10",
            "UklGR",
            "/9j",
        ];
        for data in cases {
            let image = image_with_label("image/webp", data);
            assert_eq!(image.request_media_type(), "image/webp", "{data:?}");
        }
    }

    #[test]
    fn message_serde_without_images_omits_field() {
        let msg = Message::user("hello world");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("\"images\""),
            "None images field should be omitted from JSON, got: {json}"
        );
    }

    #[test]
    fn response_format_default_is_text() {
        assert!(
            matches!(ResponseFormat::default(), ResponseFormat::Text),
            "default ResponseFormat should be Text"
        );
    }

    #[test]
    fn completion_options_default_has_text_format() {
        let opts = CompletionOptions::default();
        assert!(
            matches!(opts.response_format, ResponseFormat::Text),
            "default CompletionOptions should have Text response format"
        );
        assert!(
            opts.max_tokens.is_none(),
            "default max_tokens should be None"
        );
        assert!(
            opts.temperature.is_none(),
            "default temperature should be None"
        );
    }

    #[test]
    fn an_archived_message_keeps_readable_reasoning_and_drops_what_only_a_provider_needs() {
        let message = Message::assistant("42.".to_string(), None).with_thinking(vec![
            ThinkingBlock {
                text: "work it out".to_string(),
                signature: Some("sig".to_string()),
                redacted: None,
                part: None,
                origin: None,
                binding: None,
            },
            ThinkingBlock {
                text: String::new(),
                signature: None,
                redacted: Some("encrypted".to_string()),
                part: None,
                origin: None,
                binding: None,
            },
        ]);

        let archived = message.without_replay_data();

        assert_eq!(archived.thinking, [ThinkingBlock::text("work it out")]);
        assert_eq!(archived.content, "42.");
        assert_eq!(
            message.thinking.len(),
            2,
            "the original keeps its blocks whole"
        );
    }

    #[test]
    fn reasoning_serializes_only_when_there_is_some() {
        let plain = serde_json::to_value(Message::assistant("hi".to_string(), None)).unwrap();
        assert!(plain.get("thinking").is_none(), "{plain}");

        let reasoned = Message::assistant("hi".to_string(), None)
            .with_thinking(vec![ThinkingBlock::text("because")]);
        let json = serde_json::to_value(&reasoned).unwrap();
        assert_eq!(
            json.get("thinking"),
            Some(&serde_json::json!([{ "text": "because" }]))
        );
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back.thinking, reasoned.thinking);
    }

    #[test]
    fn an_origin_round_trips_and_names_the_provider_in_plain_words() {
        let block = ThinkingBlock {
            signature: Some("sig".to_string()),
            ..ThinkingBlock::default()
        }
        .with_origin(&ThinkingOrigin::new(
            ProviderApi::OpenAiCompatible,
            "gpt-test",
        ));
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "signature": "sig",
                "origin": {"provider": "openai_compatible", "model": "gpt-test"}
            })
        );
        let back: ThinkingBlock = serde_json::from_value(json).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn blocks_saved_before_origins_were_recorded_still_load_without_one() {
        let saved = r#"{"role":"assistant","content":"hi","thinking":[
            {"text":"plan","signature":"sig-old"},
            {"redacted":"ENC"},
            {"signature":"sig-g","part":"call_0"}
        ]}"#;
        let message: Message = serde_json::from_str(saved).unwrap();
        assert_eq!(message.thinking.len(), 3, "every saved block loads");
        assert!(
            message.thinking.iter().all(|block| block.origin.is_none()),
            "none of them claims an origin: {:?}",
            message.thinking
        );
        let again = serde_json::to_value(&message).unwrap();
        assert!(
            !again.to_string().contains("origin"),
            "and saving them back adds no origin: {again}"
        );
    }

    fn block_from(provider: ProviderApi, model: &str, text: &str) -> ThinkingBlock {
        ThinkingBlock::text(text).with_origin(&ThinkingOrigin::new(provider, model))
    }

    fn texts<'a>(blocks: &[&'a ThinkingBlock]) -> Vec<&'a str> {
        blocks.iter().map(|block| block.text.as_str()).collect()
    }

    #[test]
    fn a_reader_replays_only_blocks_its_own_provider_produced() {
        let thinking = vec![
            block_from(ProviderApi::Anthropic, "claude-a", "anthropic"),
            block_from(ProviderApi::Gemini, "gemini-a", "gemini"),
            block_from(ProviderApi::OpenAiCompatible, "gpt-a", "openai"),
            block_from(ProviderApi::Ollama, "llama-a", "ollama"),
            ThinkingBlock::text("no origin"),
        ];
        for (provider, model, expected) in [
            (ProviderApi::Anthropic, "claude-a", "anthropic"),
            (ProviderApi::Gemini, "gemini-a", "gemini"),
            (ProviderApi::OpenAiCompatible, "gpt-a", "openai"),
            (ProviderApi::Ollama, "llama-a", "ollama"),
        ] {
            let reader = ThinkingOrigin::new(provider, model);
            for scope in [ReplayScope::SameProvider, ReplayScope::SameModel] {
                assert_eq!(
                    texts(&blocks_produced_at(&thinking, &reader, scope)),
                    [expected],
                    "{provider:?} reading at {scope:?}"
                );
            }
        }
    }

    #[test]
    fn the_scope_decides_whether_another_model_of_the_same_provider_counts() {
        let thinking = vec![
            block_from(ProviderApi::Anthropic, "claude-a", "model a"),
            block_from(ProviderApi::Anthropic, "claude-b", "model b"),
        ];
        let reader = ThinkingOrigin::new(ProviderApi::Anthropic, "claude-b");
        assert_eq!(
            texts(&blocks_produced_at(
                &thinking,
                &reader,
                ReplayScope::SameModel
            )),
            ["model b"],
            "a signature binds to its model"
        );
        assert_eq!(
            texts(&blocks_produced_at(
                &thinking,
                &reader,
                ReplayScope::SameProvider
            )),
            ["model a", "model b"],
            "plain text means the same from any model of the provider"
        );
    }
}

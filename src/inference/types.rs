//! Shared vocabulary for inference requests and responses, and the
//! provider contract every adapter implements.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::InferenceError;

/// Base64-encoded image data for multimodal messages.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, rename = "ImageAttachment")]
pub struct ImageData {
    /// MIME type (e.g. `"image/jpeg"`, `"image/png"`).
    pub media_type: String,
    /// Base64-encoded image bytes.
    pub data: String,
}

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message sender.
    pub role: Role,
    /// The text content of the message.
    pub content: String,
    /// Tool calls requested by the assistant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// The ID of the tool call this message is a result for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Inline images attached to the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    /// Who sent a user message, when it came from an identifiable person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<MessageSender>,
    /// The agent that sent a user-role message, when it is a message one
    /// agent sent another (a session's relayed result, a `message_agent`
    /// call). Lets readers such as the web UI attribute it without trusting
    /// the header in its text, which anyone can type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_sender: Option<AgentSender>,
    /// The model's reasoning behind an assistant message, as its provider
    /// returned it. Kept so the conversation's readers can show it and so a
    /// provider that requires its reasoning back within a tool-use exchange
    /// gets it unchanged. Providers decide which of it they replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<ThinkingBlock>,
}

/// One piece of a model's reasoning, as its provider returned it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThinkingBlock {
    /// The readable reasoning. Some providers return a summary of it rather
    /// than the full text; empty when the provider withheld it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Opaque token the provider requires back, unchanged, when the
    /// conversation continues (Anthropic's `signature`, Gemini's
    /// `thoughtSignature`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Encrypted reasoning returned in place of readable text (Anthropic's
    /// `redacted_thinking`), replayed unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<String>,
    /// Which part of the response the provider attached `signature` to, for
    /// a provider that signs individual parts (Gemini: the id of the tool
    /// call it belongs to, or `text` for the response's text). Replayed so
    /// each signature goes back on the part it came with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// The provider and model that produced the block, recorded by the
    /// provider that answered. Signatures and encrypted blocks mean something
    /// only to the provider (and model) that issued them, so a provider
    /// replays only the blocks it produced itself. A block saved without an
    /// origin has none to match and is never replayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ThinkingOrigin>,
}

impl ThinkingBlock {
    /// A block holding readable reasoning and nothing to replay.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// This block, recorded as produced at `origin`.
    #[must_use]
    pub fn from_origin(mut self, origin: &ThinkingOrigin) -> Self {
        self.origin = Some(origin.clone());
        self
    }
}

/// The kind of provider API a model's reply came from. Providers that speak
/// the same wire protocol are one kind: a Fireworks host and any other
/// OpenAI-compatible server are both [`OpenAiCompatible`](Self::OpenAiCompatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    /// The Anthropic Messages API.
    Anthropic,
    /// The Google Gemini `generateContent` API.
    Gemini,
    /// An OpenAI-compatible chat completions API, including `OpenAI` itself.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// The Ollama chat API.
    Ollama,
}

/// Who produced a piece of reasoning: the provider API and the model behind
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingOrigin {
    /// The provider API that returned the reasoning.
    pub provider: ProviderApi,
    /// The model that wrote it, as the provider names it.
    pub model: String,
}

impl ThinkingOrigin {
    /// The origin of reasoning that `model` returned through `provider`.
    #[must_use]
    pub fn new(provider: ProviderApi, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }
}

/// How closely a provider ties the reasoning it replays to whoever produced
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplayScope {
    /// Any model of the provider's own API: the reasoning goes back as plain
    /// text, which means the same whichever model wrote it.
    SameProvider,
    /// Only the model that wrote it: the reasoning carries a signature or
    /// encrypted data that the issuing model binds to itself.
    SameModel,
}

/// The blocks of `thinking` that the provider at `reader` may replay: those
/// whose origin matches `reader` within `scope`. Every other block (another
/// provider's, another model's, or one saved with no origin) is left out, with
/// a debug line saying how many, so a provider never sends a request the API
/// refuses over reasoning it did not produce.
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

/// Index of the first message of the tool-use exchange the conversation is
/// in the middle of: the message after the last assistant message that made
/// no tool calls, or the first message when there is none. A provider that
/// needs its reasoning replayed with the tool calls it led to replays it for
/// the assistant messages from here on, and no earlier.
pub(crate) fn current_exchange_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|msg| {
            msg.role == Role::Assistant && msg.tool_calls.as_ref().is_none_or(Vec::is_empty)
        })
        .map_or(0, |last_reply| last_reply + 1)
}

/// The readable text of each block that has some, in order, skipping blocks
/// the provider withheld. The one place reasoning becomes text for a reader:
/// signatures and encrypted data never leave the blocks.
#[must_use]
pub fn readable_thinking(blocks: &[ThinkingBlock]) -> Vec<&str> {
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect()
}

/// The readable text of `blocks` joined by a blank line, or `None` when there
/// is none (see [`readable_thinking`]).
#[must_use]
pub fn joined_thinking_text(blocks: &[ThinkingBlock]) -> Option<String> {
    let parts = readable_thinking(blocks);
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// The person behind a user message and where they sent it from.
///
/// Stored with the message so the agent can tell participants apart in
/// shared spaces (team channels) long after the message arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageSender {
    /// Display name as the interface reports it.
    pub name: String,
    /// Stable identifier on that interface (user ID, AAD object ID).
    pub id: String,
    /// Interface the message arrived on (e.g. `"discord"`, `"teams"`).
    pub interface: String,
    /// Where on the interface it was sent (e.g. `"direct message"`, `"#general"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub location: Option<String>,
}

/// The agent behind a message one agent sent another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSender {
    /// Sender's address (`"main"` or a session address, or `agent:<name>` /
    /// `agent:<name>/<session>` for a teammate).
    pub address: String,
    /// Sender's category label (`"main"`, `"scheduled"`, `"external"`,
    /// `"spawned"`, or `"teammate"`).
    pub category: String,
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
    /// Create a user message.
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
        }
    }

    /// Create a user message with inline images.
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
        }
    }

    /// Create a system message.
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
        }
    }

    /// Create an assistant message with optional tool calls.
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
        }
    }

    /// Create a tool result message.
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
        }
    }

    /// Create a tool result message with inline images.
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
        }
    }

    /// Attach the person who sent this message.
    #[must_use]
    pub fn with_sender(mut self, sender: Option<MessageSender>) -> Self {
        self.sender = sender;
        self
    }

    /// Attach the agent that sent this message.
    #[must_use]
    pub fn with_agent_sender(mut self, agent_sender: Option<AgentSender>) -> Self {
        self.agent_sender = agent_sender;
        self
    }

    /// Attach the reasoning the model produced for this message.
    #[must_use]
    pub fn with_thinking(mut self, thinking: Vec<ThinkingBlock>) -> Self {
        self.thinking = thinking;
        self
    }

    /// This message for an archive nothing replays to a provider: its
    /// reasoning is kept as readable text only, without the signatures and
    /// encrypted blocks a provider needs back within a conversation, which
    /// mean nothing once the conversation is over.
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

    /// Message text as the agent reads it in history and transcripts.
    ///
    /// A message with a known sender is prefixed with a `[From: …]` line so
    /// the agent can tell who said what in a shared conversation.
    #[must_use]
    pub fn attributed_content(&self) -> std::borrow::Cow<'_, str> {
        match &self.sender {
            Some(sender) => format!("[From: {sender}]\n{}", self.content).into(),
            None => std::borrow::Cow::Borrowed(&self.content),
        }
    }
}

/// The role of a message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System instruction message.
    System,
    /// User input message.
    User,
    /// Assistant response message.
    Assistant,
    /// Tool result message.
    Tool,
}

impl Role {
    /// Lowercase string label for this role (e.g. `"system"`, `"user"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    /// Bold-markdown label for display in transcripts.
    #[must_use]
    pub fn as_display_str(self) -> &'static str {
        match self {
            Self::System => "**System**",
            Self::User => "**User**",
            Self::Assistant => "**Assistant**",
            Self::Tool => "**Tool**",
        }
    }
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments as a JSON value.
    pub arguments: serde_json::Value,
    /// The MCP server (its name in `mcp.json`) that owns this tool, or
    /// `None` for a built-in. Unknown when the model's response is first
    /// parsed; filled in from the MCP registry before the call is recorded
    /// or dispatched (see `agent::turn::annotate_tool_call_servers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// Definition of an available tool sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The tool name.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: serde_json::Value,
}

/// Token usage information from a model response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Number of input/prompt tokens consumed.
    pub input_tokens: u32,
    /// Number of output/completion tokens generated.
    pub output_tokens: u32,
    /// Tokens written to the prompt cache (Anthropic-specific).
    pub cache_creation_tokens: Option<u32>,
    /// Tokens read from the prompt cache.
    pub cache_read_tokens: Option<u32>,
}

/// Why a model response ended, normalized across providers. Each provider
/// reports this under its own name on the wire (Anthropic's `stop_reason`,
/// Gemini's `finishReason`, `OpenAI`'s `finish_reason`, Ollama's
/// `done_reason`) and maps it to this common vocabulary when parsing its
/// response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The model reached a natural stopping point.
    EndTurn,
    /// The model decided to call a tool.
    ToolUse,
    /// Generation was cut off by the configured output-token limit —
    /// the response is incomplete.
    MaxTokens,
    /// The provider's content filter stopped generation.
    ContentFilter,
    /// A configured stop sequence was hit.
    StopSequence,
    /// A reason not covered above, carrying the provider's own raw value.
    Other(String),
}

/// Response from a model provider.
#[derive(Debug, Clone)]
pub struct InferenceResponse {
    /// The assistant's text response (may be empty if only tool calls).
    pub content: String,
    /// Tool calls the assistant wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage information, if the provider reports it.
    pub usage: Option<Usage>,
    /// The model's reasoning for this response, in the order it was returned.
    pub thinking: Vec<ThinkingBlock>,
    /// Why generation ended, if the provider reports it.
    pub stop_reason: Option<StopReason>,
}

impl InferenceResponse {
    /// Create a new model response.
    #[must_use]
    pub fn new(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content,
            tool_calls,
            usage: None,
            thinking: Vec::new(),
            stop_reason: None,
        }
    }

    /// This response with every thinking block recorded as produced at
    /// `origin`. The provider that answered calls this on what it returns,
    /// so a chain of providers (failover) records the one that actually
    /// answered.
    #[must_use]
    pub(crate) fn produced_at(mut self, origin: &ThinkingOrigin) -> Self {
        for block in &mut self.thinking {
            block.origin = Some(origin.clone());
        }
        self
    }

    /// Whether this response represents a complete turn (text, no tool calls).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.tool_calls.is_empty() && !self.content.is_empty()
    }

    /// Whether generation was cut off by the output-token limit rather than
    /// ending naturally.
    #[must_use]
    pub fn was_truncated(&self) -> bool {
        self.stop_reason == Some(StopReason::MaxTokens)
    }
}

/// Desired response format for model completions.
#[derive(Debug, Clone, Default)]
pub enum ResponseFormat {
    /// Plain text response (default behavior).
    #[default]
    Text,
    /// JSON response conforming to a JSON Schema.
    JsonSchema {
        /// Schema name (required by `OpenAI`, ignored by other providers).
        name: String,
        /// The JSON Schema that the response must conform to.
        schema: serde_json::Value,
    },
}

/// Thinking/reasoning configuration for model completions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingConfig {
    /// Graduated reasoning effort (Anthropic, `OpenAI`, Gemini).
    Level(ThinkingLevel),
    /// Simple on/off toggle (Ollama, or explicit disable).
    Toggle(bool),
}

/// Reasoning effort levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}

/// Provider-native web search configuration passed to completion requests.
///
/// Flat struct — each provider reads the fields it cares about.
#[derive(Debug, Clone, Default)]
pub struct WebSearchNativeConfig {
    /// Maximum web search invocations per request (Anthropic).
    pub max_uses: Option<u32>,
    /// Restrict search to these domains (Anthropic).
    pub allowed_domains: Option<Vec<String>>,
    /// Exclude these domains from search (Anthropic).
    pub blocked_domains: Option<Vec<String>>,
    /// Search context size (`OpenAI`: `"low"`, `"medium"`, `"high"`).
    pub search_context_size: Option<String>,
    /// Domains to exclude from Google Search grounding (Gemini).
    pub exclude_domains: Option<Vec<String>>,
}

/// Options for model completion requests.
#[derive(Debug, Clone, Default)]
pub struct CompletionOptions {
    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,
    /// Desired response format.
    pub response_format: ResponseFormat,
    /// Sampling temperature (0.0–2.0). None uses provider default.
    pub temperature: Option<f32>,
    /// Thinking/reasoning configuration. None means not configured (off).
    pub thinking: Option<ThinkingConfig>,
    /// Provider-native web search configuration. None disables web search.
    pub web_search: Option<WebSearchNativeConfig>,
}

/// A piece of a model response that arrived while the call is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDelta {
    /// More of the response's text.
    Text(String),
    /// More of the model's readable reasoning.
    Thinking(String),
    /// Everything streamed so far for this call is void: the request is
    /// being sent again (a retry, or the next provider in a failover chain),
    /// and what follows starts over from the beginning.
    Restart,
}

/// Receives a response's pieces as they stream in.
///
/// `push` must not block: providers call it from inside their read loop.
pub trait StreamSink: Send + Sync {
    /// Take the next piece of the response.
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

    /// Like [`complete`](Self::complete), also handing the response's text
    /// and reasoning to `sink` as they arrive. The returned response is
    /// complete and authoritative; what the sink received previews it. A
    /// provider that can't stream keeps this default, which streams nothing.
    ///
    /// # Errors
    /// Returns `InferenceError` if the request fails, times out, or the response is malformed.
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

    /// Get the model identifier.
    fn model_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // History written before senders existed has no `sender` key.
        let legacy: Message = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(legacy.sender, None);
        let plain = serde_json::to_string(&Message::user("hi")).unwrap();
        assert!(!plain.contains("sender"), "absent sender is not serialized");
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
        // Simulate old JSON without images field
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
            },
            ThinkingBlock {
                text: String::new(),
                signature: None,
                redacted: Some("encrypted".to_string()),
                part: None,
                origin: None,
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
        .from_origin(&ThinkingOrigin::new(
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
        ThinkingBlock::text(text).from_origin(&ThinkingOrigin::new(provider, model))
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

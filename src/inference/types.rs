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
}

/// The person behind a user message and where they sent it from.
///
/// Stored with the message so the agent can tell participants apart in
/// shared spaces (team channels) long after the message arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSender {
    /// Display name as the interface reports it.
    pub name: String,
    /// Stable identifier on that interface (user ID, AAD object ID).
    pub id: String,
    /// Interface the message arrived on (e.g. `"discord"`, `"teams"`).
    pub interface: String,
    /// Where on the interface it was sent (e.g. `"direct message"`, `"#general"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// The agent behind a message one agent sent another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSender {
    /// Sender's address (`"main"` or a session address).
    pub address: String,
    /// Sender's category label (`"main"`, `"scheduled"`, `"external"`, or `"spawned"`).
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

/// Response from a model provider.
#[derive(Debug, Clone)]
pub struct InferenceResponse {
    /// The assistant's text response (may be empty if only tool calls).
    pub content: String,
    /// Tool calls the assistant wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage information, if the provider reports it.
    pub usage: Option<Usage>,
    /// Thinking/reasoning text from the model (not sent back in context).
    pub thinking: Option<String>,
}

impl InferenceResponse {
    /// Create a new model response.
    #[must_use]
    pub fn new(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            content,
            tool_calls,
            usage: None,
            thinking: None,
        }
    }

    /// Whether this response represents a complete turn (text, no tool calls).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.tool_calls.is_empty() && !self.content.is_empty()
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
}

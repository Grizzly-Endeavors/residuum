//! Model provider abstraction and shared LLM types.

pub(crate) mod anthropic;
pub(crate) mod embedding;
pub(crate) mod factory;
pub(crate) mod failover;
pub(crate) mod gemini;
mod http;
pub(crate) mod null;
pub(crate) mod ollama;
pub(crate) mod openai;
pub(crate) mod retry;
pub(crate) mod think_tags;

pub(crate) use embedding::build_embedding_provider;
pub use embedding::{EmbeddingProvider, EmbeddingResponse};
pub(crate) use factory::build_provider_chain;
pub use http::{HttpClientConfig, SharedHttpClient};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from model provider operations.
#[derive(Error, Debug)]
pub enum ModelError {
    /// HTTP request failed (network, DNS, TLS)
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// Response could not be parsed
    #[error("failed to parse response: {0}")]
    Parse(String),

    /// API returned an error status
    #[error("API error: {0}")]
    Api(String),

    /// Request timed out
    #[error("request timed out after {0} seconds")]
    Timeout(u64),
}

impl ModelError {
    /// Whether this error is likely to succeed on retry.
    ///
    /// - Request/Timeout: transient network failures
    /// - Parse: permanent -- malformed response won't improve
    /// - Api: retryable only when the message indicates rate-limiting or overload
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Request(_) | Self::Timeout(_) => true,
            Self::Parse(_) => false,
            Self::Api(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("rate")
                    || lower.contains("limit")
                    || lower.contains("overload")
                    || lower.contains("capacity")
                    || lower.contains("429")
                    || lower.contains("503")
                    || lower.contains("502")
            }
        }
    }
}

/// Base64-encoded image data for multimodal messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageData>>,
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
            images: None,
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
            images: if images.is_empty() {
                None
            } else {
                Some(images)
            },
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
            images: None,
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
            images: None,
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
            images: None,
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
            images: if images.is_empty() {
                None
            } else {
                Some(images)
            },
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
#[derive(Debug, Clone, Copy, Default)]
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
pub struct ModelResponse {
    /// The assistant's text response (may be empty if only tool calls).
    pub content: String,
    /// Tool calls the assistant wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage information, if the provider reports it.
    pub usage: Option<Usage>,
    /// Thinking/reasoning text from the model (not sent back in context).
    pub thinking: Option<String>,
}

impl ModelResponse {
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
}

/// Trait for model provider implementations.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Send a conversation to the model and get a response.
    ///
    /// # Errors
    /// Returns `ModelError` if the request fails, times out, or the response is malformed.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError>;

    /// Get the model identifier.
    fn model_name(&self) -> &str;
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn model_error_display_parse() {
        let err = ModelError::Parse("bad json".to_string());
        assert_eq!(
            err.to_string(),
            "failed to parse response: bad json",
            "parse error should include context"
        );
    }

    #[test]
    fn model_error_display_timeout() {
        let err = ModelError::Timeout(60);
        assert_eq!(
            err.to_string(),
            "request timed out after 60 seconds",
            "timeout should show duration"
        );
    }

    #[test]
    fn model_error_is_retryable_transient() {
        assert!(
            ModelError::Timeout(60).is_retryable(),
            "timeout should be retryable"
        );

        assert!(
            ModelError::Api("rate limit exceeded".to_string()).is_retryable(),
            "rate limit should be retryable"
        );

        assert!(
            ModelError::Api("Error 429: too many requests".to_string()).is_retryable(),
            "429 should be retryable"
        );
    }

    #[test]
    fn model_error_is_retryable_permanent() {
        assert!(
            !ModelError::Parse("invalid json".to_string()).is_retryable(),
            "parse error should not be retryable"
        );

        assert!(
            !ModelError::Api("invalid api key".to_string()).is_retryable(),
            "auth error should not be retryable"
        );
    }

    #[test]
    fn model_response_is_complete() {
        let complete = ModelResponse::new("hello".to_string(), vec![]);
        assert!(complete.is_complete(), "text-only response is complete");

        let with_tools = ModelResponse::new(
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

        let empty = ModelResponse::new(String::new(), vec![]);
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
        assert!(msg.images.is_some(), "images should be present");
        assert_eq!(
            msg.images.as_ref().unwrap().len(),
            1,
            "should have one image"
        );
    }

    #[test]
    fn message_user_with_empty_images_is_none() {
        let msg = Message::user_with_images("no images", vec![]);
        assert!(
            msg.images.is_none(),
            "empty images vec should be stored as None"
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
        assert!(msg.images.is_some(), "images should be present");
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
            msg.images.is_none(),
            "missing images should deserialize as None"
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
        assert!(restored.images.is_some(), "images should roundtrip");
        assert_eq!(
            restored.images.unwrap().first().unwrap().media_type,
            "image/jpeg"
        );
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

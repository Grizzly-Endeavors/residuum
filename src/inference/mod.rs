//! Inference provider abstraction and the shared LLM vocabulary.

pub(crate) mod embedding;
mod error;
pub(crate) mod factory;
pub(crate) mod failover;
mod http;
pub(crate) mod providers;
mod reply;
pub(crate) mod retry;
mod stream;
pub mod system_one;
#[cfg(test)]
mod test_support;
mod types;

pub(crate) use embedding::build_embedding_provider;
pub use embedding::{EmbeddingProvider, EmbeddingResponse};
pub use error::{FailureDescription, InferenceError, describe_turn_failure};
pub(crate) use factory::{
    DroppedFallback, build_provider_chain, build_provider_chain_with_notices,
    build_provider_chain_with_shared_notices,
};
pub use http::{HttpClientConfig, SharedHttpClient};
pub use types::{
    AgentSender, CompletionOptions, ImageData, InferenceProvider, InferenceResponse, Message,
    MessageSender, ResponseFormat, Role, StopReason, StreamDelta, StreamSink, ThinkingBlock,
    ThinkingConfig, ThinkingLevel, ToolCall, ToolDefinition, Usage, WebSearchNativeConfig,
};

//! Inference provider abstraction and the shared LLM vocabulary.

pub(crate) mod embedding;
mod error;
pub(crate) mod factory;
pub(crate) mod failover;
mod http;
pub(crate) mod providers;
pub(crate) mod retry;
mod types;

pub(crate) use embedding::build_embedding_provider;
pub use embedding::{EmbeddingProvider, EmbeddingResponse};
pub use error::InferenceError;
pub(crate) use factory::build_provider_chain;
pub use http::{HttpClientConfig, SharedHttpClient};
pub use types::{
    CompletionOptions, ImageData, InferenceProvider, InferenceResponse, Message, ResponseFormat,
    Role, ThinkingConfig, ThinkingLevel, ToolCall, ToolDefinition, Usage, WebSearchNativeConfig,
};

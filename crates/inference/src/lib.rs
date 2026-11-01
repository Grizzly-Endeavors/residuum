//! Inference provider abstraction and the shared LLM vocabulary.
//!
//! The `test-support` feature adds `testing`, the gated model server and
//! provider other crates' tests hold model calls open with.

mod attempt;
#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;
pub(crate) mod embedding;
mod error;
pub(crate) mod factory;
pub(crate) mod failover;
mod gen_ai_span;
mod http;
mod image_input;
pub mod pricing;
pub(crate) mod providers;
mod reply;
mod retry;
mod role_options;
mod stream;
pub mod system_one;
#[cfg(test)]
mod test_support;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod tool;
mod types;
mod usage_record;

pub use embedding::{EmbeddingProvider, EmbeddingResponse, build_embedding_provider};
pub use error::{FailureDescription, InferenceError, describe_turn_failure};
pub use factory::{
    DroppedFallback, build_provider_chain, build_provider_chain_with_notices,
    build_provider_chain_with_shared_notices,
};
pub use failover::NoticeSink;
pub use http::{HttpClientConfig, SharedHttpClient};
pub use image_input::ImagesOmitted;
pub use pricing::{CostEstimate, PriceTable};
pub use providers::anthropic;
pub use providers::null::NullProvider;
pub use reply::strip_code_fences;
pub(crate) use retry::with_retry;
pub use role_options::{completion_options_for_role, main_conversation_options};
pub use tool::{ToolError, ToolResult};
pub use types::{
    AgentSender, AttemptOutcome, AttemptUsage, CacheHints, CacheRetention, CompletionOptions,
    ImageData, InferenceProvider, InferenceResponse, Message, MessageAttachment, MessageSender,
    ProviderApi, ResponseFormat, Role, StopReason, StreamDelta, StreamSink, ThinkingBlock,
    ThinkingOrigin, ToolCall, ToolDefinition, Usage, WebSearchNativeConfig, joined_thinking_text,
    readable_thinking,
};
pub(crate) use types::{ReplayScope, blocks_produced_at};
pub use usage_record::{UsageComponent, UsageRecord, UsageRecorder};

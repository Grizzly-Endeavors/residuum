//! Shared mock model provider for memory subsystem tests.

use async_trait::async_trait;

use crate::inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ToolDefinition,
};

/// A mock [`InferenceProvider`] that returns a fixed response for every `complete` call.
///
/// Eliminates the near-identical `MockObserverProvider` / `MockReflectorProvider`
/// structs that were duplicated in each test module.
pub(crate) struct MockMemoryProvider {
    response: String,
}

impl MockMemoryProvider {
    pub(crate) fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }
}

#[async_trait]
impl InferenceProvider for MockMemoryProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        Ok(InferenceResponse::new(self.response.clone(), vec![]))
    }

    fn model_name(&self) -> &'static str {
        "mock-memory"
    }
}

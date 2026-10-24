//! A fixed-reply model provider for tests that run the observer or reflector
//! (feature `test-support`).

use async_trait::async_trait;

use residuum_inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ToolDefinition,
};

/// A mock [`InferenceProvider`] that returns a fixed response for every `complete` call.
pub struct MockMemoryProvider {
    response: String,
}

impl MockMemoryProvider {
    /// A provider whose every completion is `response`.
    #[must_use]
    pub fn new(response: &str) -> Self {
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

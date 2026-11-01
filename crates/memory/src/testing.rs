//! A fixed-reply model provider for tests that run the observer or reflector
//! (feature `test-support`).

use async_trait::async_trait;

use residuum_inference::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ToolDefinition, Usage,
};

/// A mock [`InferenceProvider`] that returns a fixed response for every `complete` call.
pub struct MockMemoryProvider {
    response: String,
    usage: Option<Usage>,
}

impl MockMemoryProvider {
    /// A provider whose every completion is `response`, reporting no usage.
    #[must_use]
    pub fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
            usage: None,
        }
    }

    /// This provider, reporting `usage` on every completion.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
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
        let mut response = InferenceResponse::new(self.response.clone(), vec![]);
        response.usage = self.usage;
        Ok(response)
    }

    fn model_name(&self) -> &'static str {
        "mock-memory"
    }
}

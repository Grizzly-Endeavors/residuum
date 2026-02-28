//! Null model provider for disabled observer/reflector instances.

use async_trait::async_trait;

use super::{CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ToolDefinition};

/// A placeholder provider that always returns an error.
///
/// Used by disabled observer/reflector instances to satisfy the type system
/// without requiring a real model configuration.
pub(crate) struct NullProvider;

#[async_trait]
impl ModelProvider for NullProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Api(
            "null provider: this provider is disabled and should never be called".to_string(),
        ))
    }

    fn model_name(&self) -> &str {
        "null"
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn complete_returns_error() {
        let provider = NullProvider;
        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "null provider should always fail");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("null provider"),
            "error should mention null provider, got: {msg}"
        );
        assert!(
            msg.contains("disabled"),
            "error should mention disabled, got: {msg}"
        );
    }

    #[test]
    fn model_name_returns_null() {
        let provider = NullProvider;
        assert_eq!(
            provider.model_name(),
            "null",
            "model name should be 'null'"
        );
    }
}

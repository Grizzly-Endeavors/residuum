//! Failover provider: wraps multiple providers and tries each in order.

use async_trait::async_trait;
use tracing::{info, warn};

use super::{CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, ToolDefinition};

/// A provider that tries multiple underlying providers in order.
///
/// On error (after retries exhaust within each provider), falls back to the next.
pub(crate) struct FailoverProvider {
    providers: Vec<Box<dyn ModelProvider>>,
}

impl FailoverProvider {
    /// Create a new failover provider from an ordered list.
    ///
    /// The first provider is the primary; subsequent providers are fallbacks.
    #[must_use]
    pub(crate) fn new(providers: Vec<Box<dyn ModelProvider>>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl ModelProvider for FailoverProvider {
    #[tracing::instrument(skip_all, fields(provider_count = self.providers.len()))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<ModelResponse, ModelError> {
        let total = self.providers.len();
        let mut last_error: Option<ModelError> = None;

        for (idx, provider) in self.providers.iter().enumerate() {
            match provider.complete(messages, tools, options).await {
                Ok(response) => {
                    if idx > 0 {
                        info!(
                            primary = self.providers.first().map_or("unknown", |p| p.model_name()),
                            succeeded_on = provider.model_name(),
                            attempts = idx + 1,
                            "failover succeeded"
                        );
                    }
                    return Ok(response);
                }
                Err(err) => {
                    let remaining = total - idx - 1;
                    if remaining > 0 {
                        warn!(
                            provider = provider.model_name(),
                            error = %err,
                            remaining,
                            "provider failed, trying next in failover chain"
                        );
                    }
                    last_error = Some(err);
                }
            }
        }

        // All providers failed — return the last error
        Err(last_error.unwrap_or_else(|| {
            ModelError::Api("no providers configured in failover chain".to_string())
        }))
    }

    fn model_name(&self) -> &str {
        // Return the primary (first) provider's name
        self.providers
            .first()
            .map_or("failover(empty)", |p| p.model_name())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    /// A mock provider that always succeeds with a fixed response.
    struct SuccessProvider {
        name: &'static str,
    }

    #[async_trait]
    impl ModelProvider for SuccessProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::new(
                format!("response from {}", self.name),
                vec![],
            ))
        }

        fn model_name(&self) -> &str {
            self.name
        }
    }

    /// A mock provider that always fails.
    struct FailProvider {
        name: &'static str,
    }

    #[async_trait]
    impl ModelProvider for FailProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            Err(ModelError::Api(format!("{} unavailable", self.name)))
        }

        fn model_name(&self) -> &str {
            self.name
        }
    }

    #[tokio::test]
    async fn single_provider_succeeds() {
        let provider = FailoverProvider::new(vec![Box::new(SuccessProvider { name: "primary" })]);

        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_ok(), "single provider should succeed");
        assert_eq!(
            result.unwrap().content,
            "response from primary",
            "should return primary response"
        );
    }

    #[tokio::test]
    async fn first_fails_second_succeeds() {
        let provider = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ]);

        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_ok(), "should succeed via failover");
        assert_eq!(
            result.unwrap().content,
            "response from fallback",
            "should return fallback response"
        );
    }

    #[tokio::test]
    async fn all_fail_returns_last_error() {
        let provider = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(FailProvider { name: "fallback" }),
        ]);

        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;

        assert!(result.is_err(), "should fail when all providers fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("fallback"),
            "should return last provider's error: {err}"
        );
    }

    #[test]
    fn model_name_returns_primary() {
        let provider = FailoverProvider::new(vec![
            Box::new(SuccessProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ]);

        assert_eq!(
            provider.model_name(),
            "primary",
            "model_name should return primary provider's name"
        );
    }

    #[tokio::test]
    async fn empty_provider_list_returns_error() {
        let provider = FailoverProvider::new(vec![]);
        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;
        assert!(result.is_err(), "empty provider list should return error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no providers"),
            "error should mention no providers: {err}"
        );
    }

    #[test]
    fn empty_provider_model_name() {
        let provider = FailoverProvider::new(vec![]);
        assert_eq!(
            provider.model_name(),
            "failover(empty)",
            "empty provider should return failover(empty)"
        );
    }
}

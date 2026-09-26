//! Failover provider: wraps multiple providers and tries each in order.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tracing::{info, warn};

use super::{
    CompletionOptions, InferenceError, InferenceProvider, InferenceResponse, Message,
    ToolDefinition,
};
use crate::bus::{NoticeEvent, NotifyName, Publisher, SYSTEM_CHANNEL, topics};

/// A provider that tries multiple underlying providers in order.
///
/// On error (after retries exhaust within each provider), falls back to the next.
pub(crate) struct FailoverProvider {
    providers: Vec<Box<dyn InferenceProvider>>,
    /// When set, a user notice is published on a fallback/recovery
    /// transition — see [`Self::with_notices`].
    notices: Option<FailoverNotices>,
    /// Index of the provider that succeeded on the previous call. Used only
    /// to detect a transition; `complete` still tries every call starting
    /// from index 0, the same as before this field existed.
    ///
    /// Shared (`Arc`) rather than owned outright so multiple short-lived
    /// `FailoverProvider`s built for the same role — e.g. one freshly built
    /// per background-session spawn — can track transitions jointly via
    /// [`Self::with_shared_active_index`], instead of each instance starting
    /// from index 0 and re-announcing a fallback that's already in effect.
    active_index: Arc<AtomicUsize>,
}

/// What a fallback/recovery notice needs: where to publish it and what to
/// call the role it's speaking for (e.g. "the main model").
struct FailoverNotices {
    publisher: Publisher,
    role: String,
}

impl FailoverProvider {
    /// Create a new failover provider from an ordered list.
    ///
    /// The first provider is the primary; subsequent providers are fallbacks.
    #[must_use]
    pub(crate) fn new(providers: Vec<Box<dyn InferenceProvider>>) -> Self {
        Self {
            providers,
            notices: None,
            active_index: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Enable one user notice on a fallback transition and one when back on
    /// the primary — never on every call while already on the fallback.
    /// `role` names what's failing over in the notice, e.g. `"main model"`.
    #[must_use]
    pub(crate) fn with_notices(mut self, publisher: Publisher, role: impl Into<String>) -> Self {
        self.notices = Some(FailoverNotices {
            publisher,
            role: role.into(),
        });
        self
    }

    /// Track transitions in a shared counter instead of this instance's own.
    ///
    /// For a role whose `FailoverProvider` is rebuilt fresh per call site
    /// (background-session tiers are built anew for every spawn), sharing
    /// one counter across every instance for that role means a transition
    /// notices exactly once when the tier actually fails over or recovers,
    /// rather than once per spawn made while it's already degraded.
    #[must_use]
    pub(crate) fn with_shared_active_index(mut self, active_index: Arc<AtomicUsize>) -> Self {
        self.active_index = active_index;
        self
    }

    /// Publish a notice when `idx` (the provider that just succeeded, named
    /// by `provider_name`) differs from the provider active on the previous
    /// call — a fallback (moving off index 0) or a recovery (back to index
    /// 0). No-op when notices aren't configured, or the active provider
    /// didn't change.
    async fn notice_on_transition(
        &self,
        idx: usize,
        provider_name: &str,
        fallback_reason: Option<&'static str>,
    ) {
        let previous = self.active_index.swap(idx, Ordering::SeqCst);
        if previous == idx {
            return;
        }
        let Some(notices) = &self.notices else {
            return;
        };
        let message = if idx == 0 {
            format!(
                "The {} is back online; switched back to {provider_name}.",
                notices.role
            )
        } else {
            let reason = fallback_reason.unwrap_or("an error");
            format!(
                "The {} is unavailable ({reason}); switched to {provider_name} until it \
                 recovers.",
                notices.role
            )
        };
        if let Err(e) = notices
            .publisher
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                NoticeEvent { message },
            )
            .await
        {
            tracing::warn!(error = %e, "failed to publish failover notice");
        }
    }
}

#[async_trait]
impl InferenceProvider for FailoverProvider {
    #[tracing::instrument(skip_all, fields(provider_count = self.providers.len(), primary = self.providers.first().map_or("empty", |p| p.model_name())))]
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &CompletionOptions,
    ) -> Result<InferenceResponse, InferenceError> {
        let total = self.providers.len();
        let previously_active = self.active_index.load(Ordering::SeqCst);
        let mut last_error: Option<InferenceError> = None;
        // The cause of leaving whichever provider was active going into
        // this call, captured only if that specific provider fails again
        // this time — the reason a fallback notice names.
        let mut active_provider_cause: Option<&'static str> = None;

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
                    self.notice_on_transition(idx, provider.model_name(), active_provider_cause)
                        .await;
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
                    if idx == previously_active {
                        active_provider_cause = Some(err.cause_phrase());
                    }
                    last_error = Some(err);
                }
            }
        }

        // All providers failed — return the last error
        Err(last_error.unwrap_or_else(|| {
            InferenceError::Api("no providers configured in failover chain".to_string())
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
mod tests {
    use super::*;

    /// A mock provider that always succeeds with a fixed response.
    struct SuccessProvider {
        name: &'static str,
    }

    #[async_trait]
    impl InferenceProvider for SuccessProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Ok(InferenceResponse::new(
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
    impl InferenceProvider for FailProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Err(InferenceError::Api(format!("{} unavailable", self.name)))
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

    /// A mock provider that fails on its first N calls, then succeeds.
    struct RecoveringProvider {
        name: &'static str,
        remaining_failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for RecoveringProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            let remaining =
                self.remaining_failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        (n > 0).then(|| n - 1)
                    });
            if remaining.is_ok() {
                Err(InferenceError::Api(format!(
                    "{} rate limit exceeded",
                    self.name
                )))
            } else {
                Ok(InferenceResponse::new(
                    format!("response from {}", self.name),
                    vec![],
                ))
            }
        }

        fn model_name(&self) -> &str {
            self.name
        }
    }

    async fn notice_text(sub: &mut crate::bus::Subscriber<NoticeEvent>) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("a notice should have been published")
            .unwrap()
            .unwrap()
            .message
    }

    async fn assert_no_notice(sub: &mut crate::bus::Subscriber<NoticeEvent>) {
        let recv = tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv()).await;
        assert!(
            recv.is_err(),
            "expected no notice on a repeat call while already on the fallback"
        );
    }

    #[tokio::test]
    async fn failover_transition_publishes_exactly_one_notice() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        let provider = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ])
        .with_notices(publisher, "main model");

        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await;
        assert!(result.is_ok(), "should succeed via failover");

        let message = notice_text(&mut notices).await;
        assert!(
            message.contains("main model") && message.contains("fallback"),
            "notice should name the role and the fallback it switched to: {message}"
        );
        assert!(
            message.contains("unavailable"),
            "notice should say why it failed over: {message}"
        );
    }

    #[tokio::test]
    async fn staying_on_the_fallback_does_not_notice_again() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        let provider = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ])
        .with_notices(publisher, "main model");

        // First call: transition to the fallback, one notice.
        provider
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        notice_text(&mut notices).await;

        // Second and third calls: primary still fails, fallback still
        // serves — same active provider as before, so no repeat notice.
        for _ in 0..2 {
            provider
                .complete(&[], &[], &CompletionOptions::default())
                .await
                .unwrap();
        }
        assert_no_notice(&mut notices).await;
    }

    #[tokio::test]
    async fn recovery_to_primary_publishes_a_back_online_notice() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        let provider = FailoverProvider::new(vec![
            Box::new(RecoveringProvider {
                name: "primary",
                remaining_failures: std::sync::atomic::AtomicUsize::new(1),
            }),
            Box::new(SuccessProvider { name: "fallback" }),
        ])
        .with_notices(publisher, "main model");

        // First call: primary fails once, fallback serves — fallover notice.
        provider
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        let fallover_message = notice_text(&mut notices).await;
        assert!(fallover_message.contains("unavailable"));

        // Second call: primary has recovered — recovery notice, not a
        // second fallover notice.
        let result = provider
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_eq!(result.content, "response from primary");
        let recovery_message = notice_text(&mut notices).await;
        assert!(
            recovery_message.contains("back online") && recovery_message.contains("primary"),
            "recovery notice should say it's back on the primary: {recovery_message}"
        );
    }

    #[tokio::test]
    async fn shared_active_index_suppresses_repeat_notices_across_instances() {
        let bus_handle = crate::bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        let shared_index = Arc::new(AtomicUsize::new(0));

        // First short-lived provider: primary fails, notices the fallover.
        let first = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ])
        .with_shared_active_index(Arc::clone(&shared_index))
        .with_notices(publisher.clone(), "background sessions (large tier)");
        first
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        notice_text(&mut notices).await;

        // A second, brand-new provider for the same role/tier, built as if
        // for another spawn while the tier is still degraded — because it
        // shares the same active-index counter (already at the fallback),
        // it must not re-announce the fallover.
        let second = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ])
        .with_shared_active_index(Arc::clone(&shared_index))
        .with_notices(publisher, "background sessions (large tier)");
        second
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_no_notice(&mut notices).await;
    }

    #[tokio::test]
    async fn without_notices_configured_nothing_is_published() {
        let bus_handle = crate::bus::spawn_broker();
        let mut notices: crate::bus::Subscriber<NoticeEvent> = bus_handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        // No `.with_notices(...)` — the default (used by every role other
        // than main) must stay exactly as quiet as before this feature.
        let provider = FailoverProvider::new(vec![
            Box::new(FailProvider { name: "primary" }),
            Box::new(SuccessProvider { name: "fallback" }),
        ]);

        provider
            .complete(&[], &[], &CompletionOptions::default())
            .await
            .unwrap();
        assert_no_notice(&mut notices).await;
    }
}

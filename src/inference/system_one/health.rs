//! What the hub shows about the System 1 service: configured or not, and
//! whether it is currently answering.

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use super::error::{SystemOneError, SystemOneOutageKind};

/// The System 1 service as the user sees it. Sent on the hub WebSocket on
/// connect and whenever it changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct SystemOneStatus {
    /// A provider is set in `[system_one]`.
    pub configured: bool,
    /// The provider's display name, when configured.
    pub provider: Option<String>,
    /// The configured model, when configured.
    pub model: Option<String>,
    /// Set from the first failed call until a call or a recovery check
    /// succeeds, or the config changes.
    pub outage: Option<SystemOneOutage>,
}

/// A System 1 outage: why decisions can't be made right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct SystemOneOutage {
    pub kind: SystemOneOutageKind,
    /// Plain-language reason and what to do about it.
    pub message: String,
    /// When the outage began, RFC 3339.
    #[ts(type = "string")]
    pub since: DateTime<Utc>,
}

impl SystemOneStatus {
    /// The status of a freshly configured (or unconfigured) service.
    #[must_use]
    pub fn new(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            configured: provider.is_some(),
            provider,
            model,
            outage: None,
        }
    }

    /// Record a failed call. Returns whether the status changed in a way the
    /// user should see: an outage starting or changing kind. A repeat of the
    /// same kind keeps the original `since` and refreshes only the message.
    pub(crate) fn record_failure(&mut self, error: &SystemOneError) -> bool {
        let kind = error.outage_kind();
        let message = error.user_message();
        match &mut self.outage {
            Some(outage) if outage.kind == kind => {
                let changed = outage.message != message;
                outage.message = message;
                changed
            }
            _ => {
                self.outage = Some(SystemOneOutage {
                    kind,
                    message,
                    since: Utc::now(),
                });
                true
            }
        }
    }

    /// Record a successful call. Returns whether an outage ended.
    pub(crate) fn record_success(&mut self) -> bool {
        self.outage.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreachable(detail: &str) -> SystemOneError {
        SystemOneError::Unreachable {
            provider: "Ollama".to_string(),
            url: "http://localhost:11434".to_string(),
            detail: detail.to_string(),
        }
    }

    #[test]
    fn repeated_failures_of_one_kind_keep_the_start_time() {
        let mut status = SystemOneStatus::new(Some("Ollama".into()), Some("nimble".into()));
        assert!(
            status.record_failure(&unreachable("refused")),
            "first failure is a change"
        );
        let since = status.outage.as_ref().map(|o| o.since);
        assert!(
            !status.record_failure(&unreachable("refused again")),
            "the same kind and message is not a new change"
        );
        assert_eq!(status.outage.as_ref().map(|o| o.since), since);
    }

    #[test]
    fn a_new_kind_is_a_change_and_success_clears() {
        let mut status = SystemOneStatus::new(Some("TypeSafe".into()), Some("jev-latest".into()));
        status.record_failure(&unreachable("x"));
        assert!(status.record_failure(&SystemOneError::AuthRejected {
            provider: "TypeSafe".into()
        }));
        assert_eq!(
            status.outage.as_ref().map(|o| o.kind),
            Some(SystemOneOutageKind::Rejected)
        );
        assert!(status.record_success(), "success ends the outage");
        assert!(!status.record_success(), "nothing left to clear");
    }
}

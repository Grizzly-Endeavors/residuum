use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use super::error::{SystemOneError, SystemOneOutageKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct SystemOneStatus {
    pub configured: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub outage: Option<SystemOneOutage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct SystemOneOutage {
    pub kind: SystemOneOutageKind,
    pub message: String,
    #[ts(type = "string")]
    pub since: DateTime<Utc>,
}

impl SystemOneStatus {
    #[must_use]
    pub fn new(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            configured: provider.is_some(),
            provider,
            model,
            outage: None,
        }
    }

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

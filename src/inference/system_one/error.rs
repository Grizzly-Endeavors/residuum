//! Errors from System 1 evaluations, with plain-language messages.

use thiserror::Error;

/// Why a System 1 call failed. `provider` is the display name the user picked
/// (`TypeSafe`, Ollama, or the custom endpoint's host).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SystemOneError {
    #[error("no decision model is set up")]
    NotConfigured,
    #[error("couldn't reach {provider} at {url}: {detail}")]
    Unreachable {
        provider: String,
        url: String,
        detail: String,
    },
    #[error("{provider} didn't answer within {secs} seconds")]
    Timeout { provider: String, secs: u64 },
    #[error("{provider} rejected the API key")]
    AuthRejected { provider: String },
    #[error("{provider} is rate limiting requests")]
    RateLimited { provider: String },
    #[error("{provider} is overloaded or having a problem: {detail}")]
    Overloaded { provider: String, detail: String },
    #[error("{provider} doesn't have the model {model}: {detail}")]
    ModelNotFound {
        provider: String,
        model: String,
        detail: String,
    },
    #[error("{provider} rejected the request: {detail}")]
    RequestRejected { provider: String, detail: String },
    #[error("{provider} sent a response that couldn't be read: {detail}")]
    InvalidResponse { provider: String, detail: String },
}

impl SystemOneError {
    /// Whether waiting and trying again may succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Unreachable { .. }
                | Self::Timeout { .. }
                | Self::RateLimited { .. }
                | Self::Overloaded { .. }
        )
    }

    /// Whether this failure says the service as a whole is unusable, rather
    /// than one request being bad. A rejected request (an oversized or
    /// malformed state) leaves the service healthy for the next one.
    #[must_use]
    pub fn affects_health(&self) -> bool {
        !matches!(self, Self::RequestRejected { .. })
    }

    /// The kind of outage this failure means, for the health status.
    #[must_use]
    pub fn outage_kind(&self) -> SystemOneOutageKind {
        match self {
            Self::NotConfigured => SystemOneOutageKind::NotConfigured,
            Self::Unreachable { .. }
            | Self::Timeout { .. }
            | Self::RateLimited { .. }
            | Self::Overloaded { .. } => SystemOneOutageKind::Unreachable,
            Self::AuthRejected { .. }
            | Self::ModelNotFound { .. }
            | Self::RequestRejected { .. }
            | Self::InvalidResponse { .. } => SystemOneOutageKind::Rejected,
        }
    }

    /// A sentence for a non-technical user: what went wrong and what to do.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::NotConfigured => "No decision model is set up. Choose one under Settings → All agents → Decision model.".to_string(),
            Self::Unreachable { provider, url, .. } => format!(
                "Couldn't reach {provider} at {url}. Check that it's running and the address is right."
            ),
            Self::Timeout { provider, .. } => format!(
                "{provider} is taking too long to answer. If it's a local model it may still be loading; it will be retried."
            ),
            Self::AuthRejected { provider } => format!(
                "{provider} didn't accept the API key. Check the key under Settings → All agents → Decision model."
            ),
            Self::RateLimited { provider } => format!(
                "{provider} is limiting how many requests it takes right now. It will be retried shortly."
            ),
            Self::Overloaded { provider, .. } => format!(
                "{provider} is having trouble right now. It will be retried shortly."
            ),
            Self::ModelNotFound { provider, model, .. } => format!(
                "{provider} doesn't have the model \"{model}\". Pick another model under Settings → All agents → Decision model."
            ),
            Self::RequestRejected { provider, .. } => format!(
                "{provider} couldn't evaluate this request, usually because it was too large."
            ),
            Self::InvalidResponse { provider, .. } => format!(
                "{provider} answered in a way Residuum couldn't read. Check that the model is a decision (System 1) model."
            ),
        }
    }
}

/// The broad kind of a System 1 outage, shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SystemOneOutageKind {
    /// Something asked for a decision but no provider is configured.
    NotConfigured,
    /// The service can't be reached, timed out, or is overloaded.
    Unreachable,
    /// The service answered but refused: bad key, unknown model, bad reply.
    Rejected,
}

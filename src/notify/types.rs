//! Notification types and channel configuration.

/// A single resolved external channel configuration.
#[derive(Debug, Clone)]
pub struct ExternalChannelConfig {
    /// Channel name (key from channel definitions).
    pub name: String,
    /// Channel type and type-specific settings.
    pub kind: ExternalChannelKind,
}

/// Channel type with type-specific configuration.
#[derive(Debug, Clone)]
pub enum ExternalChannelKind {
    /// Ntfy push notification channel.
    Ntfy {
        /// Ntfy server URL.
        url: String,
        /// Topic to publish to.
        topic: String,
        /// Message priority (default: `"default"`).
        priority: Option<String>,
    },
    /// Webhook HTTP channel.
    Webhook {
        /// Endpoint URL.
        url: String,
        /// HTTP method (default: `"POST"`).
        method: Option<String>,
        /// Additional headers.
        headers: Vec<(String, String)>,
    },
    /// Native macOS notification channel.
    Macos {
        /// Default notification category.
        default_category: Option<String>,
        /// Default interruption level.
        default_priority: Option<String>,
        /// Throttle window in seconds.
        throttle_window_secs: Option<u64>,
        /// Play notification sound.
        sound: Option<bool>,
        /// Display name in banners.
        app_name: Option<String>,
        /// Base URL for "Open" action.
        web_url: Option<String>,
    },
}

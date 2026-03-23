use std::collections::HashMap;

use crate::notify::channels::NotificationChannel;
use crate::notify::external::{NtfyChannel, WebhookChannel};
use crate::notify::types::{ExternalChannelConfig, ExternalChannelKind};

/// Build external channel implementations from configs.
pub async fn build_external_channels(
    configs: &[ExternalChannelConfig],
    client: &reqwest::Client,
) -> HashMap<String, Box<dyn NotificationChannel>> {
    let mut channels: HashMap<String, Box<dyn NotificationChannel>> = HashMap::new();

    for cfg in configs {
        match &cfg.kind {
            ExternalChannelKind::Ntfy {
                url,
                topic,
                priority,
            } => {
                channels.insert(
                    cfg.name.clone(),
                    Box::new(NtfyChannel::new(
                        cfg.name.clone(),
                        client.clone(),
                        url.clone(),
                        topic.clone(),
                        priority.clone(),
                    )),
                );
            }
            ExternalChannelKind::Webhook {
                url,
                method,
                headers,
            } => {
                channels.insert(
                    cfg.name.clone(),
                    Box::new(WebhookChannel::new(
                        cfg.name.clone(),
                        client.clone(),
                        url.clone(),
                        method.clone(),
                        headers.clone(),
                    )),
                );
            }
            ExternalChannelKind::Macos { .. } => {
                if let Some(ch) = build_macos_channel(&cfg.name, &cfg.kind).await {
                    channels.insert(cfg.name.clone(), ch);
                }
            }
        }
    }

    tracing::debug!(count = channels.len(), "built external channels");
    channels
}

/// Build a macOS notification channel from raw config fields.
///
/// On non-macOS platforms, logs a warning and returns `None`.
#[cfg(target_os = "macos")]
async fn build_macos_channel(
    name: &str,
    kind: &ExternalChannelKind,
) -> Option<Box<dyn NotificationChannel>> {
    use crate::notify::macos::MacosChannelConfig;
    use crate::notify::macos::categories::{parse_category, parse_priority};

    let ExternalChannelKind::Macos {
        default_category,
        default_priority,
        throttle_window_secs,
        sound,
        app_name,
        web_url,
    } = kind
    else {
        return None;
    };

    let mut config = MacosChannelConfig::default();

    if let Some(cat) = default_category {
        match parse_category(cat) {
            Ok(c) => config.default_category = c,
            Err(e) => {
                tracing::warn!(channel = name, error = %e, "invalid macOS channel config, skipping");
                return None;
            }
        }
    }

    if let Some(pri) = default_priority {
        match parse_priority(pri) {
            Ok(p) => config.default_priority = p,
            Err(e) => {
                tracing::warn!(channel = name, error = %e, "invalid macOS channel config, skipping");
                return None;
            }
        }
    }

    if let Some(secs) = throttle_window_secs {
        config.throttle_window_secs = *secs;
    }
    if let Some(s) = sound {
        config.sound = *s;
    }
    if let Some(n) = app_name {
        config.app_name = n.clone();
    }
    config.web_url = web_url.clone();

    match crate::notify::macos::MacosNativeChannel::new(name, config).await {
        Ok((channel, _handle)) => {
            tracing::info!(channel = name, "macOS notification channel initialized");
            Some(Box::new(channel))
        }
        Err(e) => {
            tracing::warn!(channel = name, error = %e, "failed to initialize macOS channel, skipping");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[expect(
    clippy::unused_async,
    reason = "signature must match the async macOS variant"
)]
async fn build_macos_channel(
    name: &str,
    _kind: &ExternalChannelKind,
) -> Option<Box<dyn NotificationChannel>> {
    tracing::warn!(
        channel = name,
        "macOS notification channel configured but not available on this platform"
    );
    None
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_external_channels_creates_instances() {
        let configs = vec![
            ExternalChannelConfig {
                name: "my-ntfy".to_string(),
                kind: ExternalChannelKind::Ntfy {
                    url: "https://ntfy.sh".to_string(),
                    topic: "test".to_string(),
                    priority: None,
                },
            },
            ExternalChannelConfig {
                name: "my-webhook".to_string(),
                kind: ExternalChannelKind::Webhook {
                    url: "https://hooks.example.com".to_string(),
                    method: None,
                    headers: Vec::new(),
                },
            },
        ];

        let client = reqwest::Client::new();
        let channels = build_external_channels(&configs, &client).await;

        assert_eq!(channels.len(), 2);
        assert!(channels.contains_key("my-ntfy"));
        assert!(channels.contains_key("my-webhook"));
        assert_eq!(channels["my-ntfy"].channel_kind(), "ntfy");
        assert_eq!(channels["my-webhook"].channel_kind(), "webhook");
    }

    #[tokio::test]
    async fn build_external_channels_empty_input() {
        let client = reqwest::Client::new();
        let channels = build_external_channels(&[], &client).await;
        assert!(channels.is_empty());
    }
}

//! Native macOS notification channel via `UNUserNotificationCenter`.
//!
//! Delivers notification banners through Apple's `UserNotifications` framework.
//! All macOS-specific code is gated behind `cfg(target_os = "macos")`.

pub mod bridge;
pub mod categories;
pub mod permissions;
pub mod throttle;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::channels::NotificationChannel;
use super::types::Notification;

/// Configuration for a macOS native notification channel.
#[derive(Debug, Clone)]
pub struct MacosChannelConfig {
    /// Default notification category when source doesn't specify one.
    pub default_category: categories::MacosCategory,
    /// Default interruption level for notifications.
    pub default_priority: categories::MacosInterruptionLevel,
    /// Duration in seconds for the batch throttle window.
    pub throttle_window_secs: u64,
    /// Whether to play a sound with notifications.
    pub sound: bool,
    /// Display name in notification banners.
    pub app_name: String,
    /// Base URL for "Open" action. If unset, "Open" action is omitted.
    pub web_url: Option<String>,
}

impl MacosChannelConfig {
    /// Validate the configuration values.
    ///
    /// # Errors
    /// Returns an error if `throttle_window_secs` is outside the valid range (1-300).
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.throttle_window_secs == 0 || self.throttle_window_secs > 300 {
            anyhow::bail!(
                "throttle_window_secs must be between 1 and 300, got {}",
                self.throttle_window_secs
            );
        }
        Ok(())
    }
}

impl Default for MacosChannelConfig {
    fn default() -> Self {
        Self {
            default_category: categories::MacosCategory::BackgroundResults,
            default_priority: categories::MacosInterruptionLevel::Active,
            throttle_window_secs: 30,
            sound: true,
            app_name: "Residuum".to_string(),
            web_url: None,
        }
    }
}

/// Native macOS notification channel.
///
/// Sends notifications to a `BatchAggregator` via an mpsc channel for
/// throttled delivery through `UNUserNotificationCenter`.
pub struct MacosNativeChannel {
    channel_name: String,
    batch_tx: mpsc::Sender<Notification>,
}

impl MacosNativeChannel {
    /// Create a new macOS notification channel.
    ///
    /// Initializes the macOS bridge, checks permissions, and spawns the
    /// batch aggregator task.
    ///
    /// # Errors
    /// Returns an error if the macOS bridge cannot be initialized.
    pub async fn new(
        name: impl Into<String>,
        config: MacosChannelConfig,
    ) -> anyhow::Result<(Self, JoinHandle<()>)> {
        config.validate()?;

        let channel_name = name.into();
        let (tx, rx) = mpsc::channel(64);

        let macos_bridge = bridge::MacosBridge::new(config.clone())?;

        permissions::check_and_request(&macos_bridge).await;

        let aggregator_handle = throttle::BatchAggregator::spawn(rx, macos_bridge, config);

        Ok((
            Self {
                channel_name,
                batch_tx: tx,
            },
            aggregator_handle,
        ))
    }
}

#[async_trait]
impl NotificationChannel for MacosNativeChannel {
    fn name(&self) -> &str {
        &self.channel_name
    }

    fn channel_kind(&self) -> &'static str {
        "macos"
    }

    async fn deliver(&self, notification: &Notification) -> anyhow::Result<()> {
        let queued = Notification {
            task_name: notification.task_name.clone(),
            summary: notification.summary.clone(),
            source: notification.source,
            timestamp: notification.timestamp,
        };
        self.batch_tx
            .send(queued)
            .await
            .map_err(|e| anyhow::anyhow!("macOS notification aggregator is not running: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn config_default_values() {
        let cfg = MacosChannelConfig::default();
        assert_eq!(cfg.throttle_window_secs, 30);
        assert!(cfg.sound);
        assert_eq!(cfg.app_name, "Residuum");
        assert!(cfg.web_url.is_none());
    }

    #[test]
    fn config_validate_valid() {
        let cfg = MacosChannelConfig::default();
        cfg.validate().unwrap();
    }

    #[test]
    fn config_validate_zero_throttle() {
        let cfg = MacosChannelConfig {
            throttle_window_secs: 0,
            ..MacosChannelConfig::default()
        };
        assert!(cfg.validate().is_err(), "throttle 0 should fail validation");
    }

    #[test]
    fn config_validate_over_max_throttle() {
        let cfg = MacosChannelConfig {
            throttle_window_secs: 301,
            ..MacosChannelConfig::default()
        };
        assert!(
            cfg.validate().is_err(),
            "throttle 301 should fail validation"
        );
    }

    #[test]
    fn config_validate_boundary_values() {
        let min = MacosChannelConfig {
            throttle_window_secs: 1,
            ..MacosChannelConfig::default()
        };
        assert!(min.validate().is_ok(), "throttle 1 should be valid");

        let max = MacosChannelConfig {
            throttle_window_secs: 300,
            ..MacosChannelConfig::default()
        };
        assert!(max.validate().is_ok(), "throttle 300 should be valid");
    }
}

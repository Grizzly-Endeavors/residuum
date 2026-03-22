//! Native macOS notification channel via `UNUserNotificationCenter`.

pub mod bridge;
pub mod categories;
pub mod permissions;
pub mod throttle;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bus::NotificationEvent;

use super::channels::NotificationChannel;

#[derive(Debug, Clone)]
pub struct MacosChannelConfig {
    pub default_category: categories::MacosCategory,
    pub default_priority: categories::MacosInterruptionLevel,
    pub throttle_window_secs: u64,
    pub sound: bool,
    pub app_name: String,
    /// If unset, the "Open" action button is omitted from notifications.
    pub web_url: Option<String>,
}

impl MacosChannelConfig {
    /// # Errors
    /// Returns an error if `throttle_window_secs` is outside 1-300.
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

/// `deliver()` enqueues to the `BatchAggregator` and returns immediately —
/// actual macOS delivery happens asynchronously after the throttle window.
pub struct MacosNativeChannel {
    channel_name: String,
    batch_tx: mpsc::Sender<NotificationEvent>,
}

impl MacosNativeChannel {
    /// # Errors
    /// Returns an error if the macOS bridge cannot be initialized.
    pub async fn new(
        name: impl Into<String>,
        config: MacosChannelConfig,
    ) -> anyhow::Result<(Self, JoinHandle<()>)> {
        config.validate()?;

        let channel_name = name.into();
        let (tx, rx) = mpsc::channel(64);

        let macos_bridge = bridge::MacosBridge::new(config.clone(), None)?;

        permissions::check_and_request().await;

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

    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
        self.batch_tx
            .send(notification.clone())
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

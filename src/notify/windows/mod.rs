//! Native Windows notification channel via Toast notifications.

pub mod categories;
pub mod throttle;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bus::NotificationEvent;

use super::channels::NotificationChannel;

#[derive(Debug, Clone)]
pub struct WindowsChannelConfig {
    pub default_category: categories::WindowsCategory,
    pub default_scenario: categories::WindowsScenario,
    pub throttle_window_secs: u64,
    pub sound: bool,
    pub app_name: String,
    /// `WinRT` App User Model ID for Toast grouping.
    pub app_id: String,
}

impl WindowsChannelConfig {
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

impl Default for WindowsChannelConfig {
    fn default() -> Self {
        Self {
            default_category: categories::WindowsCategory::BackgroundResults,
            default_scenario: categories::WindowsScenario::Default,
            throttle_window_secs: 30,
            sound: true,
            app_name: "Residuum".to_string(),
            app_id: "GrizzlyEndeavors.Residuum".to_string(),
        }
    }
}

/// `deliver()` enqueues to the batch aggregator and returns immediately --
/// actual Toast delivery happens asynchronously after the throttle window.
pub struct WindowsNativeChannel {
    channel_name: String,
    batch_tx: mpsc::Sender<NotificationEvent>,
}

impl WindowsNativeChannel {
    /// # Errors
    /// Returns an error if config validation fails.
    pub fn new(
        name: impl Into<String>,
        config: WindowsChannelConfig,
    ) -> anyhow::Result<(Self, JoinHandle<()>)> {
        config.validate()?;

        let channel_name = name.into();
        let (tx, rx) = mpsc::channel(64);

        let bridge = throttle::WindowsBridge::new(&config);
        let aggregator_handle = throttle::spawn(rx, bridge, config);

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
impl NotificationChannel for WindowsNativeChannel {
    fn name(&self) -> &str {
        &self.channel_name
    }

    fn channel_kind(&self) -> &'static str {
        "windows"
    }

    async fn deliver(&self, notification: &NotificationEvent) -> anyhow::Result<()> {
        self.batch_tx
            .send(notification.clone())
            .await
            .map_err(|e| anyhow::anyhow!("Windows notification aggregator is not running: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn config_default_values() {
        let cfg = WindowsChannelConfig::default();
        assert_eq!(cfg.throttle_window_secs, 30);
        assert!(cfg.sound);
        assert_eq!(cfg.app_name, "Residuum");
        assert_eq!(cfg.app_id, "GrizzlyEndeavors.Residuum");
        assert_eq!(
            cfg.default_category,
            categories::WindowsCategory::BackgroundResults
        );
        assert_eq!(cfg.default_scenario, categories::WindowsScenario::Default);
    }

    #[test]
    fn config_validate_valid() {
        let cfg = WindowsChannelConfig::default();
        cfg.validate().unwrap();
    }

    #[test]
    fn config_validate_zero_throttle() {
        let cfg = WindowsChannelConfig {
            throttle_window_secs: 0,
            ..WindowsChannelConfig::default()
        };
        assert!(cfg.validate().is_err(), "throttle 0 should fail validation");
    }

    #[test]
    fn config_validate_over_max_throttle() {
        let cfg = WindowsChannelConfig {
            throttle_window_secs: 301,
            ..WindowsChannelConfig::default()
        };
        assert!(
            cfg.validate().is_err(),
            "throttle 301 should fail validation"
        );
    }

    #[test]
    fn config_validate_boundary_values() {
        let min = WindowsChannelConfig {
            throttle_window_secs: 1,
            ..WindowsChannelConfig::default()
        };
        assert!(min.validate().is_ok(), "throttle 1 should be valid");

        let max = WindowsChannelConfig {
            throttle_window_secs: 300,
            ..WindowsChannelConfig::default()
        };
        assert!(max.validate().is_ok(), "throttle 300 should be valid");
    }
}

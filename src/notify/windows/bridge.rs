//! Windows Toast notification bridge.
//!
//! Isolates the `WinRT` Toast API so the rest of the notification system
//! stays platform-agnostic and testable without a Windows runtime.

use async_trait::async_trait;

use super::WindowsChannelConfig;
use crate::bus::NotificationEvent;
use crate::notify::batch_aggregator::{self, NotificationBridge};

/// Thin wrapper around the `WinRT` Toast API for notification delivery.
#[derive(Clone)]
pub struct WindowsBridge {
    app_id: String,
    app_name: String,
    sound: bool,
}

impl WindowsBridge {
    #[must_use]
    pub fn new(config: &WindowsChannelConfig) -> Self {
        Self {
            app_id: config.app_id.clone(),
            app_name: config.app_name.clone(),
            sound: config.sound,
        }
    }
}

#[async_trait]
impl NotificationBridge for WindowsBridge {
    async fn deliver_individual(&self, notif: &NotificationEvent) {
        let title = self.app_name.clone();
        let body = batch_aggregator::truncate_body(&notif.content, 200);

        post_toast(self, &title, &body).await;
    }

    async fn deliver_summary(&self, title: &str, body: &str, _urgent: bool) {
        post_toast(self, title, body).await;
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn platform_label(&self) -> &'static str {
        "Windows"
    }
}

#[cfg(target_os = "windows")]
async fn post_toast(bridge: &WindowsBridge, title: &str, body: &str) {
    let app_id = bridge.app_id.clone();
    let title = title.to_string();
    let body = body.to_string();
    let sound = bridge.sound;

    // spawn_blocking because winrt-notification's show() is synchronous; the
    // join result is only interesting if the task panicked, which we can't
    // act on here, so it's intentionally not inspected.
    let _join_result = tokio::task::spawn_blocking(move || {
        use winrt_notification::Toast;
        let mut toast = Toast::new(&app_id).title(&title).text1(&body);
        if !sound {
            toast = toast.sound(None);
        }
        if let Err(e) = toast.show() {
            tracing::warn!(error = %e, "failed to show Windows Toast notification");
        }
    })
    .await;
}

#[cfg(not(target_os = "windows"))]
async fn post_toast(_bridge: &WindowsBridge, _title: &str, _body: &str) {
    // No-op on non-Windows platforms
}

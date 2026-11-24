use async_trait::async_trait;

use super::WindowsChannelConfig;
use crate::batch_aggregator::{self, NotificationBridge};
use residuum_bus::NotificationEvent;

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

        post_toast(self, &title, &body, notif.urgent).await;
    }

    async fn deliver_summary(&self, title: &str, body: &str, urgent: bool) {
        post_toast(self, title, body, urgent).await;
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn platform_label(&self) -> &'static str {
        "Windows"
    }
}

#[cfg(target_os = "windows")]
async fn post_toast(bridge: &WindowsBridge, title: &str, body: &str, urgent: bool) {
    let app_id = bridge.app_id.clone();
    let title = title.to_string();
    let body = body.to_string();
    let sound = bridge.sound;

    let _join_result = residuum_core::util::spawn_blocking_in_span(move || {
        use tauri_winrt_notification::{Scenario, Toast};
        let mut toast = Toast::new(&app_id).title(&title).text1(&body);
        if !sound {
            toast = toast.sound(None);
        }
        if urgent {
            // Windows only honors the Reminder scenario on a toast with at
            // least one button; without one it shows as a normal toast.
            toast = toast
                .scenario(Scenario::Reminder)
                .add_button("Dismiss", "dismiss");
        }
        if let Err(e) = toast.show() {
            tracing::warn!(error = %e, "failed to show Windows Toast notification");
        }
    })
    .await;
}

#[cfg(not(target_os = "windows"))]
async fn post_toast(_bridge: &WindowsBridge, _title: &str, _body: &str, _urgent: bool) {}

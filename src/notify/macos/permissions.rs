//! macOS notification permission checking and requesting.

use super::bridge::MacosBridge;

pub async fn check_and_request(_bridge: &MacosBridge) {
    let result = tokio::task::spawn_blocking(|| {
        check_permissions_sync();
    })
    .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, "permission check task failed");
    }
}

fn check_permissions_sync() {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    let result = std::panic::catch_unwind(|| {
        let center = UNUserNotificationCenter::currentNotificationCenter();

        // Request authorization with provisional to avoid upfront prompt
        let options = UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge
            | UNAuthorizationOptions::Provisional;

        let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!(
                    "macOS notification permission request failed. \
                     To enable: System Settings > Notifications > Residuum > Allow Notifications"
                );
            } else if granted.as_bool() {
                tracing::info!("macOS notification permissions granted");
            } else {
                tracing::warn!(
                    "macOS notification permissions not granted for Residuum. \
                     To enable: System Settings > Notifications > Residuum > Allow Notifications. \
                     Notifications will not be delivered until permissions are granted."
                );
            }
        });

        center.requestAuthorizationWithOptions_completionHandler(options, &block);
    });

    if result.is_err() {
        tracing::warn!("panic during macOS notification permission check");
    }
}

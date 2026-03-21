//! macOS `UNUserNotificationCenter` FFI bridge.

#![expect(
    unsafe_code,
    reason = "objc2 FFI for macOS UserNotifications framework"
)]

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{ClassType, msg_send};
use objc2_foundation::{NSBundle, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNMutableNotificationContent, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationInterruptionLevel, UNNotificationRequest, UNNotificationSound,
    UNUserNotificationCenter,
};
use tokio::sync::mpsc;

use super::MacosChannelConfig;
use super::categories::{MacosCategory, MacosInterruptionLevel, MacosNotificationAction};

/// Text content for a macOS notification.
pub struct NotificationText {
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

/// `UNUserNotificationCenter` throws an unrecoverable `ObjC` exception if the
/// process has no app bundle. Check `NSBundle.mainBundle.bundleIdentifier`
/// first so we can return an error instead of crashing.
fn require_app_bundle() -> anyhow::Result<()> {
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none() {
        anyhow::bail!(
            "macOS notifications require an app bundle with a CFBundleIdentifier. \
             Run residuum from a .app bundle or set the bundle identifier via Info.plist"
        );
    }
    Ok(())
}

#[derive(Debug)]
pub struct InboxAcknowledgment {
    pub item_id: String,
}

/// Isolates all objc2 FFI so the rest of the notification system
/// stays platform-agnostic and testable without a macOS runtime.
pub struct MacosBridge {
    config: MacosChannelConfig,
    ack_tx: Option<mpsc::Sender<InboxAcknowledgment>>,
}

impl MacosBridge {
    /// # Errors
    /// Returns an error if category registration fails.
    pub fn new(config: MacosChannelConfig) -> anyhow::Result<Self> {
        require_app_bundle()?;
        let bridge = Self {
            config,
            ack_tx: None,
        };
        Self::register_categories();
        Ok(bridge)
    }

    pub fn set_ack_sender(&mut self, tx: mpsc::Sender<InboxAcknowledgment>) {
        self.ack_tx = Some(tx);
    }

    /// Must run before any notification is posted — macOS silently drops
    /// action buttons for notifications whose category isn't registered.
    fn register_categories() {
        let result = std::panic::catch_unwind(|| {
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let mut category_set: Vec<Retained<UNNotificationCategory>> = Vec::new();

            for cat in MacosCategory::all() {
                let actions = MacosNotificationAction::for_category(*cat);
                let mut ns_actions: Vec<Retained<objc2_user_notifications::UNNotificationAction>> =
                    Vec::new();

                for action in actions {
                    let action_id = NSString::from_str(action.action_id());
                    let title = NSString::from_str(action.button_title());
                    let ns_action =
                        objc2_user_notifications::UNNotificationAction::actionWithIdentifier_title_options(
                            &action_id,
                            &title,
                            objc2_user_notifications::UNNotificationActionOptions::empty(),
                        );
                    ns_actions.push(ns_action);
                }

                let cat_id = NSString::from_str(cat.as_category_id());
                let actions_array = objc2_foundation::NSArray::from_retained_slice(&ns_actions);
                let empty_intents: Retained<objc2_foundation::NSArray<NSString>> =
                    objc2_foundation::NSArray::from_retained_slice(&[]);

                let category =
                    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                        &cat_id,
                        &actions_array,
                        &empty_intents,
                        UNNotificationCategoryOptions::empty(),
                    );
                category_set.push(category);
            }

            let ns_set = NSSet::from_retained_slice(&category_set);
            center.setNotificationCategories(&ns_set);

            tracing::info!(
                categories = category_set.len(),
                "macOS notification categories registered"
            );
        });

        if let Err(panic_val) = result {
            let msg = panic_val
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic_val.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown");
            tracing::warn!(
                panic = msg,
                "failed to register macOS notification categories"
            );
        }
    }

    /// # Errors
    /// Returns an error if the `spawn_blocking` task panics.
    pub async fn post_notification(
        &self,
        identifier: &str,
        text: NotificationText,
        category_id: &str,
        interruption_level: MacosInterruptionLevel,
        sound: bool,
        thread_id: &str,
    ) -> anyhow::Result<()> {
        let id = identifier.to_string();
        let cat_id = category_id.to_string();
        let thread = thread_id.to_string();
        let level = interruption_level;
        let play_sound = sound;

        tokio::task::spawn_blocking(move || {
            post_notification_sync(&id, text, &cat_id, level, play_sound, &thread);
        })
        .await
        .map_err(|e| anyhow::anyhow!("notification post task failed: {e}"))?;

        tracing::info!(
            identifier,
            category = category_id,
            "macOS notification submitted"
        );

        Ok(())
    }

    /// Uses a stable identifier so each new summary replaces the previous
    /// one in Notification Center instead of stacking.
    ///
    /// # Errors
    /// Returns an error if the `spawn_blocking` task panics.
    pub async fn post_summary(
        &self,
        title: &str,
        body: &str,
        interruption_level: MacosInterruptionLevel,
    ) -> anyhow::Result<()> {
        let text = NotificationText {
            title: title.to_string(),
            subtitle: String::new(),
            body: body.to_string(),
        };
        self.post_notification(
            "residuum-batch-summary",
            text,
            "background-results",
            interruption_level,
            self.config.sound,
            "background-results",
        )
        .await
    }

    pub fn handle_action(&self, action_id: &str, notification_id: &str) {
        match action_id {
            "open" => {
                if let Some(ref web_url) = self.config.web_url {
                    let url = format!("{web_url}/notification/{notification_id}");
                    open_url(&url);
                } else {
                    tracing::info!("open action received but no web_url configured, skipping");
                }
            }
            "mark-read" => {
                if let Some(ref tx) = self.ack_tx {
                    let ack = InboxAcknowledgment {
                        item_id: notification_id.to_string(),
                    };
                    if tx.try_send(ack).is_err() {
                        tracing::warn!(notification_id, "failed to send mark-read acknowledgment");
                    }
                } else {
                    tracing::warn!(
                        notification_id,
                        "mark-read action received but no ack sender configured"
                    );
                }
            }
            // "dismiss" is the default macOS action — no-op
            _ => {}
        }
    }

    #[must_use]
    pub fn config(&self) -> &MacosChannelConfig {
        &self.config
    }
}

/// Wraps objc2 calls in `catch_unwind` because the `ObjC` runtime can panic
/// on missing selectors or nil center — we log rather than crash the process.
fn post_notification_sync(
    identifier: &str,
    text: NotificationText,
    category_id: &str,
    level: MacosInterruptionLevel,
    sound: bool,
    thread_id: &str,
) {
    let result = std::panic::catch_unwind(|| {
        let content = UNMutableNotificationContent::new();

        let ns_title = NSString::from_str(&text.title);
        let ns_subtitle = NSString::from_str(&text.subtitle);
        let ns_body = NSString::from_str(&text.body);
        let ns_cat = NSString::from_str(category_id);
        let ns_thread = NSString::from_str(thread_id);

        content.setTitle(&ns_title);
        content.setSubtitle(&ns_subtitle);
        content.setBody(&ns_body);
        content.setCategoryIdentifier(&ns_cat);
        content.setThreadIdentifier(&ns_thread);

        let interruption_level = match level {
            MacosInterruptionLevel::Passive => UNNotificationInterruptionLevel::Passive,
            MacosInterruptionLevel::Active => UNNotificationInterruptionLevel::Active,
            MacosInterruptionLevel::TimeSensitive => UNNotificationInterruptionLevel::TimeSensitive,
        };
        content.setInterruptionLevel(interruption_level);

        if sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }

        let ns_id = NSString::from_str(identifier);
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&ns_id, &content, None);

        let center = UNUserNotificationCenter::currentNotificationCenter();

        let id_for_block = identifier.to_string();
        let block = RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!(identifier = %id_for_block, "macOS notification delivery error");
            }
        });

        center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
    });

    if result.is_err() {
        tracing::warn!(
            identifier,
            category_id,
            "panic in macOS notification posting"
        );
    }
}

/// Uses raw `msg_send!` because `objc2-app-kit` doesn't expose
/// `NSWorkspace.open(_:)` as a safe wrapper yet.
fn open_url(url_str: &str) {
    let result = std::panic::catch_unwind(|| {
        let ns_url_str = NSString::from_str(url_str);
        unsafe {
            let url: Option<Retained<objc2_foundation::NSURL>> =
                msg_send![objc2_foundation::NSURL::class(), URLWithString: &*ns_url_str];
            if let Some(url) = url {
                let cls_name = c"NSWorkspace";
                let workspace: Retained<objc2_foundation::NSObject> = msg_send![
                    objc2::runtime::AnyClass::get(cls_name).unwrap_unchecked(),
                    sharedWorkspace
                ];
                let _: Bool = msg_send![&*workspace, openURL: &*url];
            }
        }
    });

    if result.is_err() {
        tracing::warn!(url = url_str, "failed to open URL");
    }
}

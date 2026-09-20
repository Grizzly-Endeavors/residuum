//! Gateway-specific helpers: bus notifications.

use crate::bus::{ErrorEvent, NoticeEvent, NotifyName, Publisher, SYSTEM_CHANNEL, topics};

/// Publish a notice to the system notification channel.
pub(super) async fn publish_notice(publisher: &Publisher, message: String) {
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent { message },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish notice to bus");
    }
}

/// Publish an error to the system notification channel.
pub(super) async fn publish_error(publisher: &Publisher, message: String) {
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            ErrorEvent {
                correlation_id: String::new(),
                message,
            },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish error to bus");
    }
}

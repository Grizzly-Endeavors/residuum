//! Gateway-specific helpers: bus notifications.

use crate::bus::{
    ErrorEvent, NoticeEvent, NotifyName, Publisher, ReloadOutcome, SYSTEM_CHANNEL, topics,
};

/// Publish a notice to the system notification channel.
pub(crate) async fn publish_notice(publisher: &Publisher, message: String) {
    publish_notice_event(publisher, NoticeEvent::new(message)).await;
}

/// Publish a settings reload's outcome to the system notification channel.
/// `message` is the agent's own account of it, which the web keeps as the
/// detail behind its own wording of `outcome`.
pub(crate) async fn publish_reload_notice(
    publisher: &Publisher,
    outcome: ReloadOutcome,
    message: String,
) {
    publish_notice_event(publisher, NoticeEvent::reload(outcome, message)).await;
}

async fn publish_notice_event(publisher: &Publisher, notice: NoticeEvent) {
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            notice,
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish notice to bus");
    }
}

/// Publish an error to the system notification channel. `message` must be
/// plain language safe to show as-is; `details` carries the technical cause
/// chain behind a web UI details toggle (see [`ErrorEvent`]).
pub(crate) async fn publish_error(publisher: &Publisher, message: String, details: Option<String>) {
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            ErrorEvent {
                correlation_id: String::new(),
                message,
                details,
            },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish error to bus");
    }
}

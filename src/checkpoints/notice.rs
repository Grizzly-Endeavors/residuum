//! Publishing user-visible notices for the checkpoints system.

use crate::bus::Publisher;

/// Publish a plain-language notice to the system notification channel, if a
/// publisher is configured. Never fails the caller: a publish error is
/// logged and otherwise ignored.
pub(super) async fn publish(publisher: Option<&Publisher>, message: String) {
    if let Some(publisher) = publisher {
        crate::bus::notice::publish_notice(publisher, message).await;
    }
}

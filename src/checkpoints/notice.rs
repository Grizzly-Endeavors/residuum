//! Publishing user-visible notices for the checkpoints system.
//!
//! `crate::gateway::helpers::publish_notice` is `pub(super)` to the gateway
//! module and not reachable from here, so this is its own thin wrapper over
//! the same `crate::bus` primitives.

use crate::bus::{NoticeEvent, NotifyName, Publisher, SYSTEM_CHANNEL, topics};

/// Publish a plain-language notice to the system notification channel, if a
/// publisher is configured. Never fails the caller: a publish error is
/// logged and otherwise ignored.
pub(super) async fn publish(publisher: Option<&Publisher>, message: String) {
    let Some(publisher) = publisher else {
        return;
    };
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent { message },
        )
        .await
    {
        tracing::warn!(error = %e, "failed to publish checkpoints notice to bus");
    }
}

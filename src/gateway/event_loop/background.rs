//! Background task result handling and routing in the event loop.

use crate::background::types::{BackgroundResult, ResultRouting};
use crate::bus::{BusEvent, EventTrigger, NotificationEvent, Publisher, TopicId};
use crate::notify::types::TaskSource;

/// Bundled context for handling background task results.
pub struct BackgroundContext<'a> {
    pub publisher: &'a Publisher,
    pub tz: chrono_tz::Tz,
}

/// Handle a background task result: publish notifications to channel topics.
///
/// For each channel name in the result's routing:
/// - `"inbox"` → publish to `TopicId::Inbox`
/// - anything else → publish to `TopicId::Notify(name)`
///
/// If no subscriber exists for a topic, the event is silently undelivered.
pub(super) async fn handle_background_result(
    result: BackgroundResult,
    ctx: &BackgroundContext<'_>,
) {
    // Pulse HEARTBEAT_OK results are silently logged — no routing
    if matches!(result.source, TaskSource::Pulse) && result.summary.contains("HEARTBEAT_OK") {
        tracing::info!(task = %result.task_name, "pulse check: HEARTBEAT_OK");
        return;
    }

    let notification = NotificationEvent {
        title: result.task_name.clone(),
        content: result.summary.clone(),
        source: EventTrigger::from(result.source),
        timestamp: result.timestamp.with_timezone(&ctx.tz).naive_local(),
    };

    let ResultRouting::Direct(channels) = &result.routing;
    for channel_name in channels {
        let topic = if channel_name == "inbox" {
            TopicId::Inbox
        } else {
            TopicId::Notify(crate::bus::NotifyName::from(channel_name.as_str()))
        };
        drop(
            ctx.publisher
                .publish(topic, BusEvent::Notification(notification.clone()))
                .await,
        );
    }

    if channels.is_empty() {
        tracing::debug!(
            task = %result.task_name,
            "background result has no routing channels"
        );
    }
}

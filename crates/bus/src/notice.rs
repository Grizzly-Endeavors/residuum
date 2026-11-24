use super::{
    ErrorEvent, NoticeEvent, NotifyName, Publisher, ReloadOutcome, ReloadedFiles, SYSTEM_CHANNEL,
    topics,
};

pub async fn publish_notice(publisher: &Publisher, message: String) {
    publish_notice_event(publisher, NoticeEvent::new(message)).await;
}

pub async fn publish_reload_notice(
    publisher: &Publisher,
    outcome: ReloadOutcome,
    of: ReloadedFiles,
    message: String,
) {
    publish_notice_event(publisher, NoticeEvent::reload(outcome, of, message)).await;
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

pub async fn publish_error(publisher: &Publisher, message: String, details: Option<String>) {
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

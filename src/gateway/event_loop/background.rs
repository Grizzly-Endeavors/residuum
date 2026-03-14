//! Background result routing: subscribes to `TopicId::BackgroundResult` and
//! delivers notifications to the appropriate channels.

use tokio::task::JoinHandle;

use crate::bus::{
    AgentResultEvent, BusEvent, BusHandle, HeartbeatStatus, NotificationEvent, Publisher, TopicId,
};

/// Spawn a bus subscriber that routes `AgentResultEvent`s to notification channels.
///
/// Subscribes to `TopicId::BackgroundResult` and for each `BusEvent::AgentResult`:
/// - Filters out `HeartbeatStatus::Ok` results (silently logged)
/// - Converts to `NotificationEvent` and publishes to each channel in `event.routing`
///
/// This is a temporary router — Phase 8 replaces it with the LLM-powered notification router.
pub(crate) async fn spawn_result_router(
    bus_handle: &BusHandle,
    publisher: Publisher,
) -> Option<JoinHandle<()>> {
    let subscriber = match bus_handle.subscribe(TopicId::BackgroundResult).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to background:result topic");
            return None;
        }
    };

    Some(tokio::spawn(result_router_loop(subscriber, publisher)))
}

/// Main loop for the result router subscriber.
async fn result_router_loop(mut subscriber: crate::bus::Subscriber, publisher: Publisher) {
    while let Some(event) = subscriber.recv().await {
        if let BusEvent::AgentResult(agent_result) = event {
            route_agent_result(&agent_result, &publisher).await;
        }
    }
    tracing::info!("result router shutting down");
}

/// Route a single `AgentResultEvent` to notification channels.
async fn route_agent_result(event: &AgentResultEvent, publisher: &Publisher) {
    // Silently log heartbeat-ok results
    if event.heartbeat_status == HeartbeatStatus::Ok {
        tracing::info!(source_label = %event.source_label, "pulse check: HEARTBEAT_OK");
        return;
    }

    let notification = NotificationEvent {
        title: event.source_label.clone(),
        content: event.summary.clone(),
        source: event.source.clone(),
        timestamp: event.timestamp,
    };

    for channel_name in &event.routing {
        let topic = if channel_name == "inbox" {
            TopicId::Inbox
        } else {
            TopicId::Notify(crate::bus::NotifyName::from(channel_name.as_str()))
        };
        if let Err(e) = publisher
            .publish(topic.clone(), BusEvent::Notification(notification.clone()))
            .await
        {
            tracing::warn!(
                topic = %topic,
                source_label = %event.source_label,
                error = %e,
                "failed to publish notification to bus"
            );
        }
    }

    if event.routing.is_empty() {
        tracing::debug!(
            source_label = %event.source_label,
            "agent result has no routing channels"
        );
    }
}

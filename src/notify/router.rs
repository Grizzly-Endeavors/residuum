//! Notification router: subscribes to `TopicId::BackgroundResult` and delivers
//! each result according to the disposition its producing agent declared.
//!
//! Routing is a match on `ResultDisposition`, decided upstream by the agent that
//! ran the task:
//! - `Silent` → discard
//! - agent-spawned → relay to the main agent
//! - `Normal` → inbox
//! - `Urgent` → inbox and every configured notification channel

use tokio::task::JoinHandle;

use crate::bus::{
    AgentResultEvent, BusHandle, EndpointRegistry, EventTrigger, NotificationEvent, Publisher,
    ResultDisposition, Subscriber, topics,
};

/// Spawn the notification router as a bus subscriber.
///
/// Subscribes to `TopicId::BackgroundResult` and routes each `AgentResultEvent`
/// by its declared disposition.
///
/// Returns `None` if subscription fails.
pub(crate) async fn spawn_notification_router(
    bus_handle: &BusHandle,
    endpoint_registry: EndpointRegistry,
    publisher: Publisher,
) -> Option<JoinHandle<()>> {
    let subscriber = match bus_handle.subscribe(topics::Background).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to subscribe to background:result topic");
            return None;
        }
    };

    let router = NotificationRouter {
        endpoint_registry,
        publisher,
    };

    Some(tokio::spawn(router_loop(subscriber, router)))
}

/// The notification router state.
struct NotificationRouter {
    endpoint_registry: EndpointRegistry,
    publisher: Publisher,
}

/// Main loop: receive agent results and route them.
async fn router_loop(mut subscriber: Subscriber<AgentResultEvent>, router: NotificationRouter) {
    loop {
        match subscriber.recv().await {
            Ok(Some(agent_result)) => route_agent_result(&agent_result, &router).await,
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "notification router subscriber error, shutting down");
                break;
            }
        }
    }
    tracing::info!("notification router shutting down");
}

/// Route a single `AgentResultEvent` by its declared disposition.
#[tracing::instrument(skip_all, fields(source_label = %event.source_label, task_id = %event.task_id))]
async fn route_agent_result(event: &AgentResultEvent, router: &NotificationRouter) {
    // Silent results are the majority of traffic through this path and are a
    // discard by design, so check for them before logging anything at info —
    // otherwise routine health-check pulses would spam the info log.
    if event.disposition == ResultDisposition::Silent {
        tracing::trace!("pulse check: nothing to report");
        return;
    }

    tracing::info!(
        source = %event.source,
        disposition = ?event.disposition,
        "notification router received result"
    );

    // Agent-spawned results are relayed to the agent that asked for them.
    if matches!(event.source, EventTrigger::Agent) {
        tracing::info!("routing agent-spawned result to main agent");
        publish_to_agent_main(event, &router.publisher).await;
        return;
    }

    let urgent = event.disposition == ResultDisposition::Urgent;
    let targets = delivery_targets(&router.endpoint_registry, urgent);
    tracing::info!(targets = ?targets, urgent, "delivering result");
    publish_to_targets(event, &targets, urgent, &router.publisher).await;
}

/// Inbox always; every configured notification channel as well when urgent.
fn delivery_targets(registry: &EndpointRegistry, urgent: bool) -> Vec<String> {
    let mut targets = vec![INBOX_TARGET.to_string()];
    if urgent {
        targets.extend(
            registry
                .notify()
                .into_iter()
                .map(|e| e.id.as_ref().to_string()),
        );
    }
    targets
}

const INBOX_TARGET: &str = "inbox";

/// Publish a result as a `MessageEvent` to the `UserMessage` topic.
async fn publish_to_agent_main(event: &AgentResultEvent, publisher: &Publisher) {
    let content = format_agent_result_message(event);

    let msg_event = crate::bus::MessageEvent {
        id: format!("bg-result-{}", event.task_id),
        content,
        origin: crate::interfaces::types::MessageOrigin {
            endpoint: "background".to_string(),
            sender_name: "background-task".to_string(),
            sender_id: event.task_id.clone(),
        },
        timestamp: event.timestamp,
        images: vec![],
    };

    if let Err(e) = publisher.publish(topics::UserMessage, msg_event).await {
        tracing::warn!(
            task_id = %event.task_id,
            error = %e,
            "failed to publish background result to user:message"
        );
    }
}

/// Format an `AgentResultEvent` into a human-readable message for the main agent.
fn format_agent_result_message(event: &AgentResultEvent) -> String {
    let source_kind = event.source.as_str();
    let status = match &event.status {
        crate::bus::AgentResultStatus::Completed => "completed".to_string(),
        crate::bus::AgentResultStatus::Cancelled => "cancelled".to_string(),
        crate::bus::AgentResultStatus::Failed { error } => format!("failed: {error}"),
    };

    let mut parts = vec![format!(
        "[Background Task Result]\nTask: {} ({})\nSource: {}\nStatus: {}",
        event.source_label, event.task_id, source_kind, status
    )];

    if !event.summary.is_empty() {
        parts.push(format!("Output:\n{}", event.summary));
    }

    if let Some(path) = &event.transcript_path {
        parts.push(format!("Transcript: {}", path.display()));
    }

    parts.join("\n")
}

/// Publish notifications to the specified targets.
async fn publish_to_targets(
    event: &AgentResultEvent,
    targets: &[String],
    urgent: bool,
    publisher: &Publisher,
) {
    let notification = NotificationEvent {
        title: event.source_label.clone(),
        content: event.summary.clone(),
        source: event.source.clone(),
        urgent,
        timestamp: event.timestamp,
    };

    for target in targets {
        if target == INBOX_TARGET {
            if let Err(e) = publisher.publish(topics::Inbox, notification.clone()).await {
                tracing::warn!(
                    topic = "inbox",
                    source_label = %event.source_label,
                    error = %e,
                    "failed to publish notification to bus"
                );
            }
        } else {
            let topic = topics::Notification(crate::bus::NotifyName::from(target.as_str()));
            if let Err(e) = publisher.publish(topic, notification.clone()).await {
                tracing::warn!(
                    topic = %target,
                    source_label = %event.source_label,
                    error = %e,
                    "failed to publish notification to bus"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{AgentResultStatus, EndpointCapabilities, EndpointEntry, NotifyName, TopicId};
    use chrono::NaiveDate;

    fn sample_timestamp() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 14)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn sample_event(disposition: ResultDisposition) -> AgentResultEvent {
        AgentResultEvent {
            task_id: "t1".into(),
            source_label: "pulse:email_check".into(),
            agent_skill: None,
            source: EventTrigger::Pulse,
            disposition,
            status: AgentResultStatus::Completed,
            summary: "3 new emails found".into(),
            transcript_path: None,
            timestamp: sample_timestamp(),
        }
    }

    fn registry_with_channels(names: &[&str]) -> EndpointRegistry {
        let reg = EndpointRegistry::new();
        for name in names {
            reg.register(EndpointEntry {
                id: crate::bus::EndpointId::from(*name),
                topic: TopicId::Notification(NotifyName::from(*name)),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: (*name).to_string(),
            });
        }
        reg
    }

    #[test]
    fn normal_result_goes_to_inbox_only() {
        let reg = registry_with_channels(&["ntfy_phone", "ops_hook"]);
        let targets = delivery_targets(&reg, false);
        assert_eq!(targets, vec!["inbox"]);
    }

    #[test]
    fn urgent_result_fans_out_to_every_channel() {
        let reg = registry_with_channels(&["ntfy_phone", "ops_hook"]);
        let mut targets = delivery_targets(&reg, true);
        targets.sort();
        assert_eq!(targets, vec!["inbox", "ntfy_phone", "ops_hook"]);
    }

    #[test]
    fn urgent_with_no_channels_configured_still_reaches_inbox() {
        let reg = registry_with_channels(&[]);
        let targets = delivery_targets(&reg, true);
        assert_eq!(
            targets,
            vec!["inbox"],
            "an urgent result must never be dropped for want of a push channel"
        );
    }

    #[test]
    fn interactive_endpoints_are_never_delivery_targets() {
        let reg = registry_with_channels(&["ntfy_phone"]);
        reg.register(EndpointEntry {
            id: crate::bus::EndpointId::from("websocket"),
            topic: TopicId::Notification(NotifyName::from("websocket")),
            capabilities: EndpointCapabilities::INTERACTIVE,
            display_name: "websocket".to_string(),
        });

        let targets = delivery_targets(&reg, true);
        assert!(
            !targets.iter().any(|t| t == "websocket"),
            "interactive endpoints are reachable only via send_message"
        );
    }

    #[test]
    fn format_agent_result_message_completed() {
        let event = sample_event(ResultDisposition::Normal);
        let msg = format_agent_result_message(&event);
        assert!(msg.contains("[Background Task Result]"));
        assert!(msg.contains("pulse:email_check"));
        assert!(msg.contains("completed"));
        assert!(msg.contains("3 new emails found"));
    }

    #[test]
    fn format_agent_result_message_failed() {
        let mut event = sample_event(ResultDisposition::Normal);
        event.status = AgentResultStatus::Failed {
            error: "connection refused".into(),
        };
        event.summary = String::new();

        let msg = format_agent_result_message(&event);
        assert!(msg.contains("failed: connection refused"));
        assert!(
            !msg.contains("Error: connection refused"),
            "error should not be duplicated in a separate Error: line"
        );
    }

    #[test]
    fn format_agent_result_message_cancelled() {
        let mut event = sample_event(ResultDisposition::Normal);
        event.status = AgentResultStatus::Cancelled;
        event.summary = String::new();

        let msg = format_agent_result_message(&event);
        assert!(msg.contains("[Background Task Result]"));
        assert!(msg.contains("cancelled"));
    }

    #[test]
    fn format_agent_result_message_with_transcript() {
        let mut event = sample_event(ResultDisposition::Normal);
        event.transcript_path = Some(std::path::PathBuf::from("/var/log/residuum/t1.transcript"));

        let msg = format_agent_result_message(&event);
        assert!(msg.contains("Transcript:"));
        assert!(msg.contains("t1.transcript"));
    }

    #[tokio::test]
    async fn silent_result_is_delivered_nowhere() {
        let handle = crate::bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Silent), &router).await;

        let got = tokio::time::timeout(std::time::Duration::from_millis(100), inbox_sub.recv())
            .await
            .ok();
        assert!(got.is_none(), "silent results must not reach the inbox");
    }

    #[tokio::test]
    async fn urgent_result_reaches_inbox_and_channel() {
        let handle = crate::bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let mut ntfy_sub: Subscriber<NotificationEvent> = handle
            .subscribe(topics::Notification(NotifyName::from("ntfy_phone")))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Urgent), &router).await;

        let inbox_item =
            tokio::time::timeout(std::time::Duration::from_millis(200), inbox_sub.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        assert!(inbox_item.urgent, "urgency must survive to the channel");

        let pushed = tokio::time::timeout(std::time::Duration::from_millis(200), ntfy_sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pushed.title, "pulse:email_check");
        assert!(pushed.urgent);
    }

    #[tokio::test]
    async fn normal_result_does_not_reach_a_channel() {
        let handle = crate::bus::spawn_broker();
        let mut ntfy_sub: Subscriber<NotificationEvent> = handle
            .subscribe(topics::Notification(NotifyName::from("ntfy_phone")))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Normal), &router).await;

        let got = tokio::time::timeout(std::time::Duration::from_millis(100), ntfy_sub.recv())
            .await
            .ok();
        assert!(got.is_none(), "only urgent results should push");
    }

    #[tokio::test]
    async fn agent_spawned_result_relays_to_the_main_agent() {
        let handle = crate::bus::spawn_broker();
        let mut user_sub = handle.subscribe(topics::UserMessage).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Agent;
        route_agent_result(&event, &router).await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), user_sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(msg.content.contains("[Background Task Result]"));
    }
}

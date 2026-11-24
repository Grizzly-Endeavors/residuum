use tokio::task::JoinHandle;

use residuum_bus::{
    AgentResultEvent, AgentResultStatus, BusHandle, EndpointRegistry, EventTrigger, NoticeEvent,
    NotificationEvent, NotifyName, Publisher, ResultDisposition, SYSTEM_CHANNEL, Subscriber,
    topics,
};

pub async fn spawn_notification_router(
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

    Some(residuum_core::util::spawn_in_span(router_loop(
        subscriber, router,
    )))
}

struct NotificationRouter {
    endpoint_registry: EndpointRegistry,
    publisher: Publisher,
}

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

#[tracing::instrument(skip_all, fields(source_label = %event.source_label, session_address = %event.session_address, run_id = %event.run_id))]
async fn route_agent_result(event: &AgentResultEvent, router: &NotificationRouter) {
    if event.disposition == ResultDisposition::Silent {
        tracing::trace!("pulse check: nothing to report");
        return;
    }

    if matches!(event.source, EventTrigger::Agent) {
        tracing::trace!("spawned session result: already relayed per-turn, nothing to do here");
        return;
    }

    if matches!(event.source, EventTrigger::Artifact(_)) {
        tracing::trace!("artifact session result: stays with its artifact, nothing to route");
        return;
    }

    if matches!(event.source, EventTrigger::Conversation) {
        tracing::trace!(
            "conversation session result: already delivered to its conversation, nothing to route"
        );
        return;
    }

    tracing::info!(
        source = %event.source,
        disposition = ?event.disposition,
        "notification router received result"
    );

    let urgent = event.disposition == ResultDisposition::Urgent;
    let targets = delivery_targets(&router.endpoint_registry, urgent);
    tracing::info!(targets = ?targets, urgent, "delivering result");
    publish_to_targets(event, &targets, urgent, &router.publisher).await;

    if matches!(event.status, AgentResultStatus::Failed { .. })
        && matches!(event.source, EventTrigger::Pulse | EventTrigger::Action)
    {
        publish_failed_scheduled_run_notice(event, &router.publisher).await;
    }
}

async fn publish_failed_scheduled_run_notice(event: &AgentResultEvent, publisher: &Publisher) {
    let AgentResultStatus::Failed { error, .. } = &event.status else {
        return;
    };
    let kind = match event.source {
        EventTrigger::Pulse => "pulse",
        EventTrigger::Action => "scheduled action",
        EventTrigger::Agent
        | EventTrigger::Webhook(_)
        | EventTrigger::Conversation
        | EventTrigger::Artifact(_) => "scheduled run",
    };
    let name = event
        .source_label
        .split_once(':')
        .map_or(event.source_label.as_str(), |(_, name)| name);
    let message = format!("Your {kind} \"{name}\" failed and didn't finish its run: {error}");
    if let Err(e) = publisher
        .publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent::new(message),
        )
        .await
    {
        tracing::warn!(source_label = %event.source_label, error = %e, "failed to publish failed-scheduled-run notice");
    }
}

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

fn notification_content(event: &AgentResultEvent) -> String {
    match &event.status {
        AgentResultStatus::Completed => event.summary.clone(),
        AgentResultStatus::Cancelled => append_status_line(
            &event.summary,
            "This run was stopped before it finished.".to_string(),
        ),
        AgentResultStatus::Failed { error, .. } => {
            append_status_line(&event.summary, format!("This run failed: {error}"))
        }
    }
}

fn append_status_line(summary: &str, line: String) -> String {
    if summary.is_empty() {
        line
    } else {
        format!("{summary}\n\n{line}")
    }
}

async fn publish_to_targets(
    event: &AgentResultEvent,
    targets: &[String],
    urgent: bool,
    publisher: &Publisher,
) {
    let notification = NotificationEvent {
        title: event.source_label.clone(),
        content: notification_content(event),
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
            let topic = topics::Notification(residuum_bus::NotifyName::from(target.as_str()));
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
    use chrono::NaiveDate;
    use residuum_bus::{EndpointCapabilities, EndpointEntry, TopicId};
    use residuum_core::testing::{clock, wait};

    fn sample_timestamp() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 14)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn sample_event(disposition: ResultDisposition) -> AgentResultEvent {
        AgentResultEvent {
            session_address: residuum_bus::SessionAddress::from("scheduled-email-check-0001"),
            run_id: "t1".into(),
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
        EndpointRegistry::from_entries(names.iter().map(|name| EndpointEntry {
            id: residuum_bus::EndpointId::from(*name),
            topic: TopicId::Notification(NotifyName::from(*name)),
            capabilities: EndpointCapabilities::NOTIFY_ONLY,
            display_name: (*name).to_string(),
        }))
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
        let reg = EndpointRegistry::from_entries([
            EndpointEntry {
                id: residuum_bus::EndpointId::from("ntfy_phone"),
                topic: TopicId::Notification(NotifyName::from("ntfy_phone")),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: "ntfy_phone".to_string(),
            },
            EndpointEntry {
                id: residuum_bus::EndpointId::from("websocket"),
                topic: TopicId::Notification(NotifyName::from("websocket")),
                capabilities: EndpointCapabilities::INTERACTIVE,
                display_name: "websocket".to_string(),
            },
        ]);

        let targets = delivery_targets(&reg, true);
        assert!(
            !targets.iter().any(|t| t == "websocket"),
            "interactive endpoints are reachable only via send_message"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silent_result_is_delivered_nowhere() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Silent), &router).await;

        let got = clock::within(std::time::Duration::from_millis(100), inbox_sub.recv()).await;
        assert!(got.is_none(), "silent results must not reach the inbox");
    }

    #[tokio::test]
    async fn urgent_result_reaches_inbox_and_channel() {
        let handle = residuum_bus::spawn_broker();
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

        let inbox_item = wait::guarded("the inbox item", inbox_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(inbox_item.urgent, "urgency must survive to the channel");

        let pushed = wait::guarded("the ntfy push", ntfy_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pushed.title, "pulse:email_check");
        assert!(pushed.urgent);
    }

    #[tokio::test(start_paused = true)]
    async fn normal_result_does_not_reach_a_channel() {
        let handle = residuum_bus::spawn_broker();
        let mut ntfy_sub: Subscriber<NotificationEvent> = handle
            .subscribe(topics::Notification(NotifyName::from("ntfy_phone")))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Normal), &router).await;

        let got = clock::within(std::time::Duration::from_millis(100), ntfy_sub.recv()).await;
        assert!(got.is_none(), "only urgent results should push");
    }

    #[tokio::test(start_paused = true)]
    async fn agent_spawned_result_is_discarded_here_not_relayed_to_main() {
        let handle = residuum_bus::spawn_broker();
        let mut user_sub = handle.subscribe(topics::UserMessage).await.unwrap();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Agent;
        route_agent_result(&event, &router).await;

        assert!(
            clock::within(std::time::Duration::from_millis(100), user_sub.recv())
                .await
                .is_none(),
            "an agent-spawned result must not be relayed to main by this router"
        );
        assert!(
            clock::within(std::time::Duration::from_millis(100), inbox_sub.recv())
                .await
                .is_none(),
            "an agent-spawned result must not leak into the inbox either"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_session_result_reaches_neither_inbox_nor_channels_even_when_urgent() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let mut ntfy_sub: Subscriber<NotificationEvent> = handle
            .subscribe(topics::Notification(NotifyName::from("ntfy_phone")))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Urgent);
        event.source = EventTrigger::Conversation;
        event.source_label = "discord:#builds".to_string();
        route_agent_result(&event, &router).await;

        let wait = std::time::Duration::from_millis(100);
        assert!(
            clock::within(wait, inbox_sub.recv()).await.is_none(),
            "an urgent conversation session's result must not be filed to the inbox"
        );
        assert!(
            clock::within(wait, ntfy_sub.recv()).await.is_none(),
            "an urgent conversation session's result must not push to notification channels"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn conversation_session_result_stays_out_of_the_inbox_when_normal() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Conversation;
        event.source_label = "a2a:laptop".to_string();
        route_agent_result(&event, &router).await;

        assert!(
            clock::within(std::time::Duration::from_millis(100), inbox_sub.recv())
                .await
                .is_none(),
            "a normal conversation session's result must not be filed to the inbox either"
        );
    }

    #[tokio::test]
    async fn webhook_triggered_result_still_reaches_the_inbox() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Webhook("gh".into());
        event.source_label = "webhook:gh".to_string();
        route_agent_result(&event, &router).await;

        let inbox_item = wait::guarded("the inbox item", inbox_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inbox_item.title, "webhook:gh");
    }

    #[tokio::test(start_paused = true)]
    async fn artifact_session_result_reaches_neither_inbox_nor_channels_even_when_urgent() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let mut user_sub = handle.subscribe(topics::UserMessage).await.unwrap();
        let mut ntfy_sub: Subscriber<NotificationEvent> = handle
            .subscribe(topics::Notification(NotifyName::from("ntfy_phone")))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&["ntfy_phone"]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Urgent);
        event.source = EventTrigger::Artifact("wiki".into());
        event.source_label = "artifact:wiki".to_string();
        route_agent_result(&event, &router).await;

        let wait = std::time::Duration::from_millis(100);
        assert!(
            clock::within(wait, inbox_sub.recv()).await.is_none(),
            "an artifact session's result must not be filed to the inbox"
        );
        assert!(
            clock::within(wait, ntfy_sub.recv()).await.is_none(),
            "an artifact session's result must not push to notification channels"
        );
        assert!(
            clock::within(wait, user_sub.recv()).await.is_none(),
            "an artifact session's result must not reach the main conversation"
        );
    }

    #[tokio::test]
    async fn failed_pulse_inbox_item_carries_the_failure_reason() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.status = AgentResultStatus::Failed {
            error: "the model call timed out".to_string(),
            details: None,
        };
        event.summary = String::new();
        route_agent_result(&event, &router).await;

        let inbox_item = wait::guarded("the inbox item", inbox_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            inbox_item.content.contains("the model call timed out"),
            "the inbox item body must name why the run failed, not just the timestamp line: {}",
            inbox_item.content
        );
    }

    #[tokio::test]
    async fn stopped_pulse_inbox_item_says_it_was_stopped() {
        let handle = residuum_bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.status = AgentResultStatus::Cancelled;
        event.summary = String::new();
        route_agent_result(&event, &router).await;

        let inbox_item = wait::guarded("the inbox item", inbox_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            inbox_item.content.contains("stopped"),
            "the inbox item body must say the run was stopped: {}",
            inbox_item.content
        );
    }

    #[test]
    fn completed_run_content_is_the_summary_verbatim() {
        let event = sample_event(ResultDisposition::Normal);
        assert_eq!(notification_content(&event), "3 new emails found");
    }

    #[tokio::test]
    async fn failed_pulse_publishes_an_owner_notice() {
        let handle = residuum_bus::spawn_broker();
        let mut notice_sub: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Pulse;
        event.source_label = "pulse:email_check".to_string();
        event.status = AgentResultStatus::Failed {
            error: "the model call timed out".to_string(),
            details: None,
        };
        route_agent_result(&event, &router).await;

        let notice = wait::guarded("the owner notice", notice_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(notice.message.contains("email_check"));
        assert!(notice.message.contains("the model call timed out"));
    }

    #[tokio::test]
    async fn failed_action_publishes_an_owner_notice_naming_the_action() {
        let handle = residuum_bus::spawn_broker();
        let mut notice_sub: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Action;
        event.source_label = "action:nightly digest".to_string();
        event.status = AgentResultStatus::Failed {
            error: "skill not found".to_string(),
            details: None,
        };
        route_agent_result(&event, &router).await;

        let notice = wait::guarded("the owner notice", notice_sub.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(notice.message.contains("scheduled action"));
        assert!(notice.message.contains("nightly digest"));
        assert!(notice.message.contains("skill not found"));
    }

    #[tokio::test(start_paused = true)]
    async fn completed_pulse_publishes_no_failure_notice() {
        let handle = residuum_bus::spawn_broker();
        let mut notice_sub: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        route_agent_result(&sample_event(ResultDisposition::Normal), &router).await;

        assert!(
            clock::within(std::time::Duration::from_millis(100), notice_sub.recv())
                .await
                .is_none(),
            "a completed run must never publish a failure notice"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_spawned_session_publishes_no_owner_notice() {
        let handle = residuum_bus::spawn_broker();
        let mut notice_sub: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Agent;
        event.status = AgentResultStatus::Failed {
            error: "boom".to_string(),
            details: None,
        };
        route_agent_result(&event, &router).await;

        assert!(
            clock::within(std::time::Duration::from_millis(100), notice_sub.recv())
                .await
                .is_none()
        );
    }
}

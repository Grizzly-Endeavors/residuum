//! Notification router: subscribes to `TopicId::Background` and delivers
//! each result according to the disposition its producing agent declared.
//!
//! Routing is a match on `ResultDisposition`, decided upstream by the agent that
//! ran the task:
//! - `Silent` → discard
//! - agent-spawned (`spawned` sessions) → discard here; each turn's result is
//!   already relayed to the session's direct spawner as it happens, via
//!   `AgentMessenger` from the session runtime (see
//!   `crate::background::runtime::relay_result_to_spawner`), not through this
//!   router
//! - artifact-started (`artifact` sessions) → discard; their output reaches
//!   the artifact that started them through the session stream, and is never
//!   filed to the inbox or pushed to notification channels on its own
//! - conversation-started (`external` sessions with `EventTrigger::Conversation`,
//!   i.e. A2A callers and non-owner Discord/Telegram/Teams chats) → discard;
//!   the session's output already went back to the conversation it came from,
//!   and its observations are merged into memory as an episode, so filing it
//!   to the inbox or a notification channel would be pure noise
//! - `Normal` → inbox
//! - `Urgent` → inbox and every configured notification channel

use tokio::task::JoinHandle;

use crate::bus::{
    AgentResultEvent, BusHandle, EndpointRegistry, EventTrigger, NotificationEvent, Publisher,
    ResultDisposition, Subscriber, topics,
};

/// Spawn the notification router as a bus subscriber.
///
/// Subscribes to `TopicId::Background` and routes each `AgentResultEvent`
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
#[tracing::instrument(skip_all, fields(source_label = %event.source_label, session_address = %event.session_address, run_id = %event.run_id))]
async fn route_agent_result(event: &AgentResultEvent, router: &NotificationRouter) {
    // Silent results are the majority of traffic through this path and are a
    // discard by design, so check for them before logging anything at info —
    // otherwise routine health-check pulses would spam the info log.
    if event.disposition == ResultDisposition::Silent {
        tracing::trace!("pulse check: nothing to report");
        return;
    }

    // A `spawned` session's result is relayed to its direct spawner as each
    // turn happens (see `crate::background::runtime::relay_result_to_spawner`),
    // not through this router — the disposition rules below (inbox, urgent
    // fanout) are for `scheduled` results only.
    if matches!(event.source, EventTrigger::Agent) {
        tracing::trace!("spawned session result: already relayed per-turn, nothing to do here");
        return;
    }

    // An `artifact` session's output belongs to the artifact that started
    // it, which follows the session's own stream. Filing it to the inbox or
    // notifying on it would surface work the user started from a page as if
    // the agent had produced it unprompted.
    if matches!(event.source, EventTrigger::Artifact(_)) {
        tracing::trace!("artifact session result: stays with its artifact, nothing to route");
        return;
    }

    // A `conversation` session's output already went back to the conversation
    // it came from, and its observations are merged into memory as an
    // episode. Filing it to the inbox or notifying on it would surface the
    // same content twice — once where it belongs, once as manufactured noise.
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
            session_address: crate::bus::SessionAddress::from("scheduled-email-check-0001"),
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
            id: crate::bus::EndpointId::from(*name),
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
                id: crate::bus::EndpointId::from("ntfy_phone"),
                topic: TopicId::Notification(NotifyName::from("ntfy_phone")),
                capabilities: EndpointCapabilities::NOTIFY_ONLY,
                display_name: "ntfy_phone".to_string(),
            },
            EndpointEntry {
                id: crate::bus::EndpointId::from("websocket"),
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
    async fn agent_spawned_result_is_discarded_here_not_relayed_to_main() {
        // The per-turn relay to a spawned session's direct spawner now
        // happens straight from the session runtime via `AgentMessenger` —
        // this router must not also relay it to main (which would double up
        // for main-spawned sessions, and be outright wrong for a session
        // spawned by another session).
        let handle = crate::bus::spawn_broker();
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
            tokio::time::timeout(std::time::Duration::from_millis(100), user_sub.recv())
                .await
                .is_err(),
            "an agent-spawned result must not be relayed to main by this router"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), inbox_sub.recv())
                .await
                .is_err(),
            "an agent-spawned result must not leak into the inbox either"
        );
    }

    #[tokio::test]
    async fn conversation_session_result_reaches_neither_inbox_nor_channels_even_when_urgent() {
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

        let mut event = sample_event(ResultDisposition::Urgent);
        event.source = EventTrigger::Conversation;
        event.source_label = "discord:#builds".to_string();
        route_agent_result(&event, &router).await;

        let wait = std::time::Duration::from_millis(100);
        assert!(
            tokio::time::timeout(wait, inbox_sub.recv()).await.is_err(),
            "an urgent conversation session's result must not be filed to the inbox"
        );
        assert!(
            tokio::time::timeout(wait, ntfy_sub.recv()).await.is_err(),
            "an urgent conversation session's result must not push to notification channels"
        );
    }

    #[tokio::test]
    async fn conversation_session_result_stays_out_of_the_inbox_when_normal() {
        let handle = crate::bus::spawn_broker();
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
            tokio::time::timeout(std::time::Duration::from_millis(100), inbox_sub.recv())
                .await
                .is_err(),
            "a normal conversation session's result must not be filed to the inbox either"
        );
    }

    #[tokio::test]
    async fn webhook_triggered_result_still_reaches_the_inbox() {
        let handle = crate::bus::spawn_broker();
        let mut inbox_sub = handle.subscribe(topics::Inbox).await.unwrap();
        let router = NotificationRouter {
            endpoint_registry: registry_with_channels(&[]),
            publisher: handle.publisher(),
        };

        let mut event = sample_event(ResultDisposition::Normal);
        event.source = EventTrigger::Webhook("gh".into());
        event.source_label = "webhook:gh".to_string();
        route_agent_result(&event, &router).await;

        let inbox_item =
            tokio::time::timeout(std::time::Duration::from_millis(200), inbox_sub.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        assert_eq!(inbox_item.title, "webhook:gh");
    }

    #[tokio::test]
    async fn artifact_session_result_reaches_neither_inbox_nor_channels_even_when_urgent() {
        let handle = crate::bus::spawn_broker();
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
            tokio::time::timeout(wait, inbox_sub.recv()).await.is_err(),
            "an artifact session's result must not be filed to the inbox"
        );
        assert!(
            tokio::time::timeout(wait, ntfy_sub.recv()).await.is_err(),
            "an artifact session's result must not push to notification channels"
        );
        assert!(
            tokio::time::timeout(wait, user_sub.recv()).await.is_err(),
            "an artifact session's result must not reach the main conversation"
        );
    }
}

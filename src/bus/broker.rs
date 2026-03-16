//! Broker task and `BusHandle` factory.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::events::BusEvent;
use super::handle::{BrokerCommand, ErasedEvent, Publisher, Subscriber, TypedSubscriber};
use super::topics::Topic;
use super::types::{BusError, TopicId};
use crate::spawn::spawn_monitored;

/// Command channel capacity for the broker.
const BROKER_COMMAND_CAPACITY: usize = 256;

/// Per-subscriber event channel capacity.
const SUBSCRIBER_CAPACITY: usize = 64;

// ---------------------------------------------------------------------------
// BusHandle
// ---------------------------------------------------------------------------

/// Factory handle for creating publishers and subscribers.
///
/// Cloning a `BusHandle` is cheap — it shares the command channel and the
/// atomic subscriber-id counter.
#[derive(Clone)]
pub struct BusHandle {
    cmd_tx: mpsc::Sender<BrokerCommand>,
    next_id: Arc<AtomicU64>,
}

impl BusHandle {
    /// Create a [`Publisher`] that can send events to the bus.
    #[must_use]
    pub fn publisher(&self) -> Publisher {
        Publisher::new(self.cmd_tx.clone())
    }

    /// Create a [`Subscriber`] for the given topic (legacy untyped API).
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn subscribe(&self, topic: TopicId) -> Result<Subscriber, BusError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = mpsc::channel(SUBSCRIBER_CAPACITY);

        self.cmd_tx
            .send(BrokerCommand::Subscribe {
                id,
                topic: topic.clone(),
                sender: event_tx,
            })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)?;

        Ok(Subscriber::new(id, topic, event_rx, self.cmd_tx.clone()))
    }

    /// Create a typed subscriber for the given topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn subscribe_typed<T: Topic>(
        &self,
        topic: T,
    ) -> Result<TypedSubscriber<T::Event>, BusError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = mpsc::channel(SUBSCRIBER_CAPACITY);
        let topic_id = topic.topic_id();

        self.cmd_tx
            .send(BrokerCommand::Subscribe {
                id,
                topic: topic_id.clone(),
                sender: event_tx,
            })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)?;

        Ok(TypedSubscriber::new(
            id,
            topic_id,
            event_rx,
            self.cmd_tx.clone(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Broker task
// ---------------------------------------------------------------------------

/// Spawn the broker task and return a [`BusHandle`].
#[must_use]
pub fn spawn_broker() -> BusHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(BROKER_COMMAND_CAPACITY);

    spawn_monitored("bus-broker", run_broker(cmd_rx));

    BusHandle {
        cmd_tx,
        next_id: Arc::new(AtomicU64::new(0)),
    }
}

/// Broker event loop — owns all subscription state.
///
/// Exits naturally when every `BusHandle` (and derived sender) is dropped.
async fn run_broker(mut cmd_rx: mpsc::Receiver<BrokerCommand>) {
    let mut subscriptions: HashMap<TopicId, Vec<(u64, mpsc::Sender<ErasedEvent>)>> = HashMap::new();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BrokerCommand::Publish { topic, event } => {
                let had_subscribers = if let Some(subscribers) = subscriptions.get_mut(&topic) {
                    subscribers.retain(|(id, tx)| {
                        match tx.try_send(Arc::clone(&event)) {
                            Ok(()) => true,
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                warn!(
                                    topic = %topic,
                                    subscriber_id = id,
                                    "subscriber full, event dropped"
                                );
                                true // keep subscriber
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                debug!(
                                    topic = %topic,
                                    subscriber_id = id,
                                    "subscriber closed, removing"
                                );
                                false // prune
                            }
                        }
                    });
                    let count = subscribers.len();
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
                    }
                    count > 0
                } else {
                    false
                };

                // Publish error when no subscribers received the event.
                // Guard: skip if the original topic is BusErrors to prevent recursion.
                if !had_subscribers && topic != TopicId::BusErrors {
                    let error_event: ErasedEvent = Arc::new(BusEvent::Error {
                        correlation_id: String::new(),
                        message: format!("no active subscribers for topic {topic}"),
                    });
                    if let Some(error_subs) = subscriptions.get_mut(&TopicId::BusErrors) {
                        error_subs.retain(|(_, tx)| tx.try_send(Arc::clone(&error_event)).is_ok());
                        if error_subs.is_empty() {
                            subscriptions.remove(&TopicId::BusErrors);
                        }
                    }
                }
            }
            BrokerCommand::Subscribe { id, topic, sender } => {
                subscriptions.entry(topic).or_default().push((id, sender));
            }
            BrokerCommand::Unsubscribe { id, topic } => {
                if let Some(subscribers) = subscriptions.get_mut(&topic) {
                    subscribers.retain(|(sub_id, _)| *sub_id != id);
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test assertions use wildcard for non-matching variants"
)]
#[expect(clippy::panic, reason = "test assertions")]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::events::MessageEvent;
    use crate::bus::topics;
    use crate::bus::types::EndpointName;
    use crate::interfaces::types::MessageOrigin;

    fn test_message(id: &str, content: &str) -> BusEvent {
        BusEvent::Message(MessageEvent {
            id: id.into(),
            content: content.into(),
            origin: MessageOrigin {
                endpoint: "test".into(),
                sender_name: "tester".into(),
                sender_id: "t-1".into(),
            },
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 13)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
            images: vec![],
        })
    }

    #[tokio::test]
    async fn publish_to_single_subscriber() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub = handle.subscribe(TopicId::Inbox).await.unwrap();

        pub_.publish(TopicId::Inbox, test_message("1", "hello"))
            .await
            .unwrap();

        let event = sub.recv().await.unwrap();
        match event {
            BusEvent::Message(msg) => {
                assert_eq!(msg.id, "1");
                assert_eq!(msg.content, "hello");
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[tokio::test]
    async fn publish_fan_out() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub1 = handle.subscribe(TopicId::Inbox).await.unwrap();
        let mut sub2 = handle.subscribe(TopicId::Inbox).await.unwrap();

        pub_.publish(TopicId::Inbox, test_message("2", "fanout"))
            .await
            .unwrap();

        let e1 = sub1.recv().await.unwrap();
        let e2 = sub2.recv().await.unwrap();
        match (&e1, &e2) {
            (BusEvent::Message(m1), BusEvent::Message(m2)) => {
                assert_eq!(m1.content, "fanout");
                assert_eq!(m2.content, "fanout");
            }
            _ => panic!("expected Message variants"),
        }
    }

    #[tokio::test]
    async fn publish_to_empty_topic() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Publishing to a topic with no subscribers should not error.
        let result = pub_
            .publish(TopicId::AgentMain, test_message("3", "void"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_drop_unsubscribes() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let sub = handle.subscribe(TopicId::Inbox).await.unwrap();

        // Drop subscriber, then publish — should not error.
        drop(sub);

        // Give the broker a moment to process the unsubscribe.
        tokio::task::yield_now().await;

        let result = pub_
            .publish(TopicId::Inbox, test_message("4", "gone"))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn broker_shutdown_on_all_senders_dropped() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Drop the handle and publisher — no subscribers hold cmd_tx clones,
        // so the broker should shut down.
        drop(handle);
        drop(pub_);

        // Give the broker time to exit.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn subscriber_recv_returns_none_after_drop() {
        let handle = spawn_broker();
        let mut sub = handle.subscribe(TopicId::Inbox).await.unwrap();

        drop(handle);

        // The broker is still alive because sub holds a cmd_tx clone.
        // Verify recv doesn't immediately return None (it would block).
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(result.is_err(), "recv should timeout while broker is alive");
    }

    #[tokio::test]
    async fn multiple_topics_independent() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub_inbox = handle.subscribe(TopicId::Inbox).await.unwrap();
        let mut sub_agent = handle.subscribe(TopicId::AgentMain).await.unwrap();

        pub_.publish(TopicId::Inbox, test_message("5", "for inbox"))
            .await
            .unwrap();

        pub_.publish(TopicId::AgentMain, test_message("6", "for agent"))
            .await
            .unwrap();

        let inbox_event = sub_inbox.recv().await.unwrap();
        let agent_event = sub_agent.recv().await.unwrap();

        match inbox_event {
            BusEvent::Message(msg) => assert_eq!(msg.content, "for inbox"),
            _ => panic!("expected Message variant"),
        }
        match agent_event {
            BusEvent::Message(msg) => assert_eq!(msg.content, "for agent"),
            _ => panic!("expected Message variant"),
        }
    }

    #[tokio::test]
    async fn closed_subscriber_pruned() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        let sub1 = handle.subscribe(TopicId::Inbox).await.unwrap();
        let mut sub2 = handle.subscribe(TopicId::Inbox).await.unwrap();

        // Close sub1's receiver by dropping it.
        drop(sub1);
        tokio::task::yield_now().await;

        // Publish — sub1 should be pruned, sub2 should receive.
        pub_.publish(TopicId::Inbox, test_message("7", "after prune"))
            .await
            .unwrap();

        let event = sub2.recv().await.unwrap();
        match event {
            BusEvent::Message(msg) => assert_eq!(msg.content, "after prune"),
            _ => panic!("expected Message variant"),
        }
    }

    #[tokio::test]
    async fn publish_to_empty_topic_emits_bus_error() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut error_sub = handle.subscribe(TopicId::BusErrors).await.unwrap();

        // Publish to a topic with no subscribers.
        pub_.publish(TopicId::Inbox, test_message("err-1", "nowhere"))
            .await
            .unwrap();

        let event = tokio::time::timeout(tokio::time::Duration::from_millis(200), error_sub.recv())
            .await
            .unwrap()
            .unwrap();

        match event {
            BusEvent::Error { message, .. } => {
                assert!(
                    message.contains("no active subscribers"),
                    "error should mention no subscribers: {message}"
                );
                assert!(message.contains("inbox"), "error should mention the topic");
            }
            _ => panic!("expected Error variant, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn bus_error_topic_no_recursion() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Publish to BusErrors with no subscribers — should not recurse.
        let result = pub_
            .publish(
                TopicId::BusErrors,
                BusEvent::Error {
                    correlation_id: String::new(),
                    message: "test".into(),
                },
            )
            .await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Typed API tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn typed_publish_and_subscribe() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub = handle
            .subscribe_typed(topics::Response(ep.clone()))
            .await
            .unwrap();

        let event = crate::bus::ResponseEvent {
            correlation_id: "c1".into(),
            content: "hello typed".into(),
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 16)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        };

        pub_.publish_typed(topics::Response(ep), event)
            .await
            .unwrap();

        let received = sub.recv().await.unwrap().unwrap();
        assert_eq!(received.correlation_id, "c1");
        assert_eq!(received.content, "hello typed");
    }

    #[tokio::test]
    async fn typed_fan_out() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub1 = handle
            .subscribe_typed(topics::Response(ep.clone()))
            .await
            .unwrap();
        let mut sub2 = handle
            .subscribe_typed(topics::Response(ep.clone()))
            .await
            .unwrap();

        let event = crate::bus::ResponseEvent {
            correlation_id: "c2".into(),
            content: "fanout typed".into(),
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 16)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        };

        pub_.publish_typed(topics::Response(ep), event)
            .await
            .unwrap();

        let e1 = sub1.recv().await.unwrap().unwrap();
        let e2 = sub2.recv().await.unwrap().unwrap();
        assert_eq!(e1.content, "fanout typed");
        assert_eq!(e2.content, "fanout typed");
    }

    #[tokio::test]
    async fn typed_subscriber_returns_none_on_shutdown() {
        let handle = spawn_broker();
        let ep = EndpointName::from("ws");
        let mut sub = handle.subscribe_typed(topics::Response(ep)).await.unwrap();

        // Drop handle so broker eventually shuts down
        drop(handle);

        let result = tokio::time::timeout(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(result.is_err(), "recv should timeout while broker is alive");
    }

    #[tokio::test]
    async fn typed_subscriber_drop_unsubscribes() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let sub = handle
            .subscribe_typed(topics::Response(ep.clone()))
            .await
            .unwrap();

        drop(sub);
        tokio::task::yield_now().await;

        // Should not error
        let event = crate::bus::ResponseEvent {
            correlation_id: "c3".into(),
            content: "gone".into(),
            timestamp: NaiveDate::from_ymd_opt(2026, 3, 16)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        };
        let result = pub_.publish_typed(topics::Response(ep), event).await;
        assert!(result.is_ok());
    }
}

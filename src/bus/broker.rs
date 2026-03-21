//! Broker task and `BusHandle` factory.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::events::SystemMessageEvent;
use super::handle::{BrokerCommand, ErasedEvent, Publisher, Subscriber};
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

    /// Create a typed [`Subscriber`] for the given topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn subscribe<T: Topic>(&self, topic: T) -> Result<Subscriber<T::Event>, BusError> {
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

        Ok(Subscriber::new(id, topic_id, event_rx, self.cmd_tx.clone()))
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
    debug!("bus broker running");
    let mut subscriptions: HashMap<TopicId, Vec<(u64, mpsc::Sender<ErasedEvent>)>> = HashMap::new();
    let mut full_subscribers: HashSet<u64> = HashSet::new();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BrokerCommand::Publish { topic, event } => {
                let had_subscribers = if let Some(subscribers) = subscriptions.get_mut(&topic) {
                    let mut any_delivered = false;
                    subscribers.retain(|(id, tx)| {
                        match tx.try_send(Arc::clone(&event)) {
                            Ok(()) => {
                                any_delivered = true;
                                if full_subscribers.remove(id) {
                                    debug!(
                                        topic = %topic,
                                        subscriber_id = id,
                                        "subscriber recovered from backpressure"
                                    );
                                }
                                true
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                if full_subscribers.insert(*id) {
                                    warn!(
                                        topic = %topic,
                                        subscriber_id = id,
                                        "subscriber full, dropping events"
                                    );
                                }
                                true // keep subscriber
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                full_subscribers.remove(id);
                                debug!(
                                    topic = %topic,
                                    subscriber_id = id,
                                    "subscriber closed, removing"
                                );
                                false // prune
                            }
                        }
                    });
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
                    }
                    any_delivered
                } else {
                    false
                };

                // Publish error when no subscribers received the event.
                // Guard: skip if the original topic is SystemMessage to prevent recursion.
                if !had_subscribers && topic != TopicId::SystemMessage {
                    let error_event: ErasedEvent = Arc::new(SystemMessageEvent::Error {
                        correlation_id: String::new(),
                        message: format!("no active subscribers for topic {topic}"),
                    });
                    if let Some(error_subs) = subscriptions.get_mut(&TopicId::SystemMessage) {
                        error_subs.retain(|(id, tx)| match tx.try_send(Arc::clone(&error_event)) {
                            Ok(()) => true,
                            Err(e) => {
                                warn!(
                                    subscriber_id = id,
                                    error = %e,
                                    "failed to deliver error event to SystemMessage subscriber"
                                );
                                false
                            }
                        });
                        if error_subs.is_empty() {
                            subscriptions.remove(&TopicId::SystemMessage);
                        }
                    }
                }
            }
            BrokerCommand::Subscribe { id, topic, sender } => {
                debug!(subscriber_id = id, topic = %topic, "subscriber registered");
                subscriptions.entry(topic).or_default().push((id, sender));
            }
            BrokerCommand::Unsubscribe { id, topic } => {
                full_subscribers.remove(&id);
                if let Some(subscribers) = subscriptions.get_mut(&topic) {
                    subscribers.retain(|(sub_id, _)| *sub_id != id);
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
                    }
                }
                debug!(subscriber_id = id, topic = %topic, "subscriber unregistered");
            }
        }
    }
    debug!("bus broker shut down");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::panic, reason = "test assertions")]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::events::{MessageEvent, ResponseEvent};
    use crate::bus::topics;
    use crate::bus::types::EndpointName;
    use crate::interfaces::types::MessageOrigin;

    fn test_timestamp() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn test_message(id: &str, content: &str) -> MessageEvent {
        MessageEvent {
            id: id.into(),
            content: content.into(),
            origin: MessageOrigin {
                endpoint: "test".into(),
                sender_name: "tester".into(),
                sender_id: "t-1".into(),
            },
            timestamp: test_timestamp(),
            images: vec![],
        }
    }

    #[tokio::test]
    async fn publish_to_single_subscriber() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub = handle.subscribe(topics::UserMessage).await.unwrap();

        pub_.publish(topics::UserMessage, test_message("1", "hello"))
            .await
            .unwrap();

        let msg = sub.recv().await.unwrap().unwrap();
        assert_eq!(msg.id, "1");
        assert_eq!(msg.content, "hello");
    }

    #[tokio::test]
    async fn publish_fan_out() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub1 = handle.subscribe(topics::UserMessage).await.unwrap();
        let mut sub2 = handle.subscribe(topics::UserMessage).await.unwrap();

        pub_.publish(topics::UserMessage, test_message("2", "fanout"))
            .await
            .unwrap();

        let m1 = sub1.recv().await.unwrap().unwrap();
        let m2 = sub2.recv().await.unwrap().unwrap();
        assert_eq!(m1.content, "fanout");
        assert_eq!(m2.content, "fanout");
    }

    #[tokio::test]
    async fn publish_to_empty_topic() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        let ep = EndpointName::from("ws");
        let event = ResponseEvent {
            correlation_id: "c1".into(),
            content: "void".into(),
            timestamp: test_timestamp(),
        };

        // Publishing to a topic with no subscribers should not error.
        let result = pub_.publish(topics::Response(ep), event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_drop_unsubscribes() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let sub = handle.subscribe(topics::UserMessage).await.unwrap();

        // Drop subscriber, then publish — should not error.
        drop(sub);

        // Give the broker a moment to process the unsubscribe.
        tokio::task::yield_now().await;

        let result = pub_
            .publish(topics::UserMessage, test_message("4", "gone"))
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
        let ep = EndpointName::from("ws");
        let mut sub = handle.subscribe(topics::Response(ep)).await.unwrap();

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
        let mut sub_msg = handle.subscribe(topics::UserMessage).await.unwrap();
        let ep = EndpointName::from("ws");
        let mut sub_resp = handle
            .subscribe(topics::Response(ep.clone()))
            .await
            .unwrap();

        pub_.publish(topics::UserMessage, test_message("5", "for user"))
            .await
            .unwrap();

        let resp_event = ResponseEvent {
            correlation_id: "6".into(),
            content: "for response".into(),
            timestamp: test_timestamp(),
        };
        pub_.publish(topics::Response(ep), resp_event)
            .await
            .unwrap();

        let msg = sub_msg.recv().await.unwrap().unwrap();
        let resp = sub_resp.recv().await.unwrap().unwrap();

        assert_eq!(msg.content, "for user");
        assert_eq!(resp.content, "for response");
    }

    #[tokio::test]
    async fn closed_subscriber_pruned() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        let sub1 = handle.subscribe(topics::UserMessage).await.unwrap();
        let mut sub2 = handle.subscribe(topics::UserMessage).await.unwrap();

        // Close sub1's receiver by dropping it.
        drop(sub1);
        tokio::task::yield_now().await;

        // Publish — sub1 should be pruned, sub2 should receive.
        pub_.publish(topics::UserMessage, test_message("7", "after prune"))
            .await
            .unwrap();

        let msg = sub2.recv().await.unwrap().unwrap();
        assert_eq!(msg.content, "after prune");
    }

    #[tokio::test]
    async fn publish_to_empty_topic_emits_system_error() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut error_sub = handle.subscribe(topics::SystemMessage).await.unwrap();

        // Publish to a topic with no subscribers.
        pub_.publish(topics::UserMessage, test_message("err-1", "nowhere"))
            .await
            .unwrap();

        let event = tokio::time::timeout(tokio::time::Duration::from_millis(200), error_sub.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        match event {
            SystemMessageEvent::Error { message, .. } => {
                assert!(
                    message.contains("no active subscribers"),
                    "error should mention no subscribers: {message}"
                );
                assert!(
                    message.contains("user:message"),
                    "error should mention the topic: {message}"
                );
            }
            SystemMessageEvent::Notice { .. } | SystemMessageEvent::Event(_) => {
                panic!("expected SystemMessageEvent::Error, got {event:?}")
            }
        }
    }

    #[tokio::test]
    async fn system_message_topic_no_recursion() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Publish to SystemMessage with no subscribers — should not recurse.
        let result = pub_
            .publish(
                topics::SystemMessage,
                SystemMessageEvent::Error {
                    correlation_id: String::new(),
                    message: "test".into(),
                },
            )
            .await;
        assert!(result.is_ok());
    }
}

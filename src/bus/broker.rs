//! Broker task and `BusHandle` factory.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::handle::{BrokerCommand, ErasedEvent, Publisher, Subscriber};
use super::topics::{Carries, Topic};
use super::types::{BusError, TopicId};
use crate::util::spawn_monitored;

/// Command channel capacity for the broker.
const BROKER_COMMAND_CAPACITY: usize = 256;

/// Per-subscriber event channel capacity.
const SUBSCRIBER_CAPACITY: usize = 64;

/// Composite routing key: (topic, event type).
type RouteKey = (TopicId, TypeId);

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

    /// Create a typed [`Subscriber`] for the given topic and event type.
    ///
    /// The topic must implement `Carries<E>` for the desired event type,
    /// ensuring compile-time safety for the subscription.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn subscribe<T, E>(&self, topic: T) -> Result<Subscriber<E>, BusError>
    where
        T: Topic + Carries<E>,
        E: Clone + Send + Sync + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = mpsc::channel(SUBSCRIBER_CAPACITY);
        let topic_id = topic.topic_id();

        self.cmd_tx
            .send(BrokerCommand::Subscribe {
                id,
                topic: topic_id.clone(),
                event_type: TypeId::of::<E>(),
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
/// Routes events by `(TopicId, TypeId)` composite key to matching subscribers.
/// Exits naturally when every `BusHandle` (and derived sender) is dropped.
async fn run_broker(mut cmd_rx: mpsc::Receiver<BrokerCommand>) {
    debug!("bus broker running");
    let mut subscriptions: HashMap<RouteKey, Vec<(u64, mpsc::Sender<ErasedEvent>)>> =
        HashMap::new();
    let mut full_subscribers: HashSet<u64> = HashSet::new();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BrokerCommand::Publish {
                topic,
                event_type,
                event,
            } => {
                let key = (topic, event_type);
                if let Some(subscribers) = subscriptions.get_mut(&key) {
                    subscribers.retain(|(id, tx)| {
                        match tx.try_send(Arc::clone(&event)) {
                            Ok(()) => {
                                if full_subscribers.remove(id) {
                                    debug!(
                                        topic = %key.0,
                                        subscriber_id = id,
                                        "subscriber recovered from backpressure"
                                    );
                                }
                                true
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                if full_subscribers.insert(*id) {
                                    warn!(
                                        topic = %key.0,
                                        subscriber_id = id,
                                        "subscriber full, dropping events"
                                    );
                                }
                                true // keep subscriber
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                full_subscribers.remove(id);
                                debug!(
                                    topic = %key.0,
                                    subscriber_id = id,
                                    "subscriber closed, removing"
                                );
                                false // prune
                            }
                        }
                    });
                    if subscribers.is_empty() {
                        subscriptions.remove(&key);
                    }
                } else {
                    debug!(topic = %key.0, "no active subscribers for topic, event dropped");
                }
            }
            BrokerCommand::Subscribe {
                id,
                topic,
                event_type,
                sender,
            } => {
                debug!(subscriber_id = id, topic = %topic, "subscriber registered");
                subscriptions
                    .entry((topic, event_type))
                    .or_default()
                    .push((id, sender));
            }
            BrokerCommand::Unsubscribe {
                id,
                topic,
                event_type,
            } => {
                full_subscribers.remove(&id);
                let key = (topic, event_type);
                if let Some(subscribers) = subscriptions.get_mut(&key) {
                    subscribers.retain(|(sub_id, _)| *sub_id != id);
                    if subscribers.is_empty() {
                        subscriptions.remove(&key);
                    }
                }
                debug!(subscriber_id = id, topic = %key.0, "subscriber unregistered");
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
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::events::{MessageEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent};
    use crate::bus::topics;
    use crate::bus::types::{EndpointName, NotifyName, SYSTEM_CHANNEL};
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
        let result = pub_.publish(topics::Endpoint(ep), event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_drop_unsubscribes() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let sub: Subscriber<MessageEvent> = handle.subscribe(topics::UserMessage).await.unwrap();

        // Drop subscriber, then publish — should not error.
        drop(sub);

        let result = pub_
            .publish(topics::UserMessage, test_message("4", "gone"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_recv_returns_none_after_drop() {
        let handle = spawn_broker();
        let ep = EndpointName::from("ws");
        let mut sub: Subscriber<ResponseEvent> =
            handle.subscribe(topics::Endpoint(ep)).await.unwrap();

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
        let mut sub_msg: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        let ep = EndpointName::from("ws");
        let mut sub_resp: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
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
        pub_.publish(topics::Endpoint(ep), resp_event)
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

        let sub1: Subscriber<MessageEvent> = handle.subscribe(topics::UserMessage).await.unwrap();
        let mut sub2: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        // Close sub1's receiver by dropping it.
        drop(sub1);

        // Publish — sub1 is pruned when the broker sees its channel is closed, sub2 should receive.
        pub_.publish(topics::UserMessage, test_message("7", "after prune"))
            .await
            .unwrap();

        let msg = sub2.recv().await.unwrap().unwrap();
        assert_eq!(msg.content, "after prune");
    }

    /// Verify that different event types on the same topic are routed independently.
    #[tokio::test]
    async fn multi_event_routing_on_same_topic() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");

        let mut sub_resp: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        let mut sub_lifecycle: Subscriber<TurnLifecycleEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        // Publish a ResponseEvent — only sub_resp should receive it
        pub_.publish(
            topics::Endpoint(ep.clone()),
            ResponseEvent {
                correlation_id: "c1".into(),
                content: "hello".into(),
                timestamp: test_timestamp(),
            },
        )
        .await
        .unwrap();

        let resp = sub_resp.recv().await.unwrap().unwrap();
        assert_eq!(resp.content, "hello");

        // sub_lifecycle should NOT have received anything
        let timeout_result: Result<Result<Option<TurnLifecycleEvent>, _>, _> =
            tokio::time::timeout(tokio::time::Duration::from_millis(50), sub_lifecycle.recv())
                .await;
        assert!(
            timeout_result.is_err(),
            "lifecycle subscriber should not receive ResponseEvent"
        );
    }

    /// Verify that system notices on Notification("system") are routed correctly.
    #[tokio::test]
    async fn system_notice_routing() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        let mut sub: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();

        pub_.publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            NoticeEvent {
                message: "config reloaded".into(),
            },
        )
        .await
        .unwrap();

        let notice = sub.recv().await.unwrap().unwrap();
        assert_eq!(notice.message, "config reloaded");
    }

    #[tokio::test]
    async fn backpressure_drops_and_recovers() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub = handle.subscribe(topics::UserMessage).await.unwrap();
        // sync_sub confirms the broker has processed each publish before we proceed.
        let mut sync_sub = handle.subscribe(topics::UserMessage).await.unwrap();

        // Fill sub's channel to capacity.
        for i in 0..SUBSCRIBER_CAPACITY {
            pub_.publish(topics::UserMessage, test_message(&i.to_string(), "fill"))
                .await
                .unwrap();
        }
        for _ in 0..SUBSCRIBER_CAPACITY {
            sync_sub.recv().await.unwrap().unwrap();
        }

        // Sub's channel is full — overflow message should be dropped for sub.
        pub_.publish(topics::UserMessage, test_message("overflow", "dropped"))
            .await
            .unwrap();
        sync_sub.recv().await.unwrap().unwrap();

        // Drain all fill messages from sub.
        for _ in 0..SUBSCRIBER_CAPACITY {
            sub.recv().await.unwrap().unwrap();
        }

        // Overflow must not appear in sub's channel.
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(
            result.is_err(),
            "full subscriber should not receive overflow event"
        );

        // After recovery, subsequent publishes are received.
        pub_.publish(topics::UserMessage, test_message("after", "recovered"))
            .await
            .unwrap();
        let msg = sub.recv().await.unwrap().unwrap();
        assert_eq!(msg.id, "after");
    }

    #[tokio::test]
    async fn subscriber_recv_returns_none_when_broker_exits() {
        use std::any::TypeId;

        let handle = spawn_broker();
        let ep = EndpointName::from("ws");
        let topic_id = topics::Endpoint(ep).topic_id();

        // Manually register an event channel with the broker.
        let (event_tx, event_rx) = mpsc::channel::<ErasedEvent>(16);
        handle
            .cmd_tx
            .send(BrokerCommand::Subscribe {
                id: 99,
                topic: topic_id.clone(),
                event_type: TypeId::of::<ResponseEvent>(),
                sender: event_tx,
            })
            .await
            .unwrap();

        // Create subscriber with a disconnected cmd_tx so it does not keep the broker alive.
        let (dead_cmd_tx, dead_cmd_rx) = mpsc::channel::<BrokerCommand>(1);
        drop(dead_cmd_rx);
        let mut sub = Subscriber::<ResponseEvent>::new(99, topic_id, event_rx, dead_cmd_tx);

        // Drop the handle — no remaining cmd_tx senders; broker will exit.
        drop(handle);

        // Broker exits and drops subscriptions, closing event_tx; recv returns Ok(None).
        let result = sub.recv().await;
        assert!(matches!(result, Ok(None)));
    }
}

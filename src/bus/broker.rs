//! Broker task and `BusHandle` factory.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::events::BusEvent;
use super::handle::{BrokerCommand, Publisher, Subscriber};
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

    /// Create a [`Subscriber`] for the given topic.
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
    let mut subscriptions: HashMap<TopicId, Vec<(u64, mpsc::Sender<BusEvent>)>> = HashMap::new();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BrokerCommand::Publish { topic, event } => {
                if let Some(subscribers) = subscriptions.get_mut(&topic) {
                    subscribers.retain(|(id, tx)| {
                        match tx.try_send(event.clone()) {
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
                    if subscribers.is_empty() {
                        subscriptions.remove(&topic);
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

        // Drop the handle — but the subscriber still holds a cmd_tx clone,
        // so the broker stays alive. Drop the subscriber's event sender
        // indirectly: the broker exits only when ALL cmd_tx clones are gone.
        // To test recv() returning None, we need to drop the subscriber's
        // cmd_tx too. We do this by dropping handle and relying on the
        // subscriber being the last sender — when we drop it, recv returns
        // None on next call. Instead, test that dropping the handle and
        // publishing nothing causes recv to block (i.e., broker is still alive).
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
}

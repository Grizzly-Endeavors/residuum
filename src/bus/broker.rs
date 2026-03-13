//! Broker task and `BusHandle` factory.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::handle::{BrokerCommand, Publisher, Subscriber};
use super::types::{BusError, BusEvent, TopicId};
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
pub(crate) struct BusHandle {
    cmd_tx: mpsc::Sender<BrokerCommand>,
    next_id: Arc<AtomicU64>,
}

impl BusHandle {
    /// Create a [`Publisher`] that can send events to the bus.
    #[must_use]
    pub(crate) fn publisher(&self) -> Publisher {
        Publisher::new(self.cmd_tx.clone())
    }

    /// Create a [`Subscriber`] for the given topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub(crate) async fn subscribe(&self, topic: TopicId) -> Result<Subscriber, BusError> {
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
pub(crate) fn spawn_broker() -> BusHandle {
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
                subscriptions
                    .entry(topic)
                    .or_default()
                    .push((id, sender));
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_to_single_subscriber() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub = handle.subscribe(TopicId::Inbox).await.unwrap();

        pub_
            .publish(
                TopicId::Inbox,
                BusEvent::Message {
                    id: "1".into(),
                    content: "hello".into(),
                },
            )
            .await
            .unwrap();

        let event = sub.recv().await.unwrap();
        match event {
            BusEvent::Message { id, content } => {
                assert_eq!(id, "1");
                assert_eq!(content, "hello");
            }
        }
    }

    #[tokio::test]
    async fn publish_fan_out() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub1 = handle.subscribe(TopicId::Inbox).await.unwrap();
        let mut sub2 = handle.subscribe(TopicId::Inbox).await.unwrap();

        pub_
            .publish(
                TopicId::Inbox,
                BusEvent::Message {
                    id: "2".into(),
                    content: "fanout".into(),
                },
            )
            .await
            .unwrap();

        let e1 = sub1.recv().await.unwrap();
        let e2 = sub2.recv().await.unwrap();
        match (&e1, &e2) {
            (
                BusEvent::Message {
                    content: c1, ..
                },
                BusEvent::Message {
                    content: c2, ..
                },
            ) => {
                assert_eq!(c1, "fanout");
                assert_eq!(c2, "fanout");
            }
        }
    }

    #[tokio::test]
    async fn publish_to_empty_topic() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Publishing to a topic with no subscribers should not error.
        let result = pub_
            .publish(
                TopicId::AgentMain,
                BusEvent::Message {
                    id: "3".into(),
                    content: "void".into(),
                },
            )
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
            .publish(
                TopicId::Inbox,
                BusEvent::Message {
                    id: "4".into(),
                    content: "gone".into(),
                },
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn broker_shutdown_on_handle_drop() {
        let handle = spawn_broker();
        let mut sub = handle.subscribe(TopicId::Inbox).await.unwrap();

        // Drop all handles — broker should shut down.
        drop(handle);

        // Subscriber recv should return None.
        let result = sub.recv().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn multiple_topics_independent() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub_inbox = handle.subscribe(TopicId::Inbox).await.unwrap();
        let mut sub_agent = handle.subscribe(TopicId::AgentMain).await.unwrap();

        pub_
            .publish(
                TopicId::Inbox,
                BusEvent::Message {
                    id: "5".into(),
                    content: "for inbox".into(),
                },
            )
            .await
            .unwrap();

        pub_
            .publish(
                TopicId::AgentMain,
                BusEvent::Message {
                    id: "6".into(),
                    content: "for agent".into(),
                },
            )
            .await
            .unwrap();

        let inbox_event = sub_inbox.recv().await.unwrap();
        let agent_event = sub_agent.recv().await.unwrap();

        match inbox_event {
            BusEvent::Message { content, .. } => assert_eq!(content, "for inbox"),
        }
        match agent_event {
            BusEvent::Message { content, .. } => assert_eq!(content, "for agent"),
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
        pub_
            .publish(
                TopicId::Inbox,
                BusEvent::Message {
                    id: "7".into(),
                    content: "after prune".into(),
                },
            )
            .await
            .unwrap();

        let event = sub2.recv().await.unwrap();
        match event {
            BusEvent::Message { content, .. } => assert_eq!(content, "after prune"),
        }
    }
}

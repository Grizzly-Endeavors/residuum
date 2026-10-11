//! Publisher and subscriber handles for the bus.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::error;

use super::topics::{Carries, Topic};
use super::types::{BusError, TopicId};

// ---------------------------------------------------------------------------
// Type-erased event wrapper
// ---------------------------------------------------------------------------

/// Type-erased event stored in the broker.
pub(super) type ErasedEvent = Arc<dyn Any + Send + Sync>;

// ---------------------------------------------------------------------------
// EventSender / EventReceiver
// ---------------------------------------------------------------------------

/// The broker's send half of one subscriber's channel, shaped by that
/// route's [`super::topics::DeliveryMode`].
pub(super) enum EventSender {
    /// Backs [`super::topics::DeliveryMode::Lossless`]: unbounded, so a send
    /// only ever fails when the subscriber is gone. `tokio::sync::mpsc`'s
    /// unbounded sender has no queue-depth query of its own, so the shared
    /// counter tracks it — incremented here on every send, decremented by
    /// [`EventReceiver::recv`] on the other end.
    Lossless(mpsc::UnboundedSender<ErasedEvent>, Arc<AtomicUsize>),
    /// Backs [`super::topics::DeliveryMode::Lossy`]: bounded, so a full
    /// channel is reported instead of blocking the broker.
    Lossy(mpsc::Sender<ErasedEvent>),
}

/// The outcome of one [`EventSender::send`] attempt.
pub(super) enum SendOutcome {
    /// The event was queued for the subscriber.
    Sent,
    /// The channel was full; the event was not queued. Only possible on a
    /// [`EventSender::Lossy`] route.
    Dropped,
    /// The subscriber's receiver is gone.
    Closed,
}

impl EventSender {
    /// Attempt to deliver `event`, per this route's delivery mode.
    pub(super) fn send(&self, event: ErasedEvent) -> SendOutcome {
        match self {
            Self::Lossless(tx, backlog) => match tx.send(event) {
                Ok(()) => {
                    backlog.fetch_add(1, Ordering::Relaxed);
                    SendOutcome::Sent
                }
                Err(_closed) => SendOutcome::Closed,
            },
            Self::Lossy(tx) => match tx.try_send(event) {
                Ok(()) => SendOutcome::Sent,
                Err(mpsc::error::TrySendError::Full(_)) => SendOutcome::Dropped,
                Err(mpsc::error::TrySendError::Closed(_)) => SendOutcome::Closed,
            },
        }
    }

    /// Current queue depth, for the stuck-subscriber backlog check on a
    /// lossless route (see `broker::LOSSLESS_BACKLOG_WARN_THRESHOLD`). A
    /// lossy route's bounded capacity keeps this well under that threshold
    /// by construction, so the check is a harmless no-op there.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Lossless(_, backlog) => backlog.load(Ordering::Relaxed),
            Self::Lossy(tx) => tx.max_capacity() - tx.capacity(),
        }
    }
}

/// The subscriber's receive half, mirroring [`EventSender`].
enum EventReceiver {
    Lossless(mpsc::UnboundedReceiver<ErasedEvent>, Arc<AtomicUsize>),
    Lossy(mpsc::Receiver<ErasedEvent>),
}

impl EventReceiver {
    async fn recv(&mut self) -> Option<ErasedEvent> {
        match self {
            Self::Lossless(rx, backlog) => {
                let event = rx.recv().await;
                if event.is_some() {
                    backlog.fetch_sub(1, Ordering::Relaxed);
                }
                event
            }
            Self::Lossy(rx) => rx.recv().await,
        }
    }

    /// Take one already-queued event without waiting for more, or `None` if
    /// the channel is empty right now.
    fn try_recv(&mut self) -> Option<ErasedEvent> {
        match self {
            Self::Lossless(rx, backlog) => {
                let event = rx.try_recv().ok();
                if event.is_some() {
                    backlog.fetch_sub(1, Ordering::Relaxed);
                }
                event
            }
            Self::Lossy(rx) => rx.try_recv().ok(),
        }
    }
}

// ---------------------------------------------------------------------------
// BrokerCommand
// ---------------------------------------------------------------------------

/// Commands sent from handles to the broker task.
pub enum BrokerCommand {
    /// Publish a type-erased event to a (topic, `event_type`) pair.
    Publish {
        topic: TopicId,
        event_type: TypeId,
        event: ErasedEvent,
    },
    /// Register a subscriber for a (topic, `event_type`) pair.
    Subscribe {
        id: u64,
        topic: TopicId,
        event_type: TypeId,
        sender: EventSender,
    },
    /// Remove a subscriber from a (topic, `event_type`) pair.
    Unsubscribe {
        id: u64,
        topic: TopicId,
        event_type: TypeId,
    },
}

// ---------------------------------------------------------------------------
// Publisher
// ---------------------------------------------------------------------------

/// A cloneable handle for publishing events to the bus.
#[derive(Clone)]
pub struct Publisher {
    cmd_tx: Option<mpsc::Sender<BrokerCommand>>,
}

impl Publisher {
    /// Create a new publisher from a command channel sender.
    pub(super) fn new(cmd_tx: mpsc::Sender<BrokerCommand>) -> Self {
        Self {
            cmd_tx: Some(cmd_tx),
        }
    }

    /// Create a publisher not backed by any broker.
    ///
    /// Publish calls return [`BusError::BrokerShutdown`]. Use in contexts
    /// where nothing observes the published events (e.g. tests exercising a
    /// turn rather than the events it publishes).
    #[must_use]
    pub fn noop() -> Self {
        Self { cmd_tx: None }
    }

    /// Publish a typed event to a topic that carries it.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn publish<T, E>(&self, topic: T, event: E) -> Result<(), BusError>
    where
        T: Topic + Carries<E>,
        E: Clone + Send + Sync + 'static,
    {
        let Some(cmd_tx) = &self.cmd_tx else {
            return Err(BusError::BrokerShutdown);
        };
        let erased: ErasedEvent = Arc::new(event);
        cmd_tx
            .send(BrokerCommand::Publish {
                topic: topic.topic_id(),
                event_type: TypeId::of::<E>(),
                event: erased,
            })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)
    }
}

// ---------------------------------------------------------------------------
// Subscriber (typed, receives E directly)
// ---------------------------------------------------------------------------

/// A single-consumer handle for receiving typed events from a bus topic.
pub struct Subscriber<E: 'static> {
    id: u64,
    topic: TopicId,
    event_rx: EventReceiver,
    cmd_tx: mpsc::Sender<BrokerCommand>,
    _phantom: PhantomData<E>,
}

impl<E: Clone + Send + Sync + 'static> Subscriber<E> {
    /// Create a new typed subscriber backed by an unbounded (lossless)
    /// channel. `backlog` must be the same counter given to the paired
    /// [`EventSender::Lossless`].
    pub(super) fn new_lossless(
        id: u64,
        topic: TopicId,
        event_rx: mpsc::UnboundedReceiver<ErasedEvent>,
        backlog: Arc<AtomicUsize>,
        cmd_tx: mpsc::Sender<BrokerCommand>,
    ) -> Self {
        Self {
            id,
            topic,
            event_rx: EventReceiver::Lossless(event_rx, backlog),
            cmd_tx,
            _phantom: PhantomData,
        }
    }

    /// Create a new typed subscriber backed by a bounded (lossy) channel.
    pub(super) fn new_lossy(
        id: u64,
        topic: TopicId,
        event_rx: mpsc::Receiver<ErasedEvent>,
        cmd_tx: mpsc::Sender<BrokerCommand>,
    ) -> Self {
        Self {
            id,
            topic,
            event_rx: EventReceiver::Lossy(event_rx),
            cmd_tx,
            _phantom: PhantomData,
        }
    }

    /// Receive the next typed event, or `None` if the broker has shut down.
    ///
    /// # Errors
    ///
    /// Returns `BusError::TypeMismatch` if the event cannot be downcast to `E`.
    pub async fn recv(&mut self) -> Result<Option<E>, BusError> {
        let Some(erased) = self.event_rx.recv().await else {
            return Ok(None);
        };
        // Try to unwrap the Arc (only owner) or clone via downcast
        if let Ok(arc_e) = erased.downcast::<E>() {
            Ok(Some(Arc::unwrap_or_clone(arc_e)))
        } else {
            error!(
                expected = std::any::type_name::<E>(),
                topic = %self.topic,
                "type mismatch on bus receive: programmer error"
            );
            Err(BusError::TypeMismatch {
                expected: std::any::type_name::<E>(),
                topic: self.topic.to_string(),
            })
        }
    }

    /// Non-blocking drain of every event already queued for this subscriber.
    ///
    /// Used at shutdown, before this subscriber is dropped, so events
    /// published while a sender was told delivery succeeded are not silently
    /// discarded along with the channel. A type mismatch is logged and the
    /// event skipped, mirroring [`Subscriber::recv`]'s handling.
    pub fn drain(&mut self) -> Vec<E> {
        let mut events = Vec::new();
        while let Some(erased) = self.event_rx.try_recv() {
            match erased.downcast::<E>() {
                Ok(arc_e) => events.push(Arc::unwrap_or_clone(arc_e)),
                Err(_mismatched) => error!(
                    expected = std::any::type_name::<E>(),
                    topic = %self.topic,
                    "type mismatch draining bus subscriber: programmer error"
                ),
            }
        }
        events
    }
}

impl<E: 'static> Drop for Subscriber<E> {
    fn drop(&mut self) {
        drop(self.cmd_tx.try_send(BrokerCommand::Unsubscribe {
            id: self.id,
            topic: self.topic.clone(),
            event_type: TypeId::of::<E>(),
        }));
    }
}

// ---------------------------------------------------------------------------
// Compile-time trait assertions
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{EndpointName, IntermediateEvent, MessageEvent, spawn_broker, topics};
    use crate::interfaces::types::MessageOrigin;

    fn test_message(id: &str, content: &str) -> MessageEvent {
        MessageEvent {
            id: id.to_string(),
            content: content.to_string(),
            origin: MessageOrigin {
                endpoint: "test".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            context: None,
        }
    }

    fn _assert_publisher_traits()
    where
        Publisher: Clone + Send + Sync,
    {
    }

    fn _assert_subscriber_traits()
    where
        Subscriber<String>: Send,
    {
    }

    #[tokio::test]
    async fn noop_publisher_returns_broker_shutdown() {
        use crate::bus::types::BusError;

        let publisher = Publisher::noop();
        let result = publisher
            .publish(
                topics::Endpoint(EndpointName::from("test")),
                IntermediateEvent {
                    correlation_id: String::new(),
                    content: "hello".into(),
                },
            )
            .await;

        assert!(
            matches!(result, Err(BusError::BrokerShutdown)),
            "noop publisher should return BrokerShutdown"
        );
    }

    #[tokio::test]
    async fn drain_is_empty_with_nothing_queued() {
        let handle = spawn_broker();
        let mut sub: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        assert!(sub.drain().is_empty());
    }

    #[tokio::test]
    async fn drain_returns_queued_events_in_order_then_empties() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        for i in 0..3 {
            pub_.publish(
                topics::UserMessage,
                test_message(&i.to_string(), &format!("msg-{i}")),
            )
            .await
            .unwrap();
        }
        // The broker handles commands one at a time in order, so once this
        // second subscriber has seen the marker published just after the
        // three events above, the broker has already finished offering all
        // three to `sub` as well.
        let mut barrier: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();
        pub_.publish(topics::UserMessage, test_message("marker", "marker"))
            .await
            .unwrap();
        loop {
            let seen = barrier.recv().await.unwrap().unwrap();
            if seen.id == "marker" {
                break;
            }
        }

        let drained = sub.drain();
        let ids: Vec<_> = drained.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["0", "1", "2", "marker"],
            "events must come out in publish order"
        );

        assert!(
            sub.drain().is_empty(),
            "a second drain after everything was taken must return nothing"
        );
    }
}

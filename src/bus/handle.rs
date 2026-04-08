//! Publisher and subscriber handles for the bus.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;

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
        sender: mpsc::Sender<ErasedEvent>,
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
    cmd_tx: mpsc::Sender<BrokerCommand>,
}

impl Publisher {
    /// Create a new publisher from a command channel sender.
    pub(super) fn new(cmd_tx: mpsc::Sender<BrokerCommand>) -> Self {
        Self { cmd_tx }
    }

    /// Create a publisher not backed by any broker.
    ///
    /// Publish calls return [`BusError::BrokerShutdown`]. Use in contexts
    /// where event publishing is disabled (e.g., background sub-agent turns
    /// with no output endpoints).
    #[must_use]
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        // Dropping _rx closes the channel; any send returns BrokerShutdown.
        Self { cmd_tx: tx }
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
        let erased: ErasedEvent = Arc::new(event);
        self.cmd_tx
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
    event_rx: mpsc::Receiver<ErasedEvent>,
    cmd_tx: mpsc::Sender<BrokerCommand>,
    _phantom: PhantomData<E>,
}

impl<E: Clone + Send + Sync + 'static> Subscriber<E> {
    /// Create a new typed subscriber.
    pub(super) fn new(
        id: u64,
        topic: TopicId,
        event_rx: mpsc::Receiver<ErasedEvent>,
        cmd_tx: mpsc::Sender<BrokerCommand>,
    ) -> Self {
        Self {
            id,
            topic,
            event_rx,
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
    use crate::bus::{EndpointName, IntermediateEvent, topics};

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
}

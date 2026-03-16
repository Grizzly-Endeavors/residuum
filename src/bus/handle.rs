//! Publisher and subscriber handles for the bus.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::events::BusEvent;
use super::topics::Topic;
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
    /// Publish a type-erased event to a topic.
    Publish { topic: TopicId, event: ErasedEvent },
    /// Register a subscriber for a topic.
    Subscribe {
        id: u64,
        topic: TopicId,
        sender: mpsc::Sender<ErasedEvent>,
    },
    /// Remove a subscriber from a topic.
    Unsubscribe { id: u64, topic: TopicId },
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

    /// Publish a `BusEvent` to the given topic (legacy untyped API).
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn publish(&self, topic: TopicId, event: BusEvent) -> Result<(), BusError> {
        let erased: ErasedEvent = Arc::new(event);
        self.cmd_tx
            .send(BrokerCommand::Publish {
                topic,
                event: erased,
            })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)
    }

    /// Publish a typed event to a typed topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn publish_typed<T: Topic>(&self, topic: T, event: T::Event) -> Result<(), BusError> {
        let erased: ErasedEvent = Arc::new(event);
        self.cmd_tx
            .send(BrokerCommand::Publish {
                topic: topic.topic_id(),
                event: erased,
            })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)
    }
}

// ---------------------------------------------------------------------------
// Subscriber (legacy, receives BusEvent)
// ---------------------------------------------------------------------------

/// A single-consumer handle for receiving `BusEvent`s from a bus topic (legacy API).
pub struct Subscriber {
    id: u64,
    topic: TopicId,
    event_rx: mpsc::Receiver<ErasedEvent>,
    cmd_tx: mpsc::Sender<BrokerCommand>,
}

impl Subscriber {
    /// Create a new subscriber.
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
        }
    }

    /// Receive the next event, or `None` if the broker has shut down.
    ///
    /// Downcasts from the type-erased storage back to `BusEvent`. Events
    /// that are not `BusEvent` (i.e. published via the typed API) are
    /// silently skipped.
    pub async fn recv(&mut self) -> Option<BusEvent> {
        loop {
            let erased = self.event_rx.recv().await?;
            if let Some(bus_event) = erased.downcast_ref::<BusEvent>() {
                return Some(bus_event.clone());
            }
            // Not a BusEvent (published via typed API) — skip
        }
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        // Best-effort unsubscribe — if the broker is already gone, this is a no-op.
        drop(self.cmd_tx.try_send(BrokerCommand::Unsubscribe {
            id: self.id,
            topic: self.topic.clone(),
        }));
    }
}

// ---------------------------------------------------------------------------
// TypedSubscriber (new, receives T::Event directly)
// ---------------------------------------------------------------------------

/// A single-consumer handle for receiving typed events from a bus topic.
pub struct TypedSubscriber<E> {
    id: u64,
    topic: TopicId,
    event_rx: mpsc::Receiver<ErasedEvent>,
    cmd_tx: mpsc::Sender<BrokerCommand>,
    _phantom: PhantomData<E>,
}

impl<E: Clone + Send + Sync + 'static> TypedSubscriber<E> {
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
        match erased.downcast::<E>() {
            Ok(arc_e) => Ok(Some(Arc::unwrap_or_clone(arc_e))),
            Err(_) => Err(BusError::TypeMismatch {
                expected: std::any::type_name::<E>(),
                topic: self.topic.to_string(),
            }),
        }
    }
}

impl<E> Drop for TypedSubscriber<E> {
    fn drop(&mut self) {
        drop(self.cmd_tx.try_send(BrokerCommand::Unsubscribe {
            id: self.id,
            topic: self.topic.clone(),
        }));
    }
}

// ---------------------------------------------------------------------------
// Compile-time trait assertions
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_publisher_traits()
    where
        Publisher: Clone + Send + Sync,
    {
    }

    fn _assert_subscriber_traits()
    where
        Subscriber: Send,
    {
    }

    fn _assert_typed_subscriber_traits()
    where
        TypedSubscriber<String>: Send,
    {
    }
}

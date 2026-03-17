//! Publisher and subscriber handles for the bus.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use tokio::sync::mpsc;

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

    /// Publish a typed event to a typed topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub async fn publish<T: Topic>(&self, topic: T, event: T::Event) -> Result<(), BusError> {
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
// Subscriber (typed, receives T::Event directly)
// ---------------------------------------------------------------------------

/// A single-consumer handle for receiving typed events from a bus topic.
pub struct Subscriber<E> {
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
        match erased.downcast::<E>() {
            Ok(arc_e) => Ok(Some(Arc::unwrap_or_clone(arc_e))),
            Err(_) => Err(BusError::TypeMismatch {
                expected: std::any::type_name::<E>(),
                topic: self.topic.to_string(),
            }),
        }
    }
}

impl<E> Drop for Subscriber<E> {
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
        Subscriber<String>: Send,
    {
    }
}

//! Publisher and subscriber handles for the bus.

use tokio::sync::mpsc;

use super::events::BusEvent;
use super::types::{BusError, TopicId};

// ---------------------------------------------------------------------------
// BrokerCommand
// ---------------------------------------------------------------------------

/// Commands sent from handles to the broker task.
pub(crate) enum BrokerCommand {
    /// Publish an event to a topic.
    Publish { topic: TopicId, event: BusEvent },
    /// Register a subscriber for a topic.
    Subscribe {
        id: u64,
        topic: TopicId,
        sender: mpsc::Sender<BusEvent>,
    },
    /// Remove a subscriber from a topic.
    Unsubscribe { id: u64, topic: TopicId },
}

// ---------------------------------------------------------------------------
// Publisher
// ---------------------------------------------------------------------------

/// A cloneable handle for publishing events to the bus.
#[derive(Clone)]
pub(crate) struct Publisher {
    cmd_tx: mpsc::Sender<BrokerCommand>,
}

impl Publisher {
    /// Create a new publisher from a command channel sender.
    pub(super) fn new(cmd_tx: mpsc::Sender<BrokerCommand>) -> Self {
        Self { cmd_tx }
    }

    /// Publish an event to the given topic.
    ///
    /// # Errors
    ///
    /// Returns `BusError::BrokerShutdown` if the broker has stopped.
    pub(crate) async fn publish(&self, topic: TopicId, event: BusEvent) -> Result<(), BusError> {
        self.cmd_tx
            .send(BrokerCommand::Publish { topic, event })
            .await
            .map_err(|_closed| BusError::BrokerShutdown)
    }
}

// ---------------------------------------------------------------------------
// Subscriber
// ---------------------------------------------------------------------------

/// A single-consumer handle for receiving events from a bus topic.
pub(crate) struct Subscriber {
    id: u64,
    topic: TopicId,
    event_rx: mpsc::Receiver<BusEvent>,
    cmd_tx: mpsc::Sender<BrokerCommand>,
}

impl Subscriber {
    /// Create a new subscriber.
    pub(super) fn new(
        id: u64,
        topic: TopicId,
        event_rx: mpsc::Receiver<BusEvent>,
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
    pub(crate) async fn recv(&mut self) -> Option<BusEvent> {
        self.event_rx.recv().await
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
}

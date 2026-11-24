use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::error;

use super::events::NoticeEvent;
use super::topics::{self, Carries, Topic};
use super::types::{BusError, NotifyName, SYSTEM_CHANNEL, TopicId};
use residuum_inference::NoticeSink;

pub(super) type ErasedEvent = Arc<dyn Any + Send + Sync>;

pub(super) enum EventSender {
    Lossless(mpsc::UnboundedSender<ErasedEvent>, Arc<AtomicUsize>),
    Lossy(mpsc::Sender<ErasedEvent>),
}

pub(super) enum SendOutcome {
    Sent,
    Dropped,
    Closed,
}

impl EventSender {
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

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Lossless(_, backlog) => backlog.load(Ordering::Relaxed),
            Self::Lossy(tx) => tx.max_capacity() - tx.capacity(),
        }
    }
}

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

pub enum BrokerCommand {
    Publish {
        topic: TopicId,
        event_type: TypeId,
        event: ErasedEvent,
    },
    Subscribe {
        id: u64,
        topic: TopicId,
        event_type: TypeId,
        sender: EventSender,
    },
    Unsubscribe {
        id: u64,
        topic: TopicId,
        event_type: TypeId,
    },
}

#[derive(Clone)]
pub struct Publisher {
    cmd_tx: Option<mpsc::Sender<BrokerCommand>>,
}

impl Publisher {
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

#[async_trait]
impl NoticeSink for Publisher {
    async fn notice(&self, message: String) {
        if let Err(e) = self
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                NoticeEvent::new(message),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to publish failover notice");
        }
    }
}

pub struct Subscriber<E: 'static> {
    id: u64,
    topic: TopicId,
    event_rx: EventReceiver,
    cmd_tx: mpsc::Sender<BrokerCommand>,
    _phantom: PhantomData<E>,
}

impl<E: Clone + Send + Sync + 'static> Subscriber<E> {
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

    pub async fn recv(&mut self) -> Result<Option<E>, BusError> {
        let Some(erased) = self.event_rx.recv().await else {
            return Ok(None);
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::MessageOrigin;
    use crate::{EndpointName, IntermediateEvent, MessageEvent, spawn_broker, topics};

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
            attachments: Vec::new(),
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
        use crate::types::BusError;

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

    #[tokio::test]
    async fn notice_sink_delivers_the_notice_on_the_system_channel() {
        let handle = spawn_broker();
        let mut notices: Subscriber<NoticeEvent> = handle
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let sink: Arc<dyn NoticeSink> = Arc::new(handle.publisher());

        sink.notice("the main model is unavailable".to_string())
            .await;

        let event = crate::testing::next_event("a notice", &mut notices).await;
        assert_eq!(event.message, "the main model is unavailable");
    }
}

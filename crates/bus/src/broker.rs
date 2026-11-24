use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::handle::{BrokerCommand, EventSender, Publisher, SendOutcome, Subscriber};
use super::topics::{Carries, DeliveryMode, Topic};
use super::types::{BusError, TopicId};
use residuum_core::util::spawn_monitored;

const BROKER_COMMAND_CAPACITY: usize = 256;

const LOSSY_SUBSCRIBER_CAPACITY: usize = 1024;

const LOSSLESS_BACKLOG_WARN_THRESHOLD: usize = 10_000;

type RouteKey = (TopicId, TypeId);

#[derive(Clone)]
pub struct BusHandle {
    cmd_tx: mpsc::Sender<BrokerCommand>,
    next_id: Arc<AtomicU64>,
}

impl BusHandle {
    #[must_use]
    pub fn publisher(&self) -> Publisher {
        Publisher::new(self.cmd_tx.clone())
    }

    #[tracing::instrument(skip_all, fields(topic = %topic.topic_id()))]
    pub async fn subscribe<T, E>(&self, topic: T) -> Result<Subscriber<E>, BusError>
    where
        T: Topic + Carries<E>,
        E: Clone + Send + Sync + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let topic_id = topic.topic_id();

        let subscriber = match T::DELIVERY_MODE {
            DeliveryMode::Lossless => {
                let (event_tx, event_rx) = mpsc::unbounded_channel();
                let backlog = Arc::new(AtomicUsize::new(0));
                self.cmd_tx
                    .send(BrokerCommand::Subscribe {
                        id,
                        topic: topic_id.clone(),
                        event_type: TypeId::of::<E>(),
                        sender: EventSender::Lossless(event_tx, Arc::clone(&backlog)),
                    })
                    .await
                    .map_err(|_closed| BusError::BrokerShutdown)?;
                Subscriber::new_lossless(id, topic_id, event_rx, backlog, self.cmd_tx.clone())
            }
            DeliveryMode::Lossy => {
                let (event_tx, event_rx) = mpsc::channel(LOSSY_SUBSCRIBER_CAPACITY);
                self.cmd_tx
                    .send(BrokerCommand::Subscribe {
                        id,
                        topic: topic_id.clone(),
                        event_type: TypeId::of::<E>(),
                        sender: EventSender::Lossy(event_tx),
                    })
                    .await
                    .map_err(|_closed| BusError::BrokerShutdown)?;
                Subscriber::new_lossy(id, topic_id, event_rx, self.cmd_tx.clone())
            }
        };

        Ok(subscriber)
    }
}

#[must_use]
pub fn spawn_broker() -> BusHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(BROKER_COMMAND_CAPACITY);

    spawn_monitored("bus-broker", run_broker(cmd_rx));

    BusHandle {
        cmd_tx,
        next_id: Arc::new(AtomicU64::new(0)),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DropTransition {
    Started,
    Stopped { count: u64 },
}

fn record_drop_outcome(
    dropping: &mut HashMap<u64, u64>,
    id: u64,
    outcome: &SendOutcome,
) -> Option<DropTransition> {
    match outcome {
        SendOutcome::Sent => dropping
            .remove(&id)
            .map(|count| DropTransition::Stopped { count }),
        SendOutcome::Dropped => {
            let count = dropping.entry(id).or_insert(0);
            *count += 1;
            (*count == 1).then_some(DropTransition::Started)
        }
        SendOutcome::Closed => {
            dropping.remove(&id);
            None
        }
    }
}

fn record_backlog_len(large_backlogs: &mut HashSet<u64>, id: u64, len: usize) -> bool {
    if len >= LOSSLESS_BACKLOG_WARN_THRESHOLD {
        large_backlogs.insert(id)
    } else {
        large_backlogs.remove(&id);
        false
    }
}

#[tracing::instrument(skip_all)]
async fn run_broker(mut cmd_rx: mpsc::Receiver<BrokerCommand>) {
    debug!("bus broker running");
    let mut subscriptions: HashMap<RouteKey, Vec<(u64, EventSender)>> = HashMap::new();
    let mut dropping: HashMap<u64, u64> = HashMap::new();
    let mut large_backlogs: HashSet<u64> = HashSet::new();

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
                        let outcome = tx.send(Arc::clone(&event));
                        match record_drop_outcome(&mut dropping, *id, &outcome) {
                            Some(DropTransition::Started) => warn!(
                                topic = %key.0,
                                subscriber_id = id,
                                "lossy subscriber full, dropping events"
                            ),
                            Some(DropTransition::Stopped { count }) => info!(
                                topic = %key.0,
                                subscriber_id = id,
                                dropped = count,
                                "lossy subscriber recovered from backpressure"
                            ),
                            None => {}
                        }
                        match outcome {
                            SendOutcome::Sent => {
                                if record_backlog_len(&mut large_backlogs, *id, tx.len()) {
                                    warn!(
                                        topic = %key.0,
                                        subscriber_id = id,
                                        backlog = tx.len(),
                                        "lossless subscriber backlog is large; consumer may be stuck"
                                    );
                                }
                                true
                            }
                            SendOutcome::Dropped => true, // keep subscriber
                            SendOutcome::Closed => {
                                large_backlogs.remove(id);
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
                dropping.remove(&id);
                large_backlogs.remove(&id);
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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::events::{
        MessageEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent, WorkbenchEvent,
    };
    use crate::handle::ErasedEvent;
    use crate::origin::MessageOrigin;
    use crate::topics;
    use crate::types::{EndpointName, NotifyName, SYSTEM_CHANNEL};
    use residuum_core::testing::clock;

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
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: test_timestamp(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    fn test_artifact_update(name: &str) -> WorkbenchEvent {
        WorkbenchEvent::Updated { name: name.into() }
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
            attachment: None,
            conversation: None,
        };

        let result = pub_.publish(topics::Endpoint(ep), event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscriber_drop_unsubscribes() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let sub: Subscriber<MessageEvent> = handle.subscribe(topics::UserMessage).await.unwrap();

        drop(sub);

        let result = pub_
            .publish(topics::UserMessage, test_message("4", "gone"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn subscriber_recv_returns_none_after_drop() {
        let handle = spawn_broker();
        let ep = EndpointName::from("ws");
        let mut sub: Subscriber<ResponseEvent> =
            handle.subscribe(topics::Endpoint(ep)).await.unwrap();

        drop(handle);

        let result = clock::within(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(result.is_none(), "recv should wait while broker is alive");
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
            attachment: None,
            conversation: None,
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

        drop(sub1);

        pub_.publish(topics::UserMessage, test_message("7", "after prune"))
            .await
            .unwrap();

        let msg = sub2.recv().await.unwrap().unwrap();
        assert_eq!(msg.content, "after prune");
    }

    #[tokio::test(start_paused = true)]
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

        pub_.publish(
            topics::Endpoint(ep.clone()),
            ResponseEvent {
                correlation_id: "c1".into(),
                content: "hello".into(),
                timestamp: test_timestamp(),
                attachment: None,
                conversation: None,
            },
        )
        .await
        .unwrap();

        let resp = sub_resp.recv().await.unwrap().unwrap();
        assert_eq!(resp.content, "hello");

        let timeout_result: Option<Result<Option<TurnLifecycleEvent>, _>> =
            clock::within(tokio::time::Duration::from_millis(50), sub_lifecycle.recv()).await;
        assert!(
            timeout_result.is_none(),
            "lifecycle subscriber should not receive ResponseEvent"
        );
    }

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
            NoticeEvent::new("config reloaded"),
        )
        .await
        .unwrap();

        let notice = sub.recv().await.unwrap().unwrap();
        assert_eq!(notice.message, "config reloaded");
    }

    #[tokio::test(start_paused = true)]
    async fn backpressure_drops_and_recovers() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub: Subscriber<WorkbenchEvent> =
            handle.subscribe(topics::Workbench).await.unwrap();
        let mut sync_sub: Subscriber<WorkbenchEvent> =
            handle.subscribe(topics::Workbench).await.unwrap();

        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            pub_.publish(topics::Workbench, test_artifact_update("fill"))
                .await
                .unwrap();
        }
        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            sync_sub.recv().await.unwrap().unwrap();
        }

        pub_.publish(topics::Workbench, test_artifact_update("overflow"))
            .await
            .unwrap();
        sync_sub.recv().await.unwrap().unwrap();

        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            sub.recv().await.unwrap().unwrap();
        }

        let result = clock::within(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(
            result.is_none(),
            "full subscriber should not receive overflow event"
        );

        pub_.publish(topics::Workbench, test_artifact_update("after"))
            .await
            .unwrap();
        let msg = sub.recv().await.unwrap().unwrap();
        assert_eq!(msg, test_artifact_update("after"));
    }

    #[tokio::test]
    async fn lossless_subscriber_receives_every_event_far_beyond_lossy_capacity() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let mut sub = handle.subscribe(topics::UserMessage).await.unwrap();

        let total = LOSSY_SUBSCRIBER_CAPACITY * 4;
        for i in 0..total {
            pub_.publish(topics::UserMessage, test_message(&i.to_string(), "burst"))
                .await
                .unwrap();
        }

        for i in 0..total {
            let msg = sub.recv().await.unwrap().unwrap();
            assert_eq!(
                msg.id,
                i.to_string(),
                "every event must arrive, in publish order, with none dropped"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn backpressure_drops_event_and_recovers() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        let id = handle.next_id.fetch_add(1, Ordering::Relaxed);
        let (event_tx, event_rx) = mpsc::channel::<ErasedEvent>(1);
        let topic_id = topics::UserMessage.topic_id();
        handle
            .cmd_tx
            .send(BrokerCommand::Subscribe {
                id,
                topic: topic_id.clone(),
                event_type: TypeId::of::<MessageEvent>(),
                sender: EventSender::Lossy(event_tx),
            })
            .await
            .unwrap();
        let mut small_sub =
            Subscriber::<MessageEvent>::new_lossy(id, topic_id, event_rx, handle.cmd_tx.clone());

        let mut barrier: Subscriber<MessageEvent> =
            handle.subscribe(topics::UserMessage).await.unwrap();

        pub_.publish(topics::UserMessage, test_message("bp1", "fill"))
            .await
            .unwrap();
        pub_.publish(topics::UserMessage, test_message("bp2", "dropped"))
            .await
            .unwrap();
        pub_.publish(topics::UserMessage, test_message("marker", "barrier"))
            .await
            .unwrap();
        loop {
            let seen = barrier.recv().await.unwrap().unwrap();
            if seen.id == "marker" {
                break;
            }
        }

        let first = small_sub.recv().await.unwrap().unwrap();
        assert_eq!(first.id, "bp1");

        let dropped = clock::within(tokio::time::Duration::from_millis(50), small_sub.recv()).await;
        assert!(
            dropped.is_none(),
            "second event should have been dropped due to backpressure"
        );

        pub_.publish(topics::UserMessage, test_message("bp3", "recovered"))
            .await
            .unwrap();

        let recovered = small_sub.recv().await.unwrap().unwrap();
        assert_eq!(recovered.id, "bp3");
    }

    #[tokio::test]
    async fn subscriber_recv_returns_none_when_broker_exits() {
        use std::any::TypeId;

        let handle = spawn_broker();
        let ep = EndpointName::from("ws");
        let topic_id = topics::Endpoint(ep).topic_id();

        let (event_tx, event_rx) = mpsc::channel::<ErasedEvent>(16);
        handle
            .cmd_tx
            .send(BrokerCommand::Subscribe {
                id: 99,
                topic: topic_id.clone(),
                event_type: TypeId::of::<ResponseEvent>(),
                sender: EventSender::Lossy(event_tx),
            })
            .await
            .unwrap();

        let (dead_cmd_tx, dead_cmd_rx) = mpsc::channel::<BrokerCommand>(1);
        drop(dead_cmd_rx);
        let mut sub = Subscriber::<ResponseEvent>::new_lossy(99, topic_id, event_rx, dead_cmd_tx);

        drop(handle);

        let result = sub.recv().await;
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn record_drop_outcome_counts_and_logs_edges_only() {
        let mut dropping = HashMap::new();

        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            Some(DropTransition::Started)
        );
        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            None
        );
        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            None
        );
        match record_drop_outcome(&mut dropping, 1, &SendOutcome::Sent) {
            Some(DropTransition::Stopped { count }) => assert_eq!(count, 3),
            other => panic!("expected Stopped{{count: 3}}, got {other:?}"),
        }
        assert!(
            dropping.is_empty(),
            "recovered subscriber must not linger in the dropping map"
        );
    }

    #[test]
    fn record_drop_outcome_closed_clears_without_a_stop_log() {
        let mut dropping = HashMap::new();
        record_drop_outcome(&mut dropping, 2, &SendOutcome::Dropped);

        assert_eq!(
            record_drop_outcome(&mut dropping, 2, &SendOutcome::Closed),
            None
        );
        assert!(dropping.is_empty());
    }

    #[test]
    fn record_drop_outcome_sent_with_no_prior_drops_is_a_no_op() {
        let mut dropping = HashMap::new();
        assert_eq!(
            record_drop_outcome(&mut dropping, 3, &SendOutcome::Sent),
            None
        );
    }

    #[test]
    fn record_backlog_len_flags_once_then_can_flag_again_after_recovery() {
        let mut large_backlogs = HashSet::new();

        assert!(
            !record_backlog_len(&mut large_backlogs, 1, 100),
            "well under threshold"
        );
        assert!(
            record_backlog_len(&mut large_backlogs, 1, LOSSLESS_BACKLOG_WARN_THRESHOLD),
            "first crossing of the threshold should be flagged"
        );
        assert!(
            !record_backlog_len(&mut large_backlogs, 1, LOSSLESS_BACKLOG_WARN_THRESHOLD + 1),
            "already flagged; staying over threshold must not re-flag"
        );
        assert!(
            !record_backlog_len(&mut large_backlogs, 1, 10),
            "dropping back under threshold clears the flag"
        );
        assert!(
            record_backlog_len(&mut large_backlogs, 1, LOSSLESS_BACKLOG_WARN_THRESHOLD),
            "crossing again after recovery should be flagged again"
        );
    }

    #[tokio::test]
    async fn broker_keeps_running_when_a_lossless_subscriber_has_a_large_backlog() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");

        let _stuck: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        for i in 0..=LOSSLESS_BACKLOG_WARN_THRESHOLD {
            pub_.publish(
                topics::Endpoint(ep.clone()),
                ResponseEvent {
                    correlation_id: i.to_string(),
                    content: "backlog".into(),
                    timestamp: test_timestamp(),
                    attachment: None,
                    conversation: None,
                },
            )
            .await
            .unwrap();
        }

        let ep2 = EndpointName::from("still-alive");
        let mut sub2: Subscriber<ResponseEvent> = handle
            .subscribe(topics::Endpoint(ep2.clone()))
            .await
            .unwrap();
        pub_.publish(
            topics::Endpoint(ep2),
            ResponseEvent {
                correlation_id: "sync".into(),
                content: "sync".into(),
                timestamp: test_timestamp(),
                attachment: None,
                conversation: None,
            },
        )
        .await
        .unwrap();
        let synced = sub2.recv().await.unwrap().unwrap();
        assert_eq!(synced.content, "sync");
    }
}

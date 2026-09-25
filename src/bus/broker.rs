//! Broker task and `BusHandle` factory.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::handle::{BrokerCommand, EventSender, Publisher, SendOutcome, Subscriber};
use super::topics::{Carries, DeliveryMode, Topic};
use super::types::{BusError, TopicId};
use crate::util::spawn_monitored;

/// Command channel capacity for the broker.
const BROKER_COMMAND_CAPACITY: usize = 256;

/// Per-subscriber event channel capacity on a [`DeliveryMode::Lossy`] route.
/// Lossless routes are unbounded — see [`EventSender::Lossless`].
const LOSSY_SUBSCRIBER_CAPACITY: usize = 1024;

/// Queue depth on a [`DeliveryMode::Lossless`] subscriber's channel that
/// flags a stuck consumer rather than a merely slow one. The channel is
/// unbounded so it can never force a drop, but a consumer that stops
/// draining it entirely would otherwise grow that queue forever with
/// nothing making the leak visible.
const LOSSLESS_BACKLOG_WARN_THRESHOLD: usize = 10_000;

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

/// An edge in one subscriber's drop run worth logging — the run's start, or
/// its end together with how many events it cost. Every drop in between is
/// silent: per `CLAUDE.md` "Avoid log spam", only the edges are logged.
#[derive(Debug, PartialEq, Eq)]
enum DropTransition {
    /// The first drop after the subscriber was keeping up.
    Started,
    /// The subscriber caught back up after dropping `count` events.
    Stopped { count: u64 },
}

/// Update `dropping`'s bookkeeping for one subscriber's send `outcome`,
/// returning the transition to log, if any. Pure and side-effect-free
/// besides the map update, so the logging policy above is unit-testable
/// without a running broker.
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

/// Track a lossless subscriber's backlog against
/// [`LOSSLESS_BACKLOG_WARN_THRESHOLD`], returning `true` exactly once per
/// time the backlog crosses into "large" — not on every event while it
/// stays there — so a stuck consumer is flagged without spamming.
fn record_backlog_len(large_backlogs: &mut HashSet<u64>, id: u64, len: usize) -> bool {
    if len >= LOSSLESS_BACKLOG_WARN_THRESHOLD {
        large_backlogs.insert(id)
    } else {
        large_backlogs.remove(&id);
        false
    }
}

/// Broker event loop — owns all subscription state.
///
/// Routes events by `(TopicId, TypeId)` composite key to matching
/// subscribers. Each subscriber's channel is shaped by the delivery mode its
/// `Carries` impl declared at subscribe time (see [`DeliveryMode`]):
/// unbounded and never-drop for `Lossless`, a bounded ring that drops and
/// counts for `Lossy`. Exits naturally when every `BusHandle` (and derived
/// sender) is dropped.
#[tracing::instrument(skip_all)]
async fn run_broker(mut cmd_rx: mpsc::Receiver<BrokerCommand>) {
    debug!("bus broker running");
    let mut subscriptions: HashMap<RouteKey, Vec<(u64, EventSender)>> = HashMap::new();
    // Subscriber ids currently in the middle of a drop run on a lossy route,
    // mapped to how many events that run has dropped so far.
    let mut dropping: HashMap<u64, u64> = HashMap::new();
    // Subscriber ids currently flagged for a large backlog on a lossless
    // route.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::bus::events::{
        MessageEvent, NoticeEvent, ResponseEvent, TurnLifecycleEvent, TurnUsageEvent,
    };
    use crate::bus::handle::ErasedEvent;
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
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: test_timestamp(),
            images: vec![],
            context: None,
        }
    }

    /// A `TurnUsageEvent` on `Endpoint`, a [`DeliveryMode::Lossy`] route,
    /// for tests exercising drop/backpressure behavior.
    fn test_turn_usage(output_tokens: u32) -> TurnUsageEvent {
        TurnUsageEvent {
            correlation_id: "c1".into(),
            output_tokens,
            has_usage: true,
            session_totals: None,
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
            attachment: None,
            conversation: None,
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
                attachment: None,
                conversation: None,
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

    /// `TurnUsageEvent` on `Endpoint` is a lossy (latest-wins) route: a
    /// subscriber that falls behind past its capacity drops the overflow
    /// instead of blocking the broker, and catches up once drained.
    #[tokio::test]
    async fn backpressure_drops_and_recovers() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut sub: Subscriber<TurnUsageEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();
        // sync_sub confirms the broker has processed each publish before we proceed.
        let mut sync_sub: Subscriber<TurnUsageEvent> = handle
            .subscribe(topics::Endpoint(ep.clone()))
            .await
            .unwrap();

        // Fill sub's channel to capacity.
        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            pub_.publish(topics::Endpoint(ep.clone()), test_turn_usage(0))
                .await
                .unwrap();
        }
        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            sync_sub.recv().await.unwrap().unwrap();
        }

        // Sub's channel is full — overflow message should be dropped for sub.
        pub_.publish(topics::Endpoint(ep.clone()), test_turn_usage(u32::MAX))
            .await
            .unwrap();
        sync_sub.recv().await.unwrap().unwrap();

        // Drain all fill messages from sub.
        for _ in 0..LOSSY_SUBSCRIBER_CAPACITY {
            sub.recv().await.unwrap().unwrap();
        }

        // Overflow must not appear in sub's channel.
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(50), sub.recv()).await;
        assert!(
            result.is_err(),
            "full subscriber should not receive overflow event"
        );

        // After recovery, subsequent publishes are received.
        pub_.publish(topics::Endpoint(ep), test_turn_usage(999))
            .await
            .unwrap();
        let msg = sub.recv().await.unwrap().unwrap();
        assert_eq!(msg.output_tokens, 999);
    }

    /// `MessageEvent` on `UserMessage` is a lossless route: a subscriber
    /// that never drains while a burst far larger than a lossy channel's
    /// capacity is published still receives every single event, in order,
    /// once it starts draining.
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

    #[tokio::test]
    async fn backpressure_drops_event_and_recovers() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();

        // Create a subscriber with channel capacity 1 to trigger backpressure.
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

        // Fill the capacity-1 channel with one event, then publish a second that must be dropped.
        pub_.publish(topics::UserMessage, test_message("bp1", "fill"))
            .await
            .unwrap();
        pub_.publish(topics::UserMessage, test_message("bp2", "dropped"))
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // bp1 got through; bp2 was dropped (channel was full).
        let first = small_sub.recv().await.unwrap().unwrap();
        assert_eq!(first.id, "bp1");

        let dropped =
            tokio::time::timeout(tokio::time::Duration::from_millis(50), small_sub.recv()).await;
        assert!(
            dropped.is_err(),
            "second event should have been dropped due to backpressure"
        );

        // Recovery: now that the channel is drained, the next publish succeeds.
        pub_.publish(topics::UserMessage, test_message("bp3", "recovered"))
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let recovered = small_sub.recv().await.unwrap().unwrap();
        assert_eq!(recovered.id, "bp3");
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
                sender: EventSender::Lossy(event_tx),
            })
            .await
            .unwrap();

        // Create subscriber with a disconnected cmd_tx so it does not keep the broker alive.
        let (dead_cmd_tx, dead_cmd_rx) = mpsc::channel::<BrokerCommand>(1);
        drop(dead_cmd_rx);
        let mut sub = Subscriber::<ResponseEvent>::new_lossy(99, topic_id, event_rx, dead_cmd_tx);

        // Drop the handle — no remaining cmd_tx senders; broker will exit.
        drop(handle);

        // Broker exits and drops subscriptions, closing event_tx; recv returns Ok(None).
        let result = sub.recv().await;
        assert!(matches!(result, Ok(None)));
    }

    // -----------------------------------------------------------------------
    // Drop/backlog bookkeeping (pure functions, no broker needed)
    // -----------------------------------------------------------------------

    #[test]
    fn record_drop_outcome_counts_and_logs_edges_only() {
        let mut dropping = HashMap::new();

        // First drop in a run: log the start.
        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            Some(DropTransition::Started)
        );
        // Every further drop in the same run is silent...
        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            None
        );
        assert_eq!(
            record_drop_outcome(&mut dropping, 1, &SendOutcome::Dropped),
            None
        );
        // ...but the run's total is still counted internally.
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

        // A subscriber that disconnects mid-drop-run isn't "recovered" —
        // no Stopped transition, just silent cleanup.
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

    /// A lossless subscriber that never drains grows an unbounded backlog
    /// rather than causing a publish to fail or the broker to stall — the
    /// backlog-warning bookkeeping (unit-tested above) rides alongside this
    /// without changing that behavior, and other subscribers on other
    /// routes are unaffected.
    #[tokio::test]
    async fn broker_keeps_running_when_a_lossless_subscriber_has_a_large_backlog() {
        let handle = spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");

        // A lossless subscriber that never drains.
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

        // The broker is still alive and processing — a lossless route never
        // rejects a publish, it just piles up in the unbounded channel.
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

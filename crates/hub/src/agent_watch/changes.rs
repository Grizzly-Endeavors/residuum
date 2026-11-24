use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, mpsc};

use residuum_bus::remote_task::TrackedTask;
use residuum_bus::session::{SessionInfo, SessionState};
use residuum_bus::{AgentResultStatus, PulseFireOutcome, SessionAddress, SessionEvent};
use residuum_memory::types::Visibility;

const SESSION_RELAY_CAPACITY: usize = 1024;

const BACKLOG_WARN_THRESHOLD: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WatchedPath {
    UserInbox,
    ScheduledActions,
    Heartbeat,
    PulseState,
    Config,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MainTurnEnded {
    pub user_message: Option<String>,
    pub reply: Option<String>,
    pub at: DateTime<Utc>,
    pub visibility: Visibility,
    pub client_connected: bool,
}

#[derive(Debug, Clone)]
pub enum AgentChangeKind {
    Resync,
    SessionStarted(Box<SessionInfo>),
    SessionStateChanged {
        address: SessionAddress,
        run_id: String,
        state: SessionState,
    },
    SessionCompleted {
        address: SessionAddress,
        run_id: String,
        status: AgentResultStatus,
    },
    OutboundTaskChanged(Box<TrackedTask>),
    UserInboxAdded {
        item_id: String,
    },
    PulseFireFinished {
        pulse: String,
        outcome: PulseFireOutcome,
    },
    WatchedPathChanged(WatchedPath),
    TurnEnded(MainTurnEnded),
}

#[derive(Debug, Clone)]
pub struct AgentChange {
    pub agent: String,
    pub kind: AgentChangeKind,
}

#[derive(Debug, Clone)]
pub struct AgentSessionEvent {
    pub agent: String,
    pub source_label: Option<String>,
    pub event: SessionEvent,
}

struct ChangeSubscriber {
    tx: mpsc::UnboundedSender<AgentChange>,
    backlog: Arc<AtomicUsize>,
}

pub struct AgentChangeFeed {
    subscribers: Mutex<Vec<ChangeSubscriber>>,
    sessions: broadcast::Sender<AgentSessionEvent>,
}

impl AgentChangeFeed {
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (sessions, _first_receiver) = broadcast::channel(SESSION_RELAY_CAPACITY);
        Arc::new(Self {
            subscribers: Mutex::new(Vec::new()),
            sessions,
        })
    }

    pub fn subscribe(&self) -> AgentChangeReceiver {
        let (tx, rx) = mpsc::unbounded_channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ChangeSubscriber {
                tx,
                backlog: Arc::clone(&backlog),
            });
        AgentChangeReceiver { rx, backlog }
    }

    /// Subscribe to every session event of every running agent from now on,
    /// for the session relay.
    ///
    /// A receiver that falls more than 1,024 events behind gets
    /// [`broadcast::error::RecvError::Lagged`] with the number it missed, so
    /// the consumer can tell its client to re-read the sessions it follows.
    #[must_use]
    pub fn subscribe_sessions(&self) -> broadcast::Receiver<AgentSessionEvent> {
        self.sessions.subscribe()
    }

    pub(crate) fn publish(&self, change: &AgentChange) {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain(|subscriber| {
            let depth = subscriber.backlog.fetch_add(1, Ordering::Relaxed) + 1;
            if depth == BACKLOG_WARN_THRESHOLD {
                tracing::warn!(
                    queued = depth,
                    "a consumer of agent changes isn't keeping up; it may be stuck"
                );
            }
            subscriber.tx.send(change.clone()).is_ok()
        });
    }

    pub(crate) fn relay_session(&self, event: AgentSessionEvent) {
        self.sessions.send(event).ok();
    }

    #[cfg(test)]
    pub(crate) fn session_relay_receivers(&self) -> usize {
        self.sessions.receiver_count()
    }
}

pub struct AgentChangeReceiver {
    rx: mpsc::UnboundedReceiver<AgentChange>,
    backlog: Arc<AtomicUsize>,
}

impl AgentChangeReceiver {
    pub async fn recv(&mut self) -> Option<AgentChange> {
        let change = self.rx.recv().await;
        if change.is_some() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
        change
    }

    pub fn try_recv(&mut self) -> Option<AgentChange> {
        let change = self.rx.try_recv().ok();
        if change.is_some() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
        change
    }
}

#[cfg(any(test, feature = "test-support"))]
impl residuum_core::testing::wait::Recv for AgentChangeReceiver {
    type Item = AgentChange;

    async fn recv_next(&mut self) -> Option<Self::Item> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<Self::Item> {
        self.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(agent: &str, item: &str) -> AgentChange {
        AgentChange {
            agent: agent.to_string(),
            kind: AgentChangeKind::UserInboxAdded {
                item_id: item.to_string(),
            },
        }
    }

    fn next(receiver: &mut AgentChangeReceiver) -> Option<AgentChange> {
        receiver.try_recv()
    }

    #[test]
    fn every_subscriber_gets_every_change_in_order() {
        let feed = AgentChangeFeed::new();
        let mut first = feed.subscribe();
        let mut second = feed.subscribe();
        for n in 0..3 {
            feed.publish(&change("scout", &n.to_string()));
        }
        for receiver in [&mut first, &mut second] {
            for n in 0..3 {
                let got = next(receiver).expect("a queued change");
                assert_eq!(got.agent, "scout");
                assert!(
                    matches!(got.kind, AgentChangeKind::UserInboxAdded { ref item_id } if *item_id == n.to_string()),
                    "change {n} arrives in order: {got:?}"
                );
            }
            assert!(next(receiver).is_none());
        }
    }

    #[test]
    fn a_subscriber_hears_only_what_comes_after_it_subscribes() {
        let feed = AgentChangeFeed::new();
        feed.publish(&change("scout", "before"));
        let mut late = feed.subscribe();
        assert!(next(&mut late).is_none(), "nothing is replayed");
        feed.publish(&change("scout", "after"));
        assert!(next(&mut late).is_some());
    }

    #[test]
    fn a_dropped_receiver_stops_costing_the_feed_anything() {
        let feed = AgentChangeFeed::new();
        let gone = feed.subscribe();
        let mut kept = feed.subscribe();
        drop(gone);
        feed.publish(&change("scout", "1"));
        assert!(next(&mut kept).is_some());
        assert_eq!(
            feed.subscribers.lock().unwrap().len(),
            1,
            "the closed queue is pruned"
        );
    }

    #[test]
    fn a_receiver_never_reading_loses_nothing() {
        let feed = AgentChangeFeed::new();
        let mut receiver = feed.subscribe();
        for n in 0..BACKLOG_WARN_THRESHOLD + 5 {
            feed.publish(&change("scout", &n.to_string()));
        }
        let mut count = 0;
        while next(&mut receiver).is_some() {
            count += 1;
        }
        assert_eq!(count, BACKLOG_WARN_THRESHOLD + 5);
        assert_eq!(receiver.backlog.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn session_events_reach_each_receiver_and_a_lagging_one_is_told() {
        let feed = AgentChangeFeed::new();
        let mut keeping_up = feed.subscribe_sessions();
        let mut lagging = feed.subscribe_sessions();
        let event = |n: usize| AgentSessionEvent {
            agent: "scout".to_string(),
            source_label: Some("artifact:notes".to_string()),
            event: SessionEvent {
                address: SessionAddress::from("spawned-x"),
                run_id: n.to_string(),
                kind: residuum_bus::SessionEventKind::TurnStarted {
                    turn_id: n.to_string(),
                },
            },
        };
        for n in 0..SESSION_RELAY_CAPACITY {
            feed.relay_session(event(n));
            keeping_up.recv().await.unwrap();
        }
        feed.relay_session(event(SESSION_RELAY_CAPACITY));
        assert_eq!(
            keeping_up.recv().await.unwrap().event.run_id,
            SESSION_RELAY_CAPACITY.to_string()
        );
        assert!(matches!(
            lagging.recv().await,
            Err(broadcast::error::RecvError::Lagged(1))
        ));
    }
}

//! The hub's typed stream of agent changes, and the feed that carries it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, mpsc};

use crate::a2a::TrackedTask;
use crate::background::registry::{SessionInfo, SessionState};
use crate::bus::{AgentResultStatus, SessionAddress, SessionEvent};
use crate::memory::types::Visibility;

/// How many session events a slow relay consumer can fall behind by before
/// it is told it lost some (see [`AgentChangeFeed::subscribe_sessions`]).
const SESSION_RELAY_CAPACITY: usize = 1024;

/// Queue depth at which a change subscriber is reported as not keeping up.
/// The queue is unbounded so no change is ever lost, but a consumer that
/// stops reading would otherwise grow it without anything showing.
const BACKLOG_WARN_THRESHOLD: usize = 10_000;

/// A file the watcher reports changes to, named by what reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WatchedPath {
    /// A top-level `*.json` item in `inbox/user/`, or one of the directories
    /// that hold them: the user inbox's contents changed, so its unread
    /// count may have.
    UserInbox,
    /// `scheduled_actions.json`: the one-off scheduled actions changed.
    ScheduledActions,
    /// `HEARTBEAT.yml`: the pulse definitions changed.
    Heartbeat,
    /// `pulse_state.json`: a pulse ran, so its next run moved.
    PulseState,
    /// A file in the agent's `config/` directory: its settings changed.
    Config,
}

/// What the agent's last main turn did, reported by the runtime exactly once
/// when the turn ends (see
/// [`ActivityTracker::main_turn_ended`](crate::hub::activity::ActivityTracker::main_turn_ended)).
#[derive(Debug, Clone, PartialEq)]
pub struct MainTurnEnded {
    /// What the user said that started the turn. `None` for a turn with
    /// `background` visibility, which no user message started, and for a
    /// message without text.
    pub user_message: Option<String>,
    /// The turn's last reply text, if it produced one.
    pub reply: Option<String>,
    /// When the turn ended.
    pub at: DateTime<Utc>,
    /// Whether a user was part of the turn or it ran in the background.
    pub visibility: Visibility,
    /// Whether any client had the agent's WebSocket open as the turn ended.
    pub client_connected: bool,
}

/// What changed about an agent.
#[derive(Debug, Clone)]
pub enum AgentChangeKind {
    /// Anything about the agent may have changed: recompute all of it. Sent
    /// when the agent finishes starting (after which every later change
    /// reaches the stream), when the agent's file watcher says it may have
    /// missed changes, and every 60 seconds while that watcher is down.
    Resync,
    /// A session run was registered.
    SessionStarted(Box<SessionInfo>),
    /// A session run moved to another lifecycle state (`completed` is
    /// reported by [`Self::SessionCompleted`]).
    SessionStateChanged {
        /// The session's address.
        address: SessionAddress,
        /// The run within the session.
        run_id: String,
        /// The state it moved to.
        state: SessionState,
    },
    /// A session run finished and left the registry.
    SessionCompleted {
        /// The session's address.
        address: SessionAddress,
        /// The run within the session.
        run_id: String,
        /// How the run's last turn ended.
        status: AgentResultStatus,
        /// The episode the run was merged into, if it produced one.
        episode_id: Option<String>,
    },
    /// An outbound A2A task the agent sent was recorded or changed. Sent at
    /// the start of an unreachable streak, when the streak passes the
    /// tracker's notice threshold (the task's `unreachable_notified` turns
    /// true), and when the streak ends.
    OutboundTaskChanged(Box<TrackedTask>),
    /// The `user_inbox_add` tool saved an item. Only that tool announces
    /// items this way; every change to the inbox's files, this one included,
    /// also arrives as a [`Self::WatchedPathChanged`].
    UserInboxAdded {
        /// The item's id.
        item_id: String,
    },
    /// A file the watcher follows changed. Sent once per kind per batch of
    /// file changes.
    WatchedPathChanged(WatchedPath),
    /// A main turn ended.
    TurnEnded(MainTurnEnded),
}

/// One change to one agent.
#[derive(Debug, Clone)]
pub struct AgentChange {
    /// The agent's name.
    pub agent: String,
    /// What changed.
    pub kind: AgentChangeKind,
}

/// One event of one agent's sessions, exactly as the agent's bus carried it.
#[derive(Debug, Clone)]
pub struct AgentSessionEvent {
    /// The agent the session belongs to.
    pub agent: String,
    /// The session run's source label (`artifact:notes`, `pulse:email`, ...),
    /// from its start event. `None` when the run started before the watcher
    /// attached and is no longer in the agent's registry.
    pub source_label: Option<String>,
    /// The event: a lifecycle change, or one of the session's turn events.
    pub event: SessionEvent,
}

/// One subscriber's queue.
struct ChangeSubscriber {
    tx: mpsc::UnboundedSender<AgentChange>,
    /// Changes queued and not yet received.
    backlog: Arc<AtomicUsize>,
}

/// Where the hub's agent changes are published, for the hub's own consumers.
///
/// Every running agent's watcher, and the activity tracker's turn hook,
/// publish here. Nothing is retained: a subscriber hears about changes from
/// the moment it subscribes, and a change published while nobody listens is
/// gone. The consumers recompute from disk and the agent's registry, which is
/// what [`AgentChangeKind::Resync`] asks for.
pub struct AgentChangeFeed {
    subscribers: Mutex<Vec<ChangeSubscriber>>,
    sessions: broadcast::Sender<AgentSessionEvent>,
}

impl AgentChangeFeed {
    /// A feed with no subscribers.
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (sessions, _first_receiver) = broadcast::channel(SESSION_RELAY_CAPACITY);
        Arc::new(Self {
            subscribers: Mutex::new(Vec::new()),
            sessions,
        })
    }

    /// Subscribe to every agent's changes from now on.
    ///
    /// Delivery is lossless and in order per agent: each subscriber has its
    /// own unbounded queue, so it must keep reading. A queue that reaches
    /// 10,000 changes is logged once as a stuck consumer.
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

    /// Publish `change` to every subscriber.
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
            // A closed queue means its consumer is gone.
            subscriber.tx.send(change.clone()).is_ok()
        });
    }

    /// Publish one session event for the session relay.
    pub(crate) fn relay_session(&self, event: AgentSessionEvent) {
        // No receivers is the normal state until a client asks for sessions.
        self.sessions.send(event).ok();
    }

    /// How many receivers of the session relay are open.
    #[cfg(test)]
    pub(crate) fn session_relay_receivers(&self) -> usize {
        self.sessions.receiver_count()
    }
}

/// One subscriber's end of the feed.
pub struct AgentChangeReceiver {
    rx: mpsc::UnboundedReceiver<AgentChange>,
    backlog: Arc<AtomicUsize>,
}

impl AgentChangeReceiver {
    /// The next change, waiting for one. `None` once the feed is gone.
    pub async fn recv(&mut self) -> Option<AgentChange> {
        let change = self.rx.recv().await;
        if change.is_some() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
        change
    }

    /// The next change if one is already queued, without waiting. Since
    /// [`AgentChangeFeed::publish`] queues for every subscriber before it
    /// returns, a caller that knows a change was published can read it here.
    pub fn try_recv(&mut self) -> Option<AgentChange> {
        let change = self.rx.try_recv().ok();
        if change.is_some() {
            self.backlog.fetch_sub(1, Ordering::Relaxed);
        }
        change
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

    /// What `publish` already queued: it queues for every subscriber before
    /// returning, so nothing more can be on its way.
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
                kind: crate::bus::SessionEventKind::TurnStarted {
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

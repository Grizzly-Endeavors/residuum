//! Main-conversation activity for one agent: whether a main turn is running,
//! and how many messages the web UI has not shown yet.
//!
//! The rail and Home show both. `busy` follows the main turn, and `busy_since`
//! says when that turn began. `unread` counts
//! main-conversation messages published while no web client is connected to
//! the agent's WebSocket, and resets when a client connects. Every change is
//! published on the hub bus as a [`HubEvent::AgentActivity`].
//!
//! The tracker is also the **turn hook**: the runtime calls
//! [`ActivityTracker::main_turn_ended`] once when a main turn ends, and the
//! turn reaches the hub's [`AgentChangeFeed`] as a [`MainTurnEnded`].

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use super::agent_watch::{AgentChange, AgentChangeFeed, AgentChangeKind, MainTurnEnded};
use super::types::{AgentActivity, HubEvent, NoticeLevel};
use crate::memory::types::Visibility;

#[derive(Default)]
struct ActivityState {
    /// When the current main turn began, while one is running.
    busy_since: Option<DateTime<Utc>>,
    unread: u32,
    /// Web clients currently connected to the agent's `/ws`.
    clients: usize,
    /// Bumped whenever the agent stops, so a connection guard from the run
    /// that ended cannot unbalance the next run's client count.
    run: u64,
}

/// Tracks one agent's [`AgentActivity`] and publishes its changes.
pub struct ActivityTracker {
    name: String,
    state: Mutex<ActivityState>,
    events: broadcast::Sender<HubEvent>,
    /// Where the turn hook reports each main turn's end.
    changes: Arc<AgentChangeFeed>,
}

impl ActivityTracker {
    /// A tracker for the agent `name` that publishes activity on `events`
    /// and reports each main turn's end on `changes`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        events: broadcast::Sender<HubEvent>,
        changes: Arc<AgentChangeFeed>,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            state: Mutex::new(ActivityState::default()),
            events,
            changes,
        })
    }

    /// The current activity.
    #[must_use]
    pub fn snapshot(&self) -> AgentActivity {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::activity_of(&state)
    }

    /// How many web clients are connected to the agent's `/ws` now.
    #[must_use]
    pub fn connected_clients(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clients
    }

    fn activity_of(state: &ActivityState) -> AgentActivity {
        AgentActivity {
            busy: state.busy_since.is_some(),
            busy_since: state.busy_since,
            unread: state.unread,
        }
    }

    /// Apply `change` to the state and publish the new activity if it differs.
    fn update(&self, change: impl FnOnce(&mut ActivityState)) {
        let (before, after) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let before = Self::activity_of(&state);
            change(&mut state);
            (before, Self::activity_of(&state))
        };
        if before != after {
            // No subscribers is the normal state until a client opens the
            // hub WebSocket.
            self.events
                .send(HubEvent::AgentActivity {
                    name: self.name.clone(),
                    activity: after,
                })
                .ok();
        }
    }

    /// Mark a main turn as running until the returned guard drops.
    #[must_use]
    pub fn main_turn(self: &Arc<Self>) -> MainTurnGuard {
        self.update(|state| {
            state.busy_since.get_or_insert_with(Utc::now);
        });
        MainTurnGuard {
            tracker: Arc::clone(self),
        }
    }

    /// A main-conversation message was published. It counts as unread when no
    /// web client is connected to see it.
    pub fn main_message_published(&self) {
        self.update(|state| {
            if state.clients == 0 {
                state.unread = state.unread.saturating_add(1);
            }
        });
    }

    /// The turn hook: a main turn ended. The runtime calls this exactly once
    /// per turn, after the turn's replies were published and counted by
    /// [`Self::main_message_published`].
    ///
    /// `user_message` is what the user said to start the turn, `reply` the
    /// turn's last reply text, and `visibility` whether a user was part of it.
    /// Text that is empty or only whitespace counts as no text. The turn is
    /// reported with the time, and whether any client has the agent's
    /// WebSocket open right now.
    pub fn main_turn_ended(
        &self,
        user_message: Option<String>,
        reply: Option<String>,
        visibility: Visibility,
    ) {
        let client_connected = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clients
            > 0;
        self.changes.publish(&AgentChange {
            agent: self.name.clone(),
            kind: AgentChangeKind::TurnEnded(MainTurnEnded {
                user_message: user_message.filter(|text| !text.trim().is_empty()),
                reply: reply.filter(|text| !text.trim().is_empty()),
                at: Utc::now(),
                visibility,
                client_connected,
            }),
        });
    }

    /// A web client connected to the agent's `/ws`. Resets the unread count;
    /// the client is counted as connected until the guard drops.
    #[must_use]
    pub fn client_connected(self: &Arc<Self>) -> ClientGuard {
        let mut run = 0;
        self.update(|state| {
            state.clients += 1;
            state.unread = 0;
            run = state.run;
        });
        ClientGuard {
            tracker: Arc::clone(self),
            run,
        }
    }

    /// Tell the user something about this agent as a hub notice.
    pub fn hub_notice(&self, level: NoticeLevel, message: String) {
        self.events
            .send(HubEvent::Notice {
                level,
                message,
                agent: Some(self.name.clone()),
            })
            .ok();
    }

    /// The agent stopped: nothing is running and no client is connected any
    /// more. The unread count is kept.
    pub fn run_ended(&self) {
        self.update(|state| {
            state.busy_since = None;
            state.clients = 0;
            state.run += 1;
        });
    }
}

/// Keeps an agent marked busy while a main turn runs.
pub struct MainTurnGuard {
    tracker: Arc<ActivityTracker>,
}

impl Drop for MainTurnGuard {
    fn drop(&mut self) {
        self.tracker.update(|state| state.busy_since = None);
    }
}

/// Counts one connected web client until dropped.
pub struct ClientGuard {
    tracker: Arc<ActivityTracker>,
    run: u64,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        let run = self.run;
        self.tracker.update(|state| {
            if state.run == run {
                state.clients = state.clients.saturating_sub(1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> (Arc<ActivityTracker>, broadcast::Receiver<HubEvent>) {
        let (tx, rx) = broadcast::channel(16);
        (
            ActivityTracker::new("scout", tx, AgentChangeFeed::new()),
            rx,
        )
    }

    fn drain(rx: &mut broadcast::Receiver<HubEvent>) -> Vec<AgentActivity> {
        let mut seen = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let HubEvent::AgentActivity { name, activity } = event {
                assert_eq!(name, "scout");
                seen.push(activity);
            }
        }
        seen
    }

    #[test]
    fn a_main_turn_marks_the_agent_busy_from_its_start_until_it_ends() {
        let (tracker, mut rx) = tracker();
        let before = Utc::now();
        let guard = tracker.main_turn();
        let after = Utc::now();
        let during = tracker.snapshot();
        assert!(during.busy);
        let since = during.busy_since.expect("a running turn has a start time");
        assert!(
            before <= since && since <= after,
            "the turn started between {before} and {after}, not at {since}"
        );

        drop(guard);
        assert_eq!(
            tracker.snapshot(),
            AgentActivity {
                busy: false,
                busy_since: None,
                unread: 0
            }
        );
        assert_eq!(
            drain(&mut rx),
            [
                AgentActivity {
                    busy: true,
                    busy_since: Some(since),
                    unread: 0
                },
                AgentActivity {
                    busy: false,
                    busy_since: None,
                    unread: 0
                },
            ]
        );
    }

    #[test]
    fn an_overlapping_guard_keeps_the_turn_start() {
        let (tracker, mut rx) = tracker();
        let first = tracker.main_turn();
        let since = tracker.snapshot().busy_since;
        let second = tracker.main_turn();
        assert_eq!(tracker.snapshot().busy_since, since);
        assert_eq!(
            drain(&mut rx).len(),
            1,
            "only the first guard changes anything"
        );
        drop(second);
        drop(first);
    }

    #[test]
    fn a_stopped_agent_is_not_busy() {
        let (tracker, _rx) = tracker();
        let guard = tracker.main_turn();
        tracker.run_ended();
        assert_eq!(tracker.snapshot().busy_since, None);
        assert!(!tracker.snapshot().busy);
        drop(guard);
    }

    #[test]
    fn messages_are_unread_only_while_no_client_is_connected() {
        let (tracker, _rx) = tracker();
        tracker.main_message_published();
        tracker.main_message_published();
        assert_eq!(tracker.snapshot().unread, 2);

        let client = tracker.client_connected();
        assert_eq!(tracker.snapshot().unread, 0, "connecting resets the count");
        tracker.main_message_published();
        assert_eq!(
            tracker.snapshot().unread,
            0,
            "a connected client shows the message"
        );

        drop(client);
        tracker.main_message_published();
        assert_eq!(tracker.snapshot().unread, 1);
    }

    #[test]
    fn unread_stays_while_any_client_remains_connected() {
        let (tracker, _rx) = tracker();
        let first = tracker.client_connected();
        let second = tracker.client_connected();
        drop(first);
        tracker.main_message_published();
        assert_eq!(tracker.snapshot().unread, 0);
        drop(second);
        tracker.main_message_published();
        assert_eq!(tracker.snapshot().unread, 1);
    }

    #[test]
    fn connected_clients_counts_open_guards_of_the_current_run() {
        let (tracker, _rx) = tracker();
        assert_eq!(tracker.connected_clients(), 0);
        let first = tracker.client_connected();
        let second = tracker.client_connected();
        assert_eq!(tracker.connected_clients(), 2);
        drop(first);
        assert_eq!(tracker.connected_clients(), 1);
        tracker.run_ended();
        assert_eq!(
            tracker.connected_clients(),
            0,
            "a stop disconnects everyone"
        );
        drop(second);
        assert_eq!(tracker.connected_clients(), 0);
    }

    #[test]
    fn a_guard_from_an_ended_run_does_not_unbalance_the_next_run() {
        let (tracker, _rx) = tracker();
        let old_client = tracker.client_connected();
        tracker.run_ended();
        let new_client = tracker.client_connected();
        drop(old_client);
        tracker.main_message_published();
        assert_eq!(
            tracker.snapshot().unread,
            0,
            "the new run's client is still connected"
        );
        drop(new_client);
    }

    #[test]
    fn an_unchanged_activity_is_not_republished() {
        let (tracker, mut rx) = tracker();
        let _client = tracker.client_connected();
        assert!(drain(&mut rx).is_empty(), "nothing changed on connect");
        tracker.main_message_published();
        assert!(drain(&mut rx).is_empty(), "a shown message is not unread");
    }

    /// A tracker whose turn hook reports to the returned receiver.
    fn tracker_with_changes() -> (
        Arc<ActivityTracker>,
        crate::hub::agent_watch::AgentChangeReceiver,
    ) {
        let feed = AgentChangeFeed::new();
        let receiver = feed.subscribe();
        let (tx, _rx) = broadcast::channel(16);
        (ActivityTracker::new("scout", tx, feed), receiver)
    }

    async fn next_turn(
        receiver: &mut crate::hub::agent_watch::AgentChangeReceiver,
    ) -> MainTurnEnded {
        let change = crate::testing::wait::next("the turn hook's report", receiver).await;
        assert_eq!(change.agent, "scout");
        let AgentChangeKind::TurnEnded(turn) = change.kind else {
            panic!("expected a turn ending, got {:?}", change.kind);
        };
        turn
    }

    #[tokio::test]
    async fn the_turn_hook_reports_the_turn_once_with_its_texts_and_visibility() {
        let (tracker, mut changes) = tracker_with_changes();
        let before = Utc::now();
        tracker.main_turn_ended(
            Some("what's on today?".to_string()),
            Some("Two meetings.".to_string()),
            Visibility::User,
        );
        let turn = next_turn(&mut changes).await;
        assert_eq!(turn.user_message.as_deref(), Some("what's on today?"));
        assert_eq!(turn.reply.as_deref(), Some("Two meetings."));
        assert_eq!(turn.visibility, Visibility::User);
        assert!(before <= turn.at && turn.at <= Utc::now());
        // The hook publishes before it returns, so a second event would
        // already be queued.
        assert!(changes.try_recv().is_none(), "one call, one event");

        tracker.main_turn_ended(
            None,
            Some("Checked the feeds.".to_string()),
            Visibility::Background,
        );
        let background = next_turn(&mut changes).await;
        assert_eq!(background.user_message, None);
        assert_eq!(background.reply.as_deref(), Some("Checked the feeds."));
        assert_eq!(background.visibility, Visibility::Background);
    }

    #[tokio::test]
    async fn blank_text_is_reported_as_no_text() {
        let (tracker, mut changes) = tracker_with_changes();
        tracker.main_turn_ended(
            Some("  \n".to_string()),
            Some(String::new()),
            Visibility::User,
        );
        let turn = next_turn(&mut changes).await;
        assert_eq!(turn.user_message, None);
        assert_eq!(turn.reply, None);
    }

    #[tokio::test]
    async fn the_turn_hook_says_whether_a_client_had_the_socket_open() {
        let (tracker, mut changes) = tracker_with_changes();
        tracker.main_turn_ended(None, None, Visibility::User);
        assert!(!next_turn(&mut changes).await.client_connected);

        let client = tracker.client_connected();
        tracker.main_turn_ended(None, None, Visibility::User);
        assert!(next_turn(&mut changes).await.client_connected);

        drop(client);
        tracker.main_turn_ended(None, None, Visibility::User);
        assert!(!next_turn(&mut changes).await.client_connected);
    }

    #[tokio::test]
    async fn the_turn_hook_leaves_unread_counting_alone() {
        let (tracker, _changes) = tracker_with_changes();
        tracker.main_message_published();
        tracker.main_turn_ended(None, Some("hi".to_string()), Visibility::User);
        assert_eq!(tracker.snapshot().unread, 1);
    }
}

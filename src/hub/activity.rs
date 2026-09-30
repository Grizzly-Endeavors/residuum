//! Main-conversation activity for one agent: whether a main turn is running,
//! and how many messages the web UI has not shown yet.
//!
//! The switcher shows both. `busy` follows the main turn. `unread` counts
//! main-conversation messages published while no web client is connected to
//! the agent's WebSocket, and resets when a client connects. Every change is
//! published on the hub bus as a [`HubEvent::AgentActivity`].

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use super::types::{AgentActivity, HubEvent, NoticeLevel};

#[derive(Default)]
struct ActivityState {
    busy: bool,
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
}

impl ActivityTracker {
    /// A tracker for the agent `name` that publishes on `events`.
    #[must_use]
    pub fn new(name: impl Into<String>, events: broadcast::Sender<HubEvent>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            state: Mutex::new(ActivityState::default()),
            events,
        })
    }

    /// The current activity.
    #[must_use]
    pub fn snapshot(&self) -> AgentActivity {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::activity_of(&state)
    }

    fn activity_of(state: &ActivityState) -> AgentActivity {
        AgentActivity {
            busy: state.busy,
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
        self.update(|state| state.busy = true);
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
            state.busy = false;
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
        self.tracker.update(|state| state.busy = false);
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
        (ActivityTracker::new("scout", tx), rx)
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
    fn a_main_turn_marks_the_agent_busy_until_it_ends() {
        let (tracker, mut rx) = tracker();
        let guard = tracker.main_turn();
        assert!(tracker.snapshot().busy);
        drop(guard);
        assert!(!tracker.snapshot().busy);
        assert_eq!(
            drain(&mut rx),
            [
                AgentActivity {
                    busy: true,
                    unread: 0
                },
                AgentActivity {
                    busy: false,
                    unread: 0
                },
            ]
        );
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
}

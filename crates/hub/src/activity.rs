use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use super::agent_watch::{AgentChange, AgentChangeFeed, AgentChangeKind, MainTurnEnded};
use super::types::{AgentActivity, HubEvent, NoticeLevel};
use residuum_memory::types::Visibility;

#[derive(Default)]
struct ActivityState {
    busy_since: Option<DateTime<Utc>>,
    unread: u32,
    restart_pending: bool,
    clients: usize,
    last_reply_seen: bool,
    run: u64,
}

pub struct ActivityTracker {
    name: String,
    state: Mutex<ActivityState>,
    events: broadcast::Sender<HubEvent>,
    turn_state_tx: tokio::sync::watch::Sender<(bool, bool)>,
    changes: Arc<AgentChangeFeed>,
}

impl ActivityTracker {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        events: broadcast::Sender<HubEvent>,
        changes: Arc<AgentChangeFeed>,
    ) -> Arc<Self> {
        let (turn_state_tx, _) = tokio::sync::watch::channel((false, false));
        Arc::new(Self {
            name: name.into(),
            state: Mutex::new(ActivityState::default()),
            events,
            turn_state_tx,
            changes,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> AgentActivity {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Self::activity_of(&state)
    }

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
            restart_pending: state.restart_pending,
        }
    }

    fn update(&self, change: impl FnOnce(&mut ActivityState)) {
        let (before, after) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let before = Self::activity_of(&state);
            change(&mut state);
            let after = Self::activity_of(&state);
            self.turn_state_tx
                .send_replace((after.busy, after.restart_pending));
            (before, after)
        };
        if before != after {
            self.events
                .send(HubEvent::AgentActivity {
                    name: self.name.clone(),
                    activity: after,
                })
                .ok();
        }
    }

    pub(crate) fn defer_restart_if_busy(&self) -> (bool, bool) {
        let mut deferred = false;
        let mut newly_scheduled = false;
        self.update(|state| {
            if state.restart_pending {
                deferred = true;
            } else if state.busy_since.is_some() {
                state.restart_pending = true;
                deferred = true;
                newly_scheduled = true;
            }
        });
        (deferred, newly_scheduled)
    }

    pub(crate) fn take_restart_pending(&self) -> bool {
        let mut was_pending = false;
        self.update(|state| {
            was_pending = std::mem::take(&mut state.restart_pending);
        });
        was_pending
    }

    pub(crate) async fn wait_until_turn_ends_or_cancelled(&self) {
        let mut rx = self.turn_state_tx.subscribe();
        rx.wait_for(|&(busy, restart_pending)| !busy || !restart_pending)
            .await
            .ok();
    }

    #[must_use]
    pub fn main_turn(self: &Arc<Self>) -> MainTurnGuard {
        self.update(|state| {
            state.busy_since.get_or_insert_with(Utc::now);
        });
        MainTurnGuard {
            tracker: Arc::clone(self),
        }
    }

    pub fn main_message_published(&self) {
        self.update(|state| {
            let seen = state.clients > 0;
            if !seen {
                state.unread = state.unread.saturating_add(1);
            }
            state.last_reply_seen = seen;
        });
    }

    pub fn main_turn_ended(
        &self,
        user_message: Option<String>,
        reply: Option<String>,
        visibility: Visibility,
    ) {
        let client_connected = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let reply_seen = std::mem::take(&mut state.last_reply_seen);
            reply_seen || state.clients > 0
        };
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

    pub fn hub_notice(&self, level: NoticeLevel, message: String) {
        self.events
            .send(HubEvent::notice(level, message, Some(self.name.clone())))
            .ok();
    }

    pub fn run_ended(&self) {
        self.update(|state| {
            state.busy_since = None;
            state.restart_pending = false;
            state.clients = 0;
            state.last_reply_seen = false;
            state.run += 1;
        });
    }
}

impl residuum_agent::ReplyAudience for ActivityTracker {
    fn reply_published(&self) {
        self.main_message_published();
    }
}

pub struct MainTurnGuard {
    tracker: Arc<ActivityTracker>,
}

impl Drop for MainTurnGuard {
    fn drop(&mut self) {
        self.tracker.update(|state| state.busy_since = None);
    }
}

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
                unread: 0,
                restart_pending: false,
            }
        );
        assert_eq!(
            drain(&mut rx),
            [
                AgentActivity {
                    busy: true,
                    busy_since: Some(since),
                    unread: 0,
                    restart_pending: false,
                },
                AgentActivity {
                    busy: false,
                    busy_since: None,
                    unread: 0,
                    restart_pending: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn defer_restart_if_busy_coalesces_duplicates_and_notifies_on_turn_end() {
        let (tracker, mut rx) = tracker();
        assert_eq!(tracker.defer_restart_if_busy(), (false, false));
        assert!(!tracker.snapshot().restart_pending);

        let guard = tracker.main_turn();
        assert_eq!(tracker.defer_restart_if_busy(), (true, true));
        assert!(tracker.snapshot().restart_pending);
        assert_eq!(tracker.defer_restart_if_busy(), (true, false));
        let events = drain(&mut rx);
        let flags: Vec<(bool, bool)> = events.iter().map(|e| (e.busy, e.restart_pending)).collect();
        assert_eq!(flags, [(true, false), (true, true)]);

        drop(guard);
        tracker.wait_until_turn_ends_or_cancelled().await;
        assert!(!tracker.snapshot().busy);
        assert!(tracker.snapshot().restart_pending);
        assert!(tracker.take_restart_pending());
        assert!(!tracker.snapshot().restart_pending);
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

    fn tracker_with_changes() -> (
        Arc<ActivityTracker>,
        crate::agent_watch::AgentChangeReceiver,
    ) {
        let feed = AgentChangeFeed::new();
        let receiver = feed.subscribe();
        let (tx, _rx) = broadcast::channel(16);
        (ActivityTracker::new("scout", tx, feed), receiver)
    }

    async fn next_turn(receiver: &mut crate::agent_watch::AgentChangeReceiver) -> MainTurnEnded {
        let change = residuum_core::testing::wait::next("the turn hook's report", receiver).await;
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
    async fn a_reply_seen_as_it_went_out_stays_seen_when_the_reader_leaves() {
        let (tracker, mut changes) = tracker_with_changes();
        let client = tracker.client_connected();
        tracker.main_message_published();
        drop(client);
        tracker.main_turn_ended(None, Some("Done.".to_string()), Visibility::User);
        assert!(next_turn(&mut changes).await.client_connected);
        assert_eq!(tracker.snapshot().unread, 0);
    }

    #[tokio::test]
    async fn a_reply_no_one_saw_is_seen_once_a_page_opens_before_the_turn_ends() {
        let (tracker, mut changes) = tracker_with_changes();
        tracker.main_message_published();
        assert_eq!(tracker.snapshot().unread, 1);
        let _client = tracker.client_connected();
        tracker.main_turn_ended(None, Some("Done.".to_string()), Visibility::User);
        assert!(next_turn(&mut changes).await.client_connected);
        assert_eq!(tracker.snapshot().unread, 0);
    }

    #[tokio::test]
    async fn a_reply_no_one_saw_is_unread_and_reported_unseen() {
        let (tracker, mut changes) = tracker_with_changes();
        tracker.main_message_published();
        tracker.main_turn_ended(None, Some("Done.".to_string()), Visibility::User);
        assert!(!next_turn(&mut changes).await.client_connected);
        assert_eq!(tracker.snapshot().unread, 1);
    }

    #[tokio::test]
    async fn a_turn_does_not_inherit_the_last_turns_seen_reply() {
        let (tracker, mut changes) = tracker_with_changes();
        let client = tracker.client_connected();
        tracker.main_message_published();
        drop(client);
        tracker.main_turn_ended(None, Some("Done.".to_string()), Visibility::User);
        assert!(next_turn(&mut changes).await.client_connected);

        tracker.main_message_published();
        tracker.main_turn_ended(None, Some("Again.".to_string()), Visibility::User);
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

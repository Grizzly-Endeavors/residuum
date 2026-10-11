//! Which devices have a Residuum window in front of the user.
//!
//! A web client on the hub WebSocket reports `presence { device_id, active }`
//! for its push device: `active: true` while a window is visible and focused,
//! repeated every 30 seconds while it stays so, and `active: false` when the
//! window hides or loses focus. A push to a device whose user is looking at
//! the app is redundant, so the triggers skip such a device (see
//! [`Presence::active_devices`]).
//!
//! A report holds for [`FRESH_FOR`]. It also ends when the connection that
//! made it closes, so a closed tab or a lost network never keeps a phone
//! quiet for the rest of the minute.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// How long a report of `active: true` holds without another one. The client
/// repeats it every 30 seconds, so one lost or late report doesn't end it.
pub const FRESH_FOR: Duration = Duration::from_secs(60);

/// The devices that are in front of their users, as the open hub sockets
/// report them.
#[derive(Default)]
pub struct Presence {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_connection: u64,
    /// Each open connection's devices that last reported `active: true`, and
    /// when. A connection reports for the device its window belongs to, so
    /// two windows of one browser are two connections for one device.
    connections: HashMap<u64, HashMap<String, Instant>>,
}

impl Presence {
    /// A book with no connections.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Start a connection's reports. Everything the connection reported ends
    /// when the returned guard drops.
    #[must_use]
    pub fn connect(self: &Arc<Self>) -> PresenceConnection {
        let mut state = self.lock();
        let id = state.next_connection;
        state.next_connection += 1;
        state.connections.insert(id, HashMap::new());
        PresenceConnection {
            presence: Arc::clone(self),
            id,
        }
    }

    /// The devices with a fresh `active: true` report from a connection that
    /// is still open.
    #[must_use]
    pub fn active_devices(&self) -> HashSet<String> {
        let now = Instant::now();
        self.lock()
            .connections
            .values()
            .flat_map(|devices| devices.iter())
            .filter(|(_, at)| is_fresh(**at, now))
            .map(|(device, _)| device.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn is_fresh(at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(at) < FRESH_FOR
}

/// One hub WebSocket connection's reports. Dropping it ends them.
pub struct PresenceConnection {
    presence: Arc<Presence>,
    id: u64,
}

impl PresenceConnection {
    /// The connection says its window for `device_id` is now `active` or not.
    pub fn report(&self, device_id: &str, active: bool) {
        let now = Instant::now();
        let mut state = self.presence.lock();
        let Some(devices) = state.connections.get_mut(&self.id) else {
            return;
        };
        // A device that stopped reporting without saying so has gone stale;
        // dropping it here keeps a long-lived connection's book short.
        devices.retain(|_, at| is_fresh(*at, now));
        if active {
            devices.insert(device_id.to_string(), now);
        } else {
            devices.remove(device_id);
        }
    }
}

impl Drop for PresenceConnection {
    fn drop(&mut self) {
        self.presence.lock().connections.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(presence: &Presence) -> Vec<String> {
        let mut active: Vec<String> = presence.active_devices().into_iter().collect();
        active.sort();
        active
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_is_present_while_its_active_report_is_fresh() {
        let presence = Presence::new();
        let connection = presence.connect();
        assert!(devices(&presence).is_empty(), "nothing reported yet");

        connection.report("phone", true);
        assert_eq!(devices(&presence), ["phone"]);

        tokio::time::advance(Duration::from_secs(59)).await;
        assert_eq!(devices(&presence), ["phone"], "still inside the minute");

        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            devices(&presence).is_empty(),
            "a report that isn't repeated goes stale"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn repeating_the_report_keeps_the_device_present() {
        let presence = Presence::new();
        let connection = presence.connect();

        for _ in 0..4 {
            connection.report("phone", true);
            tokio::time::advance(Duration::from_secs(30)).await;
        }

        assert_eq!(devices(&presence), ["phone"]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_inactive_report_ends_presence_at_once() {
        let presence = Presence::new();
        let connection = presence.connect();
        connection.report("phone", true);

        connection.report("phone", false);

        assert!(devices(&presence).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn closing_the_connection_ends_what_it_reported() {
        let presence = Presence::new();
        let staying = presence.connect();
        let leaving = presence.connect();
        staying.report("laptop", true);
        leaving.report("phone", true);
        assert_eq!(devices(&presence), ["laptop", "phone"]);

        drop(leaving);

        assert_eq!(devices(&presence), ["laptop"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_stays_present_while_any_of_its_windows_is_active() {
        let presence = Presence::new();
        let first = presence.connect();
        let second = presence.connect();
        first.report("laptop", true);
        second.report("laptop", true);

        // Focus moves from the first window to the second.
        first.report("laptop", false);
        assert_eq!(devices(&presence), ["laptop"]);

        second.report("laptop", false);
        assert!(devices(&presence).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn stale_reports_are_dropped_when_the_connection_reports_again() {
        let presence = Presence::new();
        let connection = presence.connect();
        connection.report("old", true);
        tokio::time::advance(FRESH_FOR).await;

        connection.report("new", true);

        let kept: usize = presence.lock().connections.values().map(HashMap::len).sum();
        assert_eq!(kept, 1, "only the fresh report is kept");
        assert_eq!(devices(&presence), ["new"]);
    }
}

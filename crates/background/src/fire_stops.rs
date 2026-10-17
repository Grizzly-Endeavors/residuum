//! Stop tokens that pulse fires hand to the sessions they ask for.
//!
//! A pulse fire's Stop has to reach its session even when it lands after the
//! fire published its spawn request and before the session registered. The
//! fire offers its stop token here under its fire id before it publishes the
//! request, and the request carries only that id. The spawn listener claims
//! the token when it forks the session, and the session's own stop token is
//! a child of it. So a fire stopped before the claim is never forked, and
//! one stopped after it has a session whose stop token is already cancelled
//! by the time it registers, which ends that run before its first turn.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

/// The stop tokens pulse fires have offered for the sessions they asked for,
/// keyed by fire id. Shared through the [`crate::SessionRegistry`], which the
/// fires and the spawn listener already hold.
#[derive(Default)]
pub struct FireStops {
    fires: Mutex<HashMap<String, OfferedStop>>,
}

struct OfferedStop {
    stop: CancellationToken,
    claimed: bool,
}

impl FireStops {
    /// Offer `stop`, the fire `fire_id`'s stop token, to the session the fire
    /// is about to ask for.
    pub fn offer(&self, fire_id: &str, stop: CancellationToken) {
        self.lock().insert(
            fire_id.to_string(),
            OfferedStop {
                stop,
                claimed: false,
            },
        );
    }

    /// Claim the stop token the fire `fire_id` offered, as the spawn listener
    /// forks its session: the token to stop that session with, a child of
    /// the fire's. `None` when the fire was stopped, or has ended and
    /// withdrawn its offer: the session must not be forked.
    pub fn claim(&self, fire_id: &str) -> Option<CancellationToken> {
        let mut fires = self.lock();
        let offered = fires.get_mut(fire_id)?;
        if offered.stop.is_cancelled() {
            return None;
        }
        offered.claimed = true;
        Some(offered.stop.child_token())
    }

    /// Withdraw the fire `fire_id`'s offer, when it is stopped or ends.
    /// Returns whether its session had already been claimed: if so, that
    /// session's stop token is a child of the fire's; if not, no session
    /// will be forked for the fire.
    pub fn withdraw(&self, fire_id: &str) -> bool {
        self.lock()
            .remove(fire_id)
            .is_some_and(|offered| offered.claimed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, OfferedStop>> {
        self.fires.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_session_stops_with_its_fire() {
        let stops = FireStops::default();
        let fire = CancellationToken::new();
        stops.offer("fire-1", fire.clone());

        let session = stops.claim("fire-1").expect("a live fire's session forks");
        assert!(!session.is_cancelled());
        fire.cancel();
        assert!(
            session.is_cancelled(),
            "the fire's stop reaches its session"
        );
        assert!(stops.withdraw("fire-1"), "the session was claimed");
    }

    #[test]
    fn a_fire_stopped_before_the_claim_forks_nothing() {
        let stops = FireStops::default();
        let fire = CancellationToken::new();
        stops.offer("fire-1", fire.clone());
        fire.cancel();

        assert!(stops.claim("fire-1").is_none());
        assert!(!stops.withdraw("fire-1"), "no session was claimed");
    }

    #[test]
    fn a_withdrawn_offer_forks_nothing() {
        let stops = FireStops::default();
        stops.offer("fire-1", CancellationToken::new());

        assert!(!stops.withdraw("fire-1"));
        assert!(stops.claim("fire-1").is_none());
        assert!(stops.claim("fire-unknown").is_none());
        assert!(!stops.withdraw("fire-unknown"));
    }
}
